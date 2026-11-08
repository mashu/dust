//! Google Drive's app data folder, as a folder sync keeps its files in
//! ([`Drive`]): Drive's REST API (v3), asked with the token the sign-in
//! gives ([`super::auth`]), straight from the device to Google and to
//! nowhere else.
//!
//! The folder is Dust's own, in the user's own Drive: hidden among their
//! files, out of reach of every other app, and counted against their own
//! storage. Drive's settings show it under Manage apps, with how much it
//! holds, and can delete it.

use std::rc::Rc;

use serde::Deserialize;

use super::auth::{Auth, AuthError, Counted};
use super::http::{Http, Request, Response};
use super::remote::{Remote, RemoteError, RemoteFile};
use super::text::{query, random_hex};
use crate::persist::record::Pending;

const FILES: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";
/// The folder's name to Drive.
const APP_FOLDER: &str = "appDataFolder";
/// The most files Drive lists at once.
const PAGE_SIZE: &str = "1000";

/// The app data folder of the user signed in with `auth`.
pub struct Drive {
    http: Rc<dyn Http>,
    auth: Rc<Counted>,
}

impl Drive {
    pub fn new(http: Rc<dyn Http>, auth: Rc<Counted>) -> Rc<Self> {
        Rc::new(Self { http, auth })
    }

    /// Send the request `make` makes with a token, and read its answer: once
    /// more with a fresh token where the one held was refused. Not with a
    /// token got once the sign-in was forgotten since the request began,
    /// as `began` counts it ([`Counted::forgotten`]): sync turned off, and
    /// maybe on anew with another account, whose drive it must not reach.
    async fn send(
        &self,
        began: u64,
        make: impl Fn(&str) -> Request,
    ) -> Result<Response, RemoteError> {
        let mut fresh = false;
        loop {
            let token = self.auth.token(fresh).await.map_err(signed_out)?;
            if self.auth.forgotten() != began {
                return Err(RemoteError::SignIn);
            }
            let answer = self
                .http
                .send(make(&token))
                .await
                .map_err(RemoteError::Failed)?;
            match read(answer) {
                Err(RemoteError::SignIn) if !fresh => fresh = true,
                read => return read,
            }
        }
    }
}

/// A sign-in that gave no token, as the folder says it.
fn signed_out(err: AuthError) -> RemoteError {
    match err {
        AuthError::NeedsUser | AuthError::Cancelled => RemoteError::SignIn,
        AuthError::Failed(why) => RemoteError::Failed(why),
    }
}

/// Drive's error, as it writes one.
#[derive(Deserialize)]
struct Failure {
    error: FailureBody,
}

#[derive(Deserialize)]
struct FailureBody {
    #[serde(default)]
    message: String,
    #[serde(default)]
    errors: Vec<FailureReason>,
}

#[derive(Deserialize)]
struct FailureReason {
    #[serde(default)]
    reason: String,
}

/// What `answer` says: itself, where it is a success; otherwise why not.
fn read(answer: Response) -> Result<Response, RemoteError> {
    if (200..300).contains(&answer.status) {
        return Ok(answer);
    }
    let failure = serde_json::from_slice::<Failure>(&answer.body).ok();
    let reasons: Vec<&str> = failure
        .iter()
        .flat_map(|failure| failure.error.errors.iter())
        .map(|reason| reason.reason.as_str())
        .collect();
    let message = failure
        .as_ref()
        .map(|failure| failure.error.message.trim())
        .filter(|message| !message.is_empty());
    Err(match answer.status {
        401 => RemoteError::SignIn,
        404 => RemoteError::Gone,
        403 if reasons.contains(&"storageQuotaExceeded") => RemoteError::Full,
        // Access given without the folder ticked, or taken back.
        403 if reasons.iter().any(|reason| {
            matches!(
                *reason,
                "insufficientPermissions" | "authError" | "forbidden"
            )
        }) =>
        {
            RemoteError::SignIn
        }
        status => RemoteError::Failed(match message {
            Some(message) => format!("Google Drive answered {status}: {message}"),
            None => format!("Google Drive answered {status}"),
        }),
    })
}

/// A page of the folder's listing, as Drive writes it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listing {
    #[serde(default)]
    files: Vec<Listed>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    id: String,
    name: String,
    #[serde(default)]
    created_time: String,
}

#[derive(Deserialize)]
struct Made {
    id: String,
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// A Drive file id as it goes in a path: Drive's are letters, digits, `-`
/// and `_`, and anything else is escaped all the same.
fn in_path(id: &str) -> String {
    super::text::percent(id)
}

/// The body of an upload of `bytes` as a new file called `name` in the app
/// folder: its metadata and its bytes, as one multipart/related body, and
/// the boundary between them, which none of the bytes hold.
fn multipart(name: &str, bytes: &[u8]) -> Option<(String, Vec<u8>)> {
    let boundary = loop {
        let boundary = format!("dust-{}", random_hex::<16>()?);
        if !bytes
            .windows(boundary.len())
            .any(|window| window == boundary.as_bytes())
        {
            break boundary;
        }
    };
    let metadata = serde_json::json!({ "name": name, "parents": [APP_FOLDER] }).to_string();
    let mut body = Vec::with_capacity(bytes.len() + metadata.len() + 200);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Some((boundary, body))
}

impl Remote for Rc<Drive> {
    fn list(&self) -> Pending<Result<Vec<RemoteFile>, RemoteError>> {
        let drive = Rc::clone(self);
        let began = drive.auth.forgotten();
        Box::pin(async move {
            let mut files = Vec::new();
            let mut page: Option<String> = None;
            loop {
                let mut asked = vec![
                    ("spaces", APP_FOLDER),
                    ("fields", "nextPageToken,files(id,name,createdTime)"),
                    ("pageSize", PAGE_SIZE),
                ];
                if let Some(page) = &page {
                    asked.push(("pageToken", page.as_str()));
                }
                let url = format!("{FILES}?{}", query(&asked));
                let answer = drive
                    .send(began, |token| {
                        Request::new("GET", url.clone()).header("Authorization", bearer(token))
                    })
                    .await?;
                let listing: Listing = serde_json::from_slice(&answer.body).map_err(|_| {
                    RemoteError::Failed("Google Drive's listing could not be read".into())
                })?;
                files.extend(listing.files.into_iter().map(|listed| RemoteFile {
                    id: listed.id,
                    name: listed.name,
                    created: listed.created_time,
                }));
                match listing.next_page_token {
                    Some(next) if !next.is_empty() && Some(&next) != page.as_ref() => {
                        page = Some(next)
                    }
                    _ => return Ok(files),
                }
            }
        })
    }

    fn get(&self, id: &str) -> Pending<Result<Vec<u8>, RemoteError>> {
        let drive = Rc::clone(self);
        let began = drive.auth.forgotten();
        let url = format!("{FILES}/{}?alt=media", in_path(id));
        Box::pin(async move {
            let answer = drive
                .send(began, |token| {
                    Request::new("GET", url.clone()).header("Authorization", bearer(token))
                })
                .await?;
            Ok(answer.body)
        })
    }

    fn create(&self, name: &str, bytes: Vec<u8>) -> Pending<Result<String, RemoteError>> {
        let drive = Rc::clone(self);
        let began = drive.auth.forgotten();
        let name = name.to_string();
        Box::pin(async move {
            let (boundary, body) = multipart(&name, &bytes).ok_or_else(|| {
                RemoteError::Failed("this device could not write the upload".into())
            })?;
            let url = format!("{UPLOAD}?uploadType=multipart&fields=id");
            let answer = drive
                .send(began, |token| {
                    Request::new("POST", url.clone())
                        .header("Authorization", bearer(token))
                        .header(
                            "Content-Type",
                            format!("multipart/related; boundary={boundary}"),
                        )
                        .body(body.clone())
                })
                .await?;
            let made: Made = serde_json::from_slice(&answer.body).map_err(|_| {
                RemoteError::Failed("Google Drive's answer could not be read".into())
            })?;
            Ok(made.id)
        })
    }

    fn update(&self, id: &str, bytes: Vec<u8>) -> Pending<Result<(), RemoteError>> {
        let drive = Rc::clone(self);
        let began = drive.auth.forgotten();
        let url = format!("{UPLOAD}/{}?uploadType=media&fields=id", in_path(id));
        Box::pin(async move {
            drive
                .send(began, |token| {
                    Request::new("PATCH", url.clone())
                        .header("Authorization", bearer(token))
                        .header("Content-Type", "application/octet-stream")
                        .body(bytes.clone())
                })
                .await?;
            Ok(())
        })
    }

    fn delete(&self, id: &str) -> Pending<Result<(), RemoteError>> {
        let drive = Rc::clone(self);
        let began = drive.auth.forgotten();
        let url = format!("{FILES}/{}", in_path(id));
        Box::pin(async move {
            drive
                .send(began, |token| {
                    Request::new("DELETE", url.clone()).header("Authorization", bearer(token))
                })
                .await?;
            Ok(())
        })
    }
}

#[cfg(test)]
pub mod fake;

#[cfg(test)]
mod tests;

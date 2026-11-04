//! Google Drive's REST API as a test meets it, in memory ([`FakeDrive`]):
//! the requests [`super::Drive`] makes, answered as Drive answers them, the
//! app data folder only, with a token checked on each; and a sign-in that
//! hands out tokens on a test's say ([`FakeAuth`]).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use super::*;
use crate::persist::record::done;
use crate::sync::text::parse_query;

/// Each file by its id: its name, when it was made, and its bytes.
type Files = RefCell<BTreeMap<String, (String, String, Vec<u8>)>>;

/// The app data folder, behind Drive's API.
#[derive(Default)]
pub struct FakeDrive {
    files: Files,
    made: Cell<u64>,
    /// The tokens it takes; any other is refused, as an expired one is.
    pub tokens: RefCell<Vec<String>>,
    /// The most files a page of the listing holds, where fewer than Drive's
    /// own, to page through a short one.
    pub page: Cell<Option<usize>>,
    /// The drive is full: no file made or changed.
    pub full: Cell<bool>,
    /// Every request, as `METHOD url`.
    pub asked: RefCell<Vec<String>>,
}

fn json(status: u16, value: serde_json::Value) -> Response {
    Response {
        status,
        body: value.to_string().into_bytes(),
    }
}

fn failure(status: u16, reason: &str, message: &str) -> Response {
    json(
        status,
        serde_json::json!({
            "error": { "code": status, "message": message, "errors": [{ "reason": reason }] }
        }),
    )
}

impl FakeDrive {
    pub fn taking(token: &str) -> Rc<Self> {
        let drive = Rc::new(Self::default());
        drive.tokens.borrow_mut().push(token.into());
        drive
    }

    /// The names of every file, in the order they were made.
    pub fn names(&self) -> Vec<String> {
        let files = self.files.borrow();
        let mut all: Vec<(&String, &String)> =
            files.values().map(|(name, made, _)| (made, name)).collect();
        all.sort();
        all.into_iter().map(|(_, name)| name.clone()).collect()
    }

    fn answer(&self, request: &Request) -> Response {
        self.asked
            .borrow_mut()
            .push(format!("{} {}", request.method, request.url));
        let token = request
            .header_of("Authorization")
            .and_then(|value| value.strip_prefix("Bearer "));
        if !token.is_some_and(|token| self.tokens.borrow().iter().any(|held| held == token)) {
            return failure(401, "authError", "Invalid Credentials");
        }
        let (path, asked) = request.url.split_once('?').unwrap_or((&request.url, ""));
        let asked: BTreeMap<String, String> = parse_query(asked).into_iter().collect();
        let files_at = "https://www.googleapis.com/drive/v3/files";
        let upload_at = "https://www.googleapis.com/upload/drive/v3/files";
        match (request.method, path) {
            ("GET", path) if path == files_at => self.list(&asked),
            ("GET", path) if path.starts_with(&format!("{files_at}/")) => {
                assert_eq!(asked.get("alt").map(String::as_str), Some("media"));
                let id = &path[files_at.len() + 1..];
                match self.files.borrow().get(id) {
                    Some((_, _, bytes)) => Response {
                        status: 200,
                        body: bytes.clone(),
                    },
                    None => failure(404, "notFound", "File not found"),
                }
            }
            ("POST", path) if path == upload_at => {
                assert_eq!(
                    asked.get("uploadType").map(String::as_str),
                    Some("multipart")
                );
                if self.full.get() {
                    return failure(
                        403,
                        "storageQuotaExceeded",
                        "The user's Drive storage quota has been exceeded.",
                    );
                }
                let (name, bytes) = read_multipart(request);
                let made = self.made.get() + 1;
                self.made.set(made);
                let id = format!("id{made}");
                let created = format!("2026-11-04T10:00:{made:06}.000Z");
                self.files
                    .borrow_mut()
                    .insert(id.clone(), (name, created, bytes));
                json(200, serde_json::json!({ "id": id }))
            }
            ("PATCH", path) if path.starts_with(&format!("{upload_at}/")) => {
                assert_eq!(asked.get("uploadType").map(String::as_str), Some("media"));
                if self.full.get() {
                    return failure(
                        403,
                        "storageQuotaExceeded",
                        "The user's Drive storage quota has been exceeded.",
                    );
                }
                let id = &path[upload_at.len() + 1..];
                match self.files.borrow_mut().get_mut(id) {
                    Some(file) => {
                        file.2 = request.body.clone();
                        json(200, serde_json::json!({ "id": id }))
                    }
                    None => failure(404, "notFound", "File not found"),
                }
            }
            ("DELETE", path) if path.starts_with(&format!("{files_at}/")) => {
                let id = &path[files_at.len() + 1..];
                match self.files.borrow_mut().remove(id) {
                    Some(_) => Response {
                        status: 204,
                        body: Vec::new(),
                    },
                    None => failure(404, "notFound", "File not found"),
                }
            }
            _ => failure(
                400,
                "badRequest",
                &format!("no such request: {}", request.url),
            ),
        }
    }

    fn list(&self, asked: &BTreeMap<String, String>) -> Response {
        assert_eq!(
            asked.get("spaces").map(String::as_str),
            Some("appDataFolder")
        );
        let fields = asked.get("fields").expect("fields");
        assert!(
            fields.contains("nextPageToken") && fields.contains("files(id,name,createdTime)"),
            "{fields}"
        );
        let size: usize = asked
            .get("pageSize")
            .expect("a page size")
            .parse()
            .expect("a number");
        assert!(size <= 1000);
        let size = self.page.get().map_or(size, |page| page.min(size));
        let from: usize = asked
            .get("pageToken")
            .map_or(0, |token| token.parse().expect("a page"));
        let files = self.files.borrow();
        let page: Vec<serde_json::Value> = files
            .iter()
            .skip(from)
            .take(size)
            .map(|(id, (name, created, _))| serde_json::json!({ "id": id, "name": name, "createdTime": created }))
            .collect();
        let mut listing = serde_json::json!({ "files": page });
        if from + size < files.len() {
            listing["nextPageToken"] = (from + size).to_string().into();
        }
        json(200, listing)
    }
}

/// The name and bytes of a multipart upload, as Drive reads one: its
/// metadata asking for a file in the app folder, then its bytes.
fn read_multipart(request: &Request) -> (String, Vec<u8>) {
    let kind = request.header_of("Content-Type").expect("a content type");
    let boundary = kind
        .strip_prefix("multipart/related; boundary=")
        .expect("multipart/related");
    let body = &request.body;
    let opening = format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n");
    assert!(body.starts_with(opening.as_bytes()));
    let rest = &body[opening.len()..];
    let between = format!("\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n");
    let split = rest
        .windows(between.len())
        .position(|window| window == between.as_bytes())
        .expect("two parts");
    let metadata: serde_json::Value = serde_json::from_slice(&rest[..split]).expect("metadata");
    assert_eq!(metadata["parents"], serde_json::json!(["appDataFolder"]));
    let content = &rest[split + between.len()..];
    let closing = format!("\r\n--{boundary}--\r\n");
    assert!(content.ends_with(closing.as_bytes()));
    let bytes = content[..content.len() - closing.len()].to_vec();
    (
        metadata["name"].as_str().expect("a name").to_string(),
        bytes,
    )
}

impl Http for Rc<FakeDrive> {
    fn send(&self, request: Request) -> Pending<Result<Response, String>> {
        done(Ok(self.answer(&request)))
    }
}

/// A sign-in that is signed in or not as a test says, and gives the next
/// token it holds when one is refused.
#[derive(Default)]
pub struct FakeAuth {
    pub signed_in: Cell<bool>,
    /// The token it gives, and those it gives in turn once one is refused.
    pub tokens: RefCell<Vec<String>>,
    /// How many times the user was asked.
    pub asked: Cell<usize>,
    /// The user turns the sign-in down.
    pub refuses: Cell<bool>,
    /// What it keeps to sign in again, and was given to keep.
    pub kept: RefCell<Option<String>>,
}

impl FakeAuth {
    pub fn giving(tokens: &[&str]) -> Rc<Self> {
        let auth = Rc::new(Self::default());
        *auth.tokens.borrow_mut() = tokens.iter().map(|token| token.to_string()).collect();
        auth
    }

    pub fn signed_in(tokens: &[&str]) -> Rc<Self> {
        let auth = Self::giving(tokens);
        auth.signed_in.set(true);
        auth
    }
}

impl Auth for FakeAuth {
    fn token(&self, fresh: bool) -> Pending<Result<String, AuthError>> {
        if !self.signed_in.get() {
            return done(Err(AuthError::NeedsUser));
        }
        let mut tokens = self.tokens.borrow_mut();
        if fresh && tokens.len() > 1 {
            tokens.remove(0);
        }
        done(tokens.first().cloned().ok_or(AuthError::NeedsUser))
    }

    fn sign_in(&self) -> Pending<Result<(), AuthError>> {
        self.asked.set(self.asked.get() + 1);
        if self.refuses.get() {
            return done(Err(AuthError::Cancelled));
        }
        self.signed_in.set(true);
        *self.kept.borrow_mut() = Some("kept".into());
        done(Ok(()))
    }

    fn kept(&self) -> Option<String> {
        self.kept.borrow().clone()
    }

    fn keep(&self, kept: Option<String>) {
        if kept.is_some() {
            self.signed_in.set(true);
        }
        *self.kept.borrow_mut() = kept;
    }

    fn forget(&self) {
        self.signed_in.set(false);
        *self.kept.borrow_mut() = None;
    }
}

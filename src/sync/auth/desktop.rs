//! Signing in on a computer: Google's sign-in opened in the user's own
//! browser, which, once they have said yes, sends the browser back to the
//! app at `http://127.0.0.1:<port>`, a port the app listens on for that one
//! answer and no longer (RFC 8252, with PKCE: RFC 7636). The code it brings
//! is traded for a token, and for a refresh token, which the app keeps to
//! sign in again with no one asked ([`super::Auth::kept`]).
//!
//! Google calls a desktop client's secret no secret: every copy of the app
//! holds it. What keeps the code to this copy alone is the PKCE verifier,
//! made afresh for each sign-in and never sent until the trade.

use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{Auth, AuthError};
use crate::persist::record::Pending;
use crate::sync::config::SCOPE;
use crate::sync::http::{Http, Request};
use crate::sync::text::{base64url, parse_query, query, random, random_hex};
use crate::time::now_ms;

const AUTH_AT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_AT: &str = "https://oauth2.googleapis.com/token";
/// How long the app listens for the browser to come back.
const WAIT: Duration = Duration::from_secs(10 * 60);
/// How long before a token runs out it is taken as run out.
const EARLY_MS: u64 = 60_000;

/// What the browser is shown once it is back at the app.
const SIGNED_IN_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Dust</title><body style=\"font:1.1rem system-ui;margin:3rem\"><p>Dust has its folder in your Google Drive. You can close this tab and go back to Dust.</p>";
const NOT_SIGNED_IN_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Dust</title><body style=\"font:1.1rem system-ui;margin:3rem\"><p>Dust was not signed in. You can close this tab and go back to Dust.</p>";

/// The client this build signs in as.
#[derive(Clone, Debug)]
pub struct Client {
    pub id: String,
    pub secret: String,
}

/// How a URL is opened in the user's browser: as the app opens it, or as a
/// test does.
pub type Opener = Box<dyn Fn(&str) -> Result<(), String>>;

/// The sign-in, shared with each request it makes on its way.
pub struct LoopbackAuth(Rc<Inner>);

struct Inner {
    http: Rc<dyn Http>,
    client: Client,
    open: Opener,
    /// The access token held, and when it runs out.
    held: RefCell<Option<(String, u64)>>,
    refresh: RefCell<Option<String>>,
    /// What stops the sign-in waiting on the browser.
    waiting: RefCell<Option<Arc<AtomicBool>>>,
    /// Which sign-in what is held is of: moved on as it is forgotten, and
    /// as another sign-in begins, so that a trade under way across either
    /// keeps nothing of what it is answered, which may be of another
    /// account than the one signed in since.
    epoch: Cell<u64>,
}

impl LoopbackAuth {
    /// The app's: the build's desktop client, the user's own browser.
    #[cfg(feature = "desktop")]
    pub fn new(http: Rc<dyn Http>) -> Self {
        let client = Client {
            id: crate::sync::config::DESKTOP_CLIENT_ID
                .unwrap_or_default()
                .into(),
            secret: crate::sync::config::DESKTOP_CLIENT_SECRET
                .unwrap_or_default()
                .into(),
        };
        let open: Opener = Box::new(|url| {
            webbrowser::open(url).map_err(|_| "Dust could not open your browser".to_string())
        });
        Self::with(http, client, open)
    }

    pub fn with(http: Rc<dyn Http>, client: Client, open: Opener) -> Self {
        Self(Rc::new(Inner {
            http,
            client,
            open,
            held: RefCell::new(None),
            refresh: RefCell::new(None),
            waiting: RefCell::new(None),
            epoch: Cell::new(0),
        }))
    }
}

/// Google's answer to a trade, a code or a refresh token for a token.
#[derive(Deserialize)]
struct Traded {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct Refused {
    #[serde(default)]
    error: String,
}

/// What the browser brought back.
#[derive(Debug, PartialEq, Eq)]
enum Brought {
    Code(String),
    Refused,
}

/// Listen at `listener` for the browser to come back with an answer of
/// `state`'s, until `stop` or the time is up.
fn wait_for_browser(
    listener: &TcpListener,
    state: &str,
    stop: &AtomicBool,
) -> Result<Brought, AuthError> {
    listener
        .set_nonblocking(true)
        .map_err(|err| AuthError::Failed(err.to_string()))?;
    // Each connection answered on a thread of its own: a browser opens
    // some it sends nothing on, ahead of need, and none of them may keep
    // the one bringing the answer waiting.
    let (heard, hearing) = std::sync::mpsc::channel();
    let until = Instant::now() + WAIT;
    while Instant::now() < until {
        if stop.load(Ordering::Relaxed) {
            return Err(AuthError::Cancelled);
        }
        if let Ok(brought) = hearing.try_recv() {
            return Ok(brought);
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let (heard, state) = (heard.clone(), state.to_string());
                std::thread::spawn(move || {
                    if let Some(brought) = answer_browser(stream, &state) {
                        let _ = heard.send(brought);
                    }
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(AuthError::Failed(err.to_string())),
        }
    }
    Err(AuthError::Cancelled)
}

/// Read what the browser asks at `stream`, tell it it can go back to the
/// app, and say what it brought: none where it is no answer to this
/// sign-in, as a browser asking for an icon is not.
fn answer_browser(mut stream: TcpStream, state: &str) -> Option<Brought> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut line = String::new();
    // No more than a request line ever holds.
    BufReader::new(Read::take(&stream, 16 * 1024))
        .read_line(&mut line)
        .ok()?;
    let target = line.split_whitespace().nth(1)?;
    let asked = parse_query(target.split_once('?').map_or("", |(_, asked)| asked));
    let value = |key: &str| {
        asked
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, value)| value.clone())
    };
    let brought = if value("state").as_deref() != Some(state) {
        None
    } else if let Some(code) = value("code") {
        Some(Brought::Code(code))
    } else {
        Some(Brought::Refused)
    };
    let (status, page) = match &brought {
        Some(Brought::Code(_)) => ("200 OK", SIGNED_IN_PAGE),
        Some(Brought::Refused) => ("200 OK", NOT_SIGNED_IN_PAGE),
        None => ("404 Not Found", ""),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
        page.len()
    );
    let _ = stream.flush();
    brought
}

impl Inner {
    /// Trade `form` at Google's token endpoint for a token, kept as held:
    /// and its refresh token, where it gives one. Answered once the
    /// sign-in of `epoch` is forgotten, or another has begun, it keeps
    /// nothing, and comes to nothing, as a sign-in given up does.
    async fn trade(&self, epoch: u64, form: &[(&str, &str)]) -> Result<String, AuthError> {
        let request = Request::new("POST", TOKEN_AT)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(query(form).into_bytes());
        let answer = self.http.send(request).await.map_err(AuthError::Failed)?;
        if self.epoch.get() != epoch {
            return Err(AuthError::Cancelled);
        }
        if !(200..300).contains(&answer.status) {
            let refused: Option<Refused> = serde_json::from_slice(&answer.body).ok();
            return Err(match refused.map(|refused| refused.error).as_deref() {
                // The refresh token is spent or taken back: only the user
                // can give another.
                Some("invalid_grant") => {
                    *self.refresh.borrow_mut() = None;
                    AuthError::NeedsUser
                }
                _ => AuthError::Failed(format!("Google answered {}", answer.status)),
            });
        }
        let traded: Traded = serde_json::from_slice(&answer.body)
            .map_err(|_| AuthError::Failed("Google's answer could not be read".into()))?;
        if let Some(scope) = &traded.scope
            && !scope.split_whitespace().any(|granted| granted == SCOPE)
        {
            return Err(AuthError::Failed(
                "Dust was not given its folder in your Google Drive: tick it when Google asks"
                    .into(),
            ));
        }
        if let Some(refresh) = traded.refresh_token {
            *self.refresh.borrow_mut() = Some(refresh);
        }
        let expires = now_ms() + traded.expires_in.max(60) * 1000;
        *self.held.borrow_mut() = Some((traded.access_token.clone(), expires));
        Ok(traded.access_token)
    }
}

impl Auth for LoopbackAuth {
    fn token(&self, fresh: bool) -> Pending<Result<String, AuthError>> {
        let held = self.0.held.borrow().clone();
        if let Some((token, expires)) = held
            && !fresh
            && expires > now_ms() + EARLY_MS
        {
            return Box::pin(std::future::ready(Ok(token)));
        }
        let Some(refresh) = self.0.refresh.borrow().clone() else {
            return Box::pin(std::future::ready(Err(AuthError::NeedsUser)));
        };
        let this = Rc::clone(&self.0);
        let epoch = this.epoch.get();
        Box::pin(async move {
            this.trade(
                epoch,
                &[
                    ("client_id", &this.client.id),
                    ("client_secret", &this.client.secret),
                    ("refresh_token", &refresh),
                    ("grant_type", "refresh_token"),
                ],
            )
            .await
        })
    }

    fn sign_in(&self) -> Pending<Result<(), AuthError>> {
        let fail = |why: String| -> Pending<Result<(), AuthError>> {
            Box::pin(std::future::ready(Err(AuthError::Failed(why))))
        };
        let this = &self.0;
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(err) => {
                return fail(format!(
                    "this computer would not let Dust listen for Google ({err})"
                ));
            }
        };
        let port = match listener.local_addr() {
            Ok(address) => address.port(),
            Err(err) => return fail(err.to_string()),
        };
        let (Some(verifier), Some(state)) = (
            random::<32>().map(|bytes| base64url(&bytes)),
            random_hex::<16>(),
        ) else {
            return fail("this computer had no randomness to sign in with".into());
        };
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        let redirect = format!("http://127.0.0.1:{port}");
        let url = format!(
            "{AUTH_AT}?{}",
            query(&[
                ("client_id", &this.client.id),
                ("redirect_uri", &redirect),
                ("response_type", "code"),
                ("scope", SCOPE),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
                ("state", &state),
                ("access_type", "offline"),
                ("prompt", "consent"),
            ])
        );
        if let Err(why) = (this.open)(&url) {
            return fail(why);
        }
        // What is under way from before stops: a sign-in waiting on the
        // browser, and a trade, which keeps nothing of its answer.
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(earlier) = this.waiting.replace(Some(Arc::clone(&stop))) {
            earlier.store(true, Ordering::Relaxed);
        }
        let epoch = this.epoch.get() + 1;
        this.epoch.set(epoch);
        let (answer, answered) = tokio::sync::oneshot::channel();
        let waiting_state = state;
        let spawned = std::thread::Builder::new()
            .name("dust-sign-in".into())
            .spawn(move || {
                let _ = answer.send(wait_for_browser(&listener, &waiting_state, &stop));
            });
        if let Err(err) = spawned {
            return fail(err.to_string());
        }
        let this = Rc::clone(&self.0);
        Box::pin(async move {
            let brought = answered.await.map_err(|_| AuthError::Cancelled)??;
            let Brought::Code(code) = brought else {
                return Err(AuthError::Cancelled);
            };
            this.trade(
                epoch,
                &[
                    ("client_id", &this.client.id),
                    ("client_secret", &this.client.secret),
                    ("code", &code),
                    ("code_verifier", &verifier),
                    ("grant_type", "authorization_code"),
                    ("redirect_uri", &redirect),
                ],
            )
            .await?;
            Ok(())
        })
    }

    fn cancel(&self) {
        if let Some(waiting) = self.0.waiting.borrow_mut().take() {
            waiting.store(true, Ordering::Relaxed);
        }
    }

    fn kept(&self) -> Option<String> {
        self.0.refresh.borrow().clone()
    }

    fn keep(&self, kept: Option<String>) {
        *self.0.refresh.borrow_mut() = kept;
    }

    fn forget(&self) {
        self.cancel();
        self.0.epoch.set(self.0.epoch.get() + 1);
        *self.0.held.borrow_mut() = None;
        *self.0.refresh.borrow_mut() = None;
    }
}

#[cfg(test)]
mod tests;

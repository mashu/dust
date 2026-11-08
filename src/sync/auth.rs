//! Signing in to Google, for an access token to Drive's app data folder
//! and nothing else ([`super::config::SCOPE`]): what [`super::drive`] asks
//! with. Each platform signs in its own way, as Google asks each to:
//!
//! - in a browser, Google's sign-in in a window of its own, which sends the
//!   token back to a page of Dust's, `oauth.html`, beside the app; that page
//!   hands it to the app over a channel only pages of the same site hear
//!   ([`web`]). A browser's token lasts an hour, and is kept in memory only:
//!   the next visit signs in again as it syncs, with no more than a click.
//! - on a computer, Google's sign-in in the user's own browser, which sends
//!   a code back to the app at a port of its own on the computer itself
//!   (RFC 8252); the app trades it for a token, and for a refresh token it
//!   keeps, to sign in again with no one asked ([`desktop`]).
//! - on Android, Google Play services, which asks the user once, in its own
//!   sheet, and gives the app a token each time after ([`android`]).
//!
//! Nothing here is sent anywhere but to Google.

use std::cell::Cell;
use std::rc::Rc;

#[cfg(any(feature = "web", test))]
use super::config::SCOPE;
use crate::persist::record::Pending;

/// Why there is no token. A build with no sign-in of its own, as iOS's,
/// meets none of them.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(any(feature = "web", feature = "desktop", target_os = "android", test)),
    allow(dead_code)
)]
pub enum AuthError {
    /// Only the user can sign in: none has, here, or the access given has
    /// been taken back.
    NeedsUser,
    /// The user turned the sign-in down, or closed it.
    Cancelled,
    /// For the reason given, in words that follow "Signing in failed:".
    Failed(String),
}

impl AuthError {
    /// What to tell the user.
    pub fn told(&self) -> String {
        match self {
            Self::NeedsUser => "Sign in to Google to sync.".into(),
            Self::Cancelled => "Signing in to Google was cancelled.".into(),
            Self::Failed(why) => format!(
                "Signing in to Google failed: {}.",
                why.trim_end_matches('.')
            ),
        }
    }
}

/// A sign-in to Google.
pub trait Auth {
    /// An access token, with no one asked: the one held, or, `fresh`, a new
    /// one in its place, as where the one held was refused.
    fn token(&self, fresh: bool) -> Pending<Result<String, AuthError>>;
    /// Ask the user to sign in. Called from the press of a button: a
    /// browser lets a page open a window only then, so the browser's opens
    /// Google's there and then, before anything waits.
    fn sign_in(&self) -> Pending<Result<(), AuthError>>;
    /// Stop a sign-in that waits on the user.
    fn cancel(&self) {}
    /// What this device keeps to sign in again with no one asked, once
    /// signed in: a refresh token, on a computer.
    fn kept(&self) -> Option<String> {
        None
    }
    /// Take up what was kept, as the app opens.
    fn keep(&self, _kept: Option<String>) {}
    /// Forget every token held here.
    fn forget(&self);
}

/// A sign-in that counts the times it is forgotten, as sync is turned off:
/// what a request to the drive goes by, so that one begun before asks
/// nothing more with a token got since, which may be of another account
/// signed in after it ([`super::drive::Drive`]).
pub struct Counted {
    auth: Rc<dyn Auth>,
    forgotten: Cell<u64>,
}

impl Counted {
    pub fn of(auth: Rc<dyn Auth>) -> Rc<Self> {
        Rc::new(Self {
            auth,
            forgotten: Cell::new(0),
        })
    }

    /// How many times it has been forgotten.
    pub fn forgotten(&self) -> u64 {
        self.forgotten.get()
    }
}

impl Auth for Counted {
    fn token(&self, fresh: bool) -> Pending<Result<String, AuthError>> {
        self.auth.token(fresh)
    }

    fn sign_in(&self) -> Pending<Result<(), AuthError>> {
        self.auth.sign_in()
    }

    fn cancel(&self) {
        self.auth.cancel();
    }

    fn kept(&self) -> Option<String> {
        self.auth.kept()
    }

    fn keep(&self, kept: Option<String>) {
        self.auth.keep(kept);
    }

    fn forget(&self) {
        self.forgotten.set(self.forgotten.get() + 1);
        self.auth.forget();
    }
}

/// What `oauth.html` was sent, as the browser's sign-in reads it: the
/// token and how long it lasts, or why there is none. Only an answer to the
/// sign-in of `state` is read at all.
#[cfg(any(feature = "web", test))]
pub(super) fn read_answer(heard: &str, state: &str) -> Option<Result<(String, u64), AuthError>> {
    let heard = heard.trim_start_matches(['#', '?']);
    let pairs = crate::sync::text::parse_query(heard);
    let value = |key: &str| {
        pairs
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, value)| value.as_str())
    };
    if value("state") != Some(state) {
        return None;
    }
    if let Some(error) = value("error") {
        return Some(Err(match error {
            "access_denied" => AuthError::Cancelled,
            error => AuthError::Failed(format!("Google said {error}")),
        }));
    }
    let token = value("access_token")?;
    if let Some(scope) = value("scope")
        && !scope.split_whitespace().any(|granted| granted == SCOPE)
    {
        return Some(Err(AuthError::Failed(
            "Dust was not given its folder in your Google Drive: tick it when Google asks".into(),
        )));
    }
    let lasts: u64 = value("expires_in")
        .and_then(|secs| secs.parse().ok())
        .unwrap_or(3600);
    Some(Ok((token.to_string(), lasts)))
}

/// How this build signs in: none where it cannot sync
/// ([`super::config::available`]).
pub fn default_auth() -> Option<Rc<dyn Auth>> {
    if !super::config::available() {
        return None;
    }
    #[cfg(feature = "web")]
    {
        Some(Rc::new(web::BrowserAuth::default()))
    }
    #[cfg(all(feature = "desktop", not(target_os = "android")))]
    {
        Some(Rc::new(desktop::LoopbackAuth::new(
            super::http::default_http(),
        )))
    }
    #[cfg(all(target_os = "android", not(feature = "web"), not(test)))]
    {
        Some(Rc::new(android::PlayAuth))
    }
    #[cfg(not(any(
        feature = "web",
        feature = "desktop",
        all(target_os = "android", not(test))
    )))]
    {
        None
    }
}

#[cfg(all(target_os = "android", not(feature = "web"), not(test)))]
mod android;
#[cfg(any(feature = "desktop", test))]
pub mod desktop;
#[cfg(feature = "web")]
mod web;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_read_only_for_its_own_sign_in() {
        let heard = format!(
            "#access_token=ya29.x&token_type=Bearer&expires_in=3599&scope={}&state=abc",
            crate::sync::text::percent(SCOPE)
        );
        assert_eq!(
            read_answer(&heard, "abc"),
            Some(Ok(("ya29.x".into(), 3599)))
        );
        assert_eq!(read_answer(&heard, "other"), None);
        assert_eq!(
            read_answer("#error=access_denied&state=abc", "abc"),
            Some(Err(AuthError::Cancelled))
        );
        assert!(matches!(
            read_answer("#access_token=t&scope=openid&state=abc", "abc"),
            Some(Err(AuthError::Failed(_)))
        ));
        assert_eq!(
            read_answer("#state=abc", "abc"),
            None,
            "no token, no answer"
        );
    }
}

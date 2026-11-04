//! Signing in in a browser: Google's sign-in in a window of its own, which
//! sends the token back to `oauth.html` beside the app (OAuth's token flow
//! for a page with no server, which is all Dust's web app is). That page
//! hands what it was sent to the app over a `BroadcastChannel`, which only
//! pages of the same site hear, and closes. A window Google's page opened
//! has no way back to the page that opened it, so a channel it is.
//!
//! The token lasts an hour, and is kept in memory only: no other page of
//! the site can read it from the browser's storage, and the next visit
//! signs in again, with a click, as it syncs.

use std::cell::RefCell;
use std::rc::Rc;
use std::task::{Poll, Waker};

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use super::{Auth, AuthError, read_answer};
use crate::persist::record::Pending;
use crate::sync::config::{SCOPE, WEB_CLIENT_ID};
use crate::sync::text::{query, random_hex};
use crate::time::now_ms;

const AUTH_AT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// The channel `oauth.html` answers on.
const CHANNEL: &str = "dust_oauth";
/// How long before a token runs out it is taken as run out.
const EARLY_MS: u64 = 60_000;

/// What a sign-in waiting on Google's window comes to, and what waits on it.
#[derive(Default)]
struct Answer {
    came: Option<Result<(), AuthError>>,
    waker: Option<Waker>,
}

impl Answer {
    fn give(answer: &Rc<RefCell<Self>>, came: Result<(), AuthError>) {
        let mut answer = answer.borrow_mut();
        if answer.came.is_none() {
            answer.came = Some(came);
        }
        if let Some(waker) = answer.waker.take() {
            waker.wake();
        }
    }
}

/// The channel listened on, while a sign-in waits.
struct Listening {
    channel: web_sys::BroadcastChannel,
    _heard: Closure<dyn FnMut(web_sys::MessageEvent)>,
    answer: Rc<RefCell<Answer>>,
}

#[derive(Default)]
pub struct BrowserAuth {
    /// The token held, and when it runs out, shared with the channel's
    /// listener, which sets it.
    held: Rc<RefCell<Option<(String, u64)>>>,
    listening: RefCell<Option<Listening>>,
}

/// Where Google sends the token: `oauth.html`, beside the page the app is.
fn redirect_uri() -> Option<String> {
    let location = web_sys::window()?.location();
    let origin = location.origin().ok()?;
    let path = location.pathname().ok()?;
    let folder = &path[..=path.rfind('/')?];
    Some(format!("{origin}{folder}oauth.html"))
}

impl BrowserAuth {
    /// Stop listening, and say `why` to a sign-in still waiting.
    fn stop(&self, why: AuthError) {
        if let Some(listening) = self.listening.borrow_mut().take() {
            listening.channel.set_onmessage(None);
            listening.channel.close();
            Answer::give(&listening.answer, Err(why));
        }
    }
}

impl Auth for BrowserAuth {
    fn token(&self, fresh: bool) -> Pending<Result<String, AuthError>> {
        let held = self.held.borrow().clone();
        let token = held
            .filter(|(_, expires)| !fresh && *expires > now_ms() + EARLY_MS)
            .map(|(token, _)| token)
            .ok_or(AuthError::NeedsUser);
        Box::pin(std::future::ready(token))
    }

    fn sign_in(&self) -> Pending<Result<(), AuthError>> {
        let failed = |why: &str| -> Pending<Result<(), AuthError>> {
            Box::pin(std::future::ready(Err(AuthError::Failed(why.into()))))
        };
        let (Some(window), Some(redirect), Some(state), Some(client)) = (
            web_sys::window(),
            redirect_uri(),
            random_hex::<16>(),
            WEB_CLIENT_ID,
        ) else {
            return failed("this browser cannot sign in here");
        };
        let url = format!(
            "{AUTH_AT}?{}",
            query(&[
                ("client_id", client),
                ("redirect_uri", &redirect),
                ("response_type", "token"),
                ("scope", SCOPE),
                ("state", &state),
                ("include_granted_scopes", "false"),
            ])
        );
        // Opened first, while the press that called this still counts.
        match window.open_with_url_and_target_and_features(
            &url,
            "dust_google",
            "popup,width=480,height=640",
        ) {
            Ok(Some(_)) => {}
            _ => {
                return failed(
                    "this browser blocked Google's sign-in window. Allow pop-ups for this site, then try again",
                );
            }
        }
        self.stop(AuthError::Cancelled);
        let Ok(channel) = web_sys::BroadcastChannel::new(CHANNEL) else {
            return failed("this browser cannot hear Google's answer");
        };
        let answer = Rc::new(RefCell::new(Answer::default()));
        let held = Rc::clone(&self.held);
        let heard_answer = Rc::clone(&answer);
        let heard_channel = channel.clone();
        let heard = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
            move |event: web_sys::MessageEvent| {
                let Some(heard) = event.data().as_string() else {
                    return;
                };
                let Some(read) = read_answer(&heard, &state) else {
                    return;
                };
                heard_channel.set_onmessage(None);
                heard_channel.close();
                Answer::give(
                    &heard_answer,
                    read.map(|(token, lasts)| {
                        *held.borrow_mut() = Some((token, now_ms() + lasts * 1000));
                    }),
                );
            },
        );
        channel.set_onmessage(Some(heard.as_ref().unchecked_ref()));
        *self.listening.borrow_mut() = Some(Listening {
            channel,
            _heard: heard,
            answer: Rc::clone(&answer),
        });
        Box::pin(std::future::poll_fn(move |asked| {
            let mut waiting = answer.borrow_mut();
            match waiting.came.take() {
                Some(came) => Poll::Ready(came),
                None => {
                    waiting.waker = Some(asked.waker().clone());
                    Poll::Pending
                }
            }
        }))
    }

    fn cancel(&self) {
        self.stop(AuthError::Cancelled);
    }

    fn forget(&self) {
        self.stop(AuthError::Cancelled);
        *self.held.borrow_mut() = None;
    }
}

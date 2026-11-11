//! The computer's sign-in against a browser of a test's own, which goes
//! where Google would send it, and a token endpoint of its own.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;

use super::*;
use crate::persist::record::done;
use crate::sync::http::Response;

/// Google's token endpoint, answering each trade as a test says: at once,
/// or, the next once `hold` is set, as the test answers it through `held`.
#[derive(Default)]
struct Endpoint {
    answers: RefCell<Vec<Response>>,
    asked: RefCell<Vec<BTreeMap<String, String>>>,
    hold: Cell<bool>,
    held: RefCell<Option<tokio::sync::oneshot::Sender<Response>>>,
}

impl Http for Rc<Endpoint> {
    fn send(&self, request: Request) -> Pending<Result<Response, String>> {
        assert_eq!((request.method, request.url.as_str()), ("POST", TOKEN_AT));
        let form = String::from_utf8(request.body).expect("a form");
        self.asked
            .borrow_mut()
            .push(parse_query(&form).into_iter().collect());
        if self.hold.replace(false) {
            let (answering, answered) = tokio::sync::oneshot::channel();
            *self.held.borrow_mut() = Some(answering);
            return Box::pin(async move { answered.await.map_err(|_| "never answered".into()) });
        }
        let answer = self.answers.borrow_mut().remove(0);
        done(Ok(answer))
    }
}

fn answer(status: u16, body: serde_json::Value) -> Response {
    Response {
        status,
        body: body.to_string().into_bytes(),
    }
}

/// The browser: what it was sent to, and where Google sends it back,
/// asking for an icon on the way, as a browser does.
fn browser(say: &'static str) -> Opener {
    Box::new(move |url: &str| {
        let (at, asked) = url.split_once('?').expect("a query");
        assert_eq!(at, AUTH_AT);
        let asked: BTreeMap<String, String> = parse_query(asked).into_iter().collect();
        assert_eq!(asked["scope"], SCOPE);
        assert_eq!(asked["response_type"], "code");
        assert_eq!(asked["code_challenge_method"], "S256");
        assert_eq!(asked["access_type"], "offline");
        let back = asked["redirect_uri"]
            .strip_prefix("http://")
            .expect("loopback")
            .to_string();
        assert!(back.starts_with("127.0.0.1:"), "{back}");
        let state = asked["state"].clone();
        assert_eq!(asked["code_challenge"].len(), 43);
        // One that never comes back, as a user who closes the tab.
        if say == "nothing" {
            return Ok(());
        }
        std::thread::spawn(move || {
            let get = |target: &str| {
                let mut stream = TcpStream::connect(&back).expect("listening");
                write!(stream, "GET {target} HTTP/1.1\r\nHost: {back}\r\n\r\n").expect("asked");
                let mut page = String::new();
                stream.read_to_string(&mut page).expect("answered");
                page
            };
            assert!(get("/favicon.ico").starts_with("HTTP/1.1 404"));
            let page = match say {
                "yes" => get(&format!(
                    "/?state={state}&code=the-code&scope={}",
                    crate::sync::text::percent(SCOPE)
                )),
                _ => get(&format!("/?error=access_denied&state={state}")),
            };
            assert!(page.contains("close this tab"), "{page}");
        });
        Ok(())
    })
}

fn client() -> Client {
    Client {
        id: "client".into(),
        secret: "not secret".into(),
    }
}

fn block_on<T>(pending: Pending<T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime")
        .block_on(pending)
}

/// `pending`, polled once: under way, as far as it goes without waiting.
fn poll_once<T>(pending: &mut Pending<T>) -> std::task::Poll<T> {
    let waker = std::task::Waker::noop();
    pending
        .as_mut()
        .poll(&mut std::task::Context::from_waker(waker))
}

/// Signed in through the browser, the code is traded with its verifier
/// for a token and a refresh token, which is kept; and the token runs out
/// into a fresh one, traded for the refresh token with no one asked.
#[test]
fn a_sign_in_through_the_browser_keeps_a_refresh_token() {
    let endpoint = Rc::new(Endpoint::default());
    endpoint.answers.borrow_mut().extend([
        answer(200, serde_json::json!({ "access_token": "first", "expires_in": 3599, "refresh_token": "refresh", "scope": SCOPE })),
        answer(200, serde_json::json!({ "access_token": "second", "expires_in": 3599 })),
        answer(400, serde_json::json!({ "error": "invalid_grant" })),
    ]);
    let auth = LoopbackAuth::with(Rc::new(Rc::clone(&endpoint)), client(), browser("yes"));
    assert_eq!(
        block_on(auth.token(false)),
        Err(AuthError::NeedsUser),
        "never signed in"
    );
    block_on(auth.sign_in()).expect("signed in");
    let traded = endpoint.asked.borrow()[0].clone();
    assert_eq!(traded["grant_type"], "authorization_code");
    assert_eq!(traded["code"], "the-code");
    assert_eq!(traded["client_secret"], "not secret");
    assert!(traded["redirect_uri"].starts_with("http://127.0.0.1:"));
    assert_eq!(traded["code_verifier"].len(), 43);
    assert_eq!(auth.kept().as_deref(), Some("refresh"));
    assert_eq!(block_on(auth.token(false)).as_deref(), Ok("first"));

    assert_eq!(block_on(auth.token(true)).as_deref(), Ok("second"));
    let refreshed = endpoint.asked.borrow()[1].clone();
    assert_eq!(refreshed["grant_type"], "refresh_token");
    assert_eq!(refreshed["refresh_token"], "refresh");
    assert_eq!(auth.kept().as_deref(), Some("refresh"), "kept as it was");

    // Taken back in the Google account: only the user can sign in again.
    assert_eq!(block_on(auth.token(true)), Err(AuthError::NeedsUser));
    assert_eq!(auth.kept(), None);
}

/// A refresh token kept from before signs in as the app opens, with no
/// one asked; and forgotten, it is gone.
#[test]
fn a_kept_refresh_token_signs_in_again() {
    let endpoint = Rc::new(Endpoint::default());
    endpoint.answers.borrow_mut().push(answer(
        200,
        serde_json::json!({ "access_token": "again", "expires_in": 3599 }),
    ));
    let auth = LoopbackAuth::with(
        Rc::new(Rc::clone(&endpoint)),
        client(),
        Box::new(|_| panic!("no browser")),
    );
    auth.keep(Some("refresh".into()));
    assert_eq!(block_on(auth.token(false)).as_deref(), Ok("again"));
    assert_eq!(block_on(auth.token(false)).as_deref(), Ok("again"), "held");
    auth.forget();
    assert_eq!(auth.kept(), None);
    assert_eq!(block_on(auth.token(false)), Err(AuthError::NeedsUser));
}

/// Turned down in the browser, or stopped from the app, nothing is
/// signed in.
#[test]
fn a_sign_in_turned_down_signs_nothing_in() {
    let endpoint = Rc::new(Endpoint::default());
    let auth = LoopbackAuth::with(Rc::new(Rc::clone(&endpoint)), client(), browser("no"));
    assert_eq!(block_on(auth.sign_in()), Err(AuthError::Cancelled));
    assert!(endpoint.asked.borrow().is_empty());

    let auth = LoopbackAuth::with(Rc::new(Rc::clone(&endpoint)), client(), browser("nothing"));
    let signing_in = auth.sign_in();
    auth.cancel();
    assert_eq!(block_on(signing_in), Err(AuthError::Cancelled));

    let auth = LoopbackAuth::with(
        Rc::new(endpoint),
        client(),
        Box::new(|_| Err("no browser".into())),
    );
    assert_eq!(
        block_on(auth.sign_in()),
        Err(AuthError::Failed("no browser".into()))
    );
}

/// Access given without Dust's folder ticked is no sign-in.
#[test]
fn access_without_the_folder_is_no_sign_in() {
    let endpoint = Rc::new(Endpoint::default());
    endpoint.answers.borrow_mut().push(answer(
        200,
        serde_json::json!({ "access_token": "t", "expires_in": 3599, "refresh_token": "r", "scope": "openid" }),
    ));
    let auth = LoopbackAuth::with(Rc::new(endpoint), client(), browser("yes"));
    let failed = block_on(auth.sign_in()).expect_err("no folder");
    assert!(failed.told().contains("tick it"), "{failed:?}");
    assert_eq!(auth.kept(), None);
}

/// A trade still under way as the sign-in is forgotten, as sync is turned
/// off, or as another sign-in begins, keeps nothing of what it is answered,
/// and hands nothing on: the account signed in since keeps its own token
/// and refresh token. It used to hold the token it was answered, the first
/// account's, for the next request to take to the first account's drive,
/// or, refused, to clear the refresh token of the account signed in since.
#[test]
fn a_trade_answered_after_another_sign_in_keeps_nothing() {
    for forgotten in [true, false] {
        for refused in [false, true] {
            let endpoint = Rc::new(Endpoint::default());
            let auth = LoopbackAuth::with(Rc::new(Rc::clone(&endpoint)), client(), browser("yes"));
            auth.keep(Some("first refresh".into()));
            endpoint.hold.set(true);
            let mut trading = auth.token(false);
            assert!(poll_once(&mut trading).is_pending(), "under way");
            if forgotten {
                auth.forget();
            }
            endpoint.answers.borrow_mut().push(answer(
                200,
                serde_json::json!({ "access_token": "second", "expires_in": 3599, "refresh_token": "second refresh", "scope": SCOPE }),
            ));
            block_on(auth.sign_in()).expect("signed in");

            let first = if refused {
                answer(400, serde_json::json!({ "error": "invalid_grant" }))
            } else {
                answer(
                    200,
                    serde_json::json!({ "access_token": "first", "expires_in": 3599 }),
                )
            };
            let held = endpoint.held.borrow_mut().take().expect("held");
            held.send(first).expect("answered");
            let traded = block_on(trading);
            let case = format!("forgotten {forgotten}, refused {refused}");
            assert_eq!(auth.kept().as_deref(), Some("second refresh"), "{case}");
            assert_eq!(
                block_on(auth.token(false)).as_deref(),
                Ok("second"),
                "{case}"
            );
            assert_eq!(traded, Err(AuthError::Cancelled), "{case}");
        }
    }
}

/// A browser is answered only once its whole request is read, however long
/// its headers run, as a browser's cookies for 127.0.0.1 can. Answered at
/// its first line and closed with the rest unread, the connection was reset
/// under the browser, which then showed no page; and the browser of these
/// tests, whose requests go out in pieces, now and then saw its icon's
/// answer cut off so, and never came back with the code, leaving the
/// sign-in to wait out its ten minutes.
#[test]
fn a_browser_is_answered_once_its_whole_request_is_read() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listening");
    let at = listener.local_addr().expect("an address");
    let browser = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(at).expect("connected");
        let cookie = "c".repeat(12 * 1024);
        write!(
            stream,
            "GET /favicon.ico HTTP/1.1\r\nHost: {at}\r\nCookie: {cookie}\r\n\r\n"
        )?;
        let mut page = String::new();
        stream.read_to_string(&mut page).map(|_| page)
    });
    let (stream, _) = listener.accept().expect("accepted");
    assert_eq!(
        answer_browser(stream, "the state"),
        None,
        "an icon is no answer"
    );
    let page = browser.join().expect("the browser");
    assert!(
        page.as_ref()
            .is_ok_and(|page| page.starts_with("HTTP/1.1 404")),
        "answered whole: {page:?}"
    );
}

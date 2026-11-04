//! The computer's sign-in against a browser of a test's own, which goes
//! where Google would send it, and a token endpoint of its own.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::rc::Rc;

use super::*;
use crate::persist::record::done;
use crate::sync::http::Response;

/// Google's token endpoint, answering each trade as a test says.
#[derive(Default)]
struct Endpoint {
    answers: RefCell<Vec<Response>>,
    asked: RefCell<Vec<BTreeMap<String, String>>>,
}

impl Http for Rc<Endpoint> {
    fn send(&self, request: Request) -> Pending<Result<Response, String>> {
        assert_eq!((request.method, request.url.as_str()), ("POST", TOKEN_AT));
        let form = String::from_utf8(request.body).expect("a form");
        self.asked
            .borrow_mut()
            .push(parse_query(&form).into_iter().collect());
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

//! HTTPS, as sync asks Google over it ([`Http`]): the browser's own fetch in
//! a browser, and elsewhere a request on a thread of its own, so that the
//! app's thread never waits on the network.

use std::rc::Rc;

use crate::persist::record::Pending;

/// A request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn new(method: &'static str, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_string(), value.into()));
        self
    }

    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    /// The value of the header `name`, if it has one.
    #[cfg(test)]
    pub fn header_of(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(held, _)| held.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// What came back: its status, whatever it is, and its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// What sends a request: in words, why it went unanswered, offline say.
pub trait Http {
    fn send(&self, request: Request) -> Pending<Result<Response, String>>;
}

/// How this build sends requests.
pub fn default_http() -> Rc<dyn Http> {
    #[cfg(feature = "web")]
    {
        Rc::new(web::Fetch)
    }
    #[cfg(not(feature = "web"))]
    {
        Rc::new(native::OnThread::default())
    }
}

#[cfg(feature = "web")]
mod web {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    use super::*;

    /// The browser's fetch.
    pub struct Fetch;

    impl Http for Fetch {
        fn send(&self, request: Request) -> Pending<Result<Response, String>> {
            Box::pin(async move { fetch(request).await.map_err(|err| said(&err)) })
        }
    }

    /// What the browser said of a request it would not make.
    fn said(err: &wasm_bindgen::JsValue) -> String {
        err.dyn_ref::<js_sys::Error>()
            .map(|err| String::from(err.message()))
            .filter(|message| !message.is_empty())
            .map_or_else(
                || "this browser could not reach Google".into(),
                |message| format!("this browser could not reach Google ({message})"),
            )
    }

    async fn fetch(request: Request) -> Result<Response, wasm_bindgen::JsValue> {
        let window = web_sys::window().ok_or("no window")?;
        let init = web_sys::RequestInit::new();
        init.set_method(request.method);
        let headers = web_sys::Headers::new()?;
        for (name, value) in &request.headers {
            headers.set(name, value)?;
        }
        init.set_headers(&headers);
        if !request.body.is_empty() {
            let body = js_sys::Uint8Array::from(request.body.as_slice());
            init.set_body(&body);
        }
        let asked = web_sys::Request::new_with_str_and_init(&request.url, &init)?;
        let answered: web_sys::Response = JsFuture::from(window.fetch_with_request(&asked))
            .await?
            .dyn_into()?;
        let buffer = JsFuture::from(answered.array_buffer()?).await?;
        Ok(Response {
            status: answered.status(),
            body: js_sys::Uint8Array::new(&buffer).to_vec(),
        })
    }
}

#[cfg(not(feature = "web"))]
mod native {
    use std::time::Duration;

    use super::*;

    /// The most a response is read to: more than any file sync writes.
    const RESPONSE_MAX: u64 = 64 * 1024 * 1024;

    /// Each request made on a thread of its own, through one agent, which
    /// keeps its connections to Google open between them.
    pub struct OnThread {
        agent: ureq::Agent,
    }

    impl Default for OnThread {
        fn default() -> Self {
            let config = ureq::Agent::config_builder()
                // Every status is an answer, for the caller to read.
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(120)))
                .build();
            Self {
                agent: ureq::Agent::new_with_config(config),
            }
        }
    }

    impl Http for OnThread {
        fn send(&self, request: Request) -> Pending<Result<Response, String>> {
            let agent = self.agent.clone();
            let (answer, answered) = tokio::sync::oneshot::channel();
            let spawned = std::thread::Builder::new()
                .name("dust-sync-http".into())
                .spawn(move || {
                    let _ = answer.send(send(&agent, request));
                });
            Box::pin(async move {
                spawned.map_err(|err| format!("this device could not reach Google ({err})"))?;
                answered
                    .await
                    .map_err(|_| "this device could not reach Google".to_string())?
            })
        }
    }

    fn send(agent: &ureq::Agent, request: Request) -> Result<Response, String> {
        let Request {
            method,
            url,
            headers,
            body,
        } = request;
        let mut asked = ureq::http::Request::builder().method(method).uri(&url);
        for (name, value) in &headers {
            asked = asked.header(name.as_str(), value.as_str());
        }
        let answered = if body.is_empty() {
            asked.body(()).map(|asked| agent.run(asked))
        } else {
            asked.body(body).map(|asked| agent.run(asked))
        }
        .map_err(|err| err.to_string())?;
        let mut answered = answered.map_err(|err| match err {
            ureq::Error::Io(_) | ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => {
                "this device could not reach Google: is it online?".to_string()
            }
            ureq::Error::Timeout(_) => "Google took too long to answer".to_string(),
            err => format!("this device could not reach Google ({err})"),
        })?;
        let status = answered.status().as_u16();
        let body = answered
            .body_mut()
            .with_config()
            .limit(RESPONSE_MAX)
            .read_to_vec()
            .map_err(|err| format!("Google's answer could not be read ({err})"))?;
        Ok(Response { status, body })
    }
}

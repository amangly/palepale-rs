#![allow(clippy::disallowed_methods)]

use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    Get,
    Post,
    Delete,
}

pub struct Request<'a> {
    pub method: Method,
    pub url: String,
    pub bearer: Option<String>,
    pub headers: &'a [(&'a str, &'a str)],
    pub json: Option<&'a serde_json::Value>,
    pub trial_home: Option<&'a std::path::Path>,
    /// Whole-request bound for `send`; for `open` it bounds only the wait for headers.
    pub timeout: Duration,
}

/// What a response says before its body.
pub struct Head {
    pub status: u16,
    pub request_id: Option<String>,
    pub retry_after: Option<Duration>,
}

pub struct Response {
    pub head: Head,
    pub body: Vec<u8>,
}

/// A response whose body is still arriving.
pub struct Streaming {
    pub head: Head,
    pub body: BoxStream<'static, Result<Vec<u8>, HttpError>>,
}

impl Streaming {
    /// The body as far as it arrives within `timeout` and `max_bytes`; a read that fails or
    /// stalls ends it there, keeping what came.
    pub async fn collect(mut self, max_bytes: usize, timeout: Duration) -> Response {
        let mut body = Vec::new();
        let read = async {
            while body.len() < max_bytes
                && let Some(Ok(chunk)) = self.body.next().await
            {
                body.extend(chunk);
            }
        };
        let _ = tokio::time::timeout(timeout, read).await;
        body.truncate(max_bytes);
        Response {
            head: self.head,
            body,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("timed out")]
    Timeout,
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("the connection opened, but the response body could not be read completely")]
    ResponseBody,
    #[error("{0}")]
    Other(String),
}

#[derive(Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    pub fn new(connect_timeout: Duration) -> Result<Self, HttpError> {
        let client = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .user_agent(concat!("palepale/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| HttpError::Other(e.to_string()))?;
        Ok(Self { client })
    }

    pub async fn send(&self, req: Request<'_>) -> Result<Response, HttpError> {
        let resp = self
            .builder(&req)?
            .timeout(req.timeout)
            .send()
            .await
            .map_err(classify)?;
        let head = Head::of(&resp);
        let body = resp.bytes().await.map_err(classify)?.to_vec();
        Ok(Response { head, body })
    }

    /// Sends and returns as soon as headers arrive; the caller bounds each read of the body.
    pub async fn open(&self, req: Request<'_>) -> Result<Streaming, HttpError> {
        let resp = tokio::time::timeout(req.timeout, self.builder(&req)?.send())
            .await
            .map_err(|_| HttpError::Timeout)?
            .map_err(classify)?;
        Ok(Streaming {
            head: Head::of(&resp),
            body: resp
                .bytes_stream()
                .map(|chunk| chunk.map(|b| b.to_vec()).map_err(classify))
                .boxed(),
        })
    }

    fn builder(&self, req: &Request<'_>) -> Result<reqwest::RequestBuilder, HttpError> {
        let mut builder = match req.method {
            Method::Get => self.client.get(&req.url),
            Method::Post => self.client.post(&req.url),
            Method::Delete => self.client.delete(&req.url),
        };
        if let Some(key) = &req.bearer {
            builder = builder.bearer_auth(key);
        }
        for (name, value) in req.headers {
            builder = builder.header(*name, *value);
        }
        if let Some(home) = req.trial_home {
            let body = serde_json::to_vec(req.json.unwrap_or(&serde_json::Value::Null))
                .map_err(|_| HttpError::Other("Invalid request body".into()))?;
            let url = url::Url::parse(&req.url)
                .map_err(|_| HttpError::Other("Invalid API URL".into()))?;
            let method = match req.method {
                Method::Get => "GET",
                Method::Post => "POST",
                Method::Delete => "DELETE",
            };
            let idempotency = req
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("idempotency-key"))
                .map_or("", |(_, value)| *value);
            let proof = super::trial::proof(
                home,
                &url,
                req.bearer.as_deref().unwrap_or(""),
                method,
                idempotency,
                &body,
            )
            .map_err(|e| HttpError::Other(e.to_string()))?;
            builder = builder
                .header(super::trial::header(), proof)
                .header("content-type", "application/json")
                .body(body);
        } else if let Some(body) = req.json {
            builder = builder.json(body);
        }
        Ok(builder)
    }
}

impl Head {
    fn of(resp: &reqwest::Response) -> Self {
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        Self {
            status: resp.status().as_u16(),
            request_id: header("x-request-id"),
            // CEILING: only the delta-seconds form; an HTTP-date Retry-After is ignored.
            retry_after: header("retry-after")
                .and_then(|v| v.trim().parse().ok())
                .map(Duration::from_secs),
        }
    }
}

/// The URL is dropped from the message: a pairing poll's URL carries the live device code.
fn classify(e: reqwest::Error) -> HttpError {
    let e = e.without_url();
    if e.is_connect() {
        use std::error::Error;
        let mut cause: &dyn Error = &e;
        while let Some(source) = cause.source() {
            cause = source;
        }
        HttpError::Connect(cause.to_string())
    } else if e.is_timeout() {
        HttpError::Timeout
    } else if e.is_body() || e.is_decode() {
        HttpError::ResponseBody
    } else {
        HttpError::Other(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_truncated_response_is_not_a_connection_failure() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000\r\nX-Request-Id: body-fixture\r\nConnection: close\r\n\r\n: hello\n\n").await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let http = Http::new(Duration::from_secs(2)).unwrap();
        let mut response = http
            .open(Request {
                method: Method::Get,
                url: format!("http://{address}/fixture/PRIVATE_MARKER"),
                bearer: None,
                headers: &[],
                json: None,
                trial_home: None,
                timeout: Duration::from_secs(2),
            })
            .await
            .unwrap();
        assert_eq!(response.head.request_id.as_deref(), Some("body-fixture"));
        let failure = loop {
            match response.body.next().await {
                Some(Ok(_)) => {}
                Some(Err(error)) => break error,
                None => panic!("truncation was accepted as a complete response"),
            }
        };
        assert!(!failure.to_string().contains("PRIVATE_MARKER"));
        let error = crate::api::error::ApiError::from(failure);
        assert_eq!(error.info.kind, "response_body");
        assert!(
            !error
                .info
                .entry()
                .hint
                .as_deref()
                .unwrap_or("")
                .contains("PALEPALE_API_URL")
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_error_never_repeats_the_url() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let http = Http::new(Duration::from_secs(2)).unwrap();
        let sent = http
            .send(Request {
                method: Method::Get,
                url: format!("http://127.0.0.1:{closed}/api/pair/DEVICECODE"),
                bearer: None,
                headers: &[],
                json: None,
                trial_home: None,
                timeout: Duration::from_secs(2),
            })
            .await;
        let Err(err) = sent else {
            panic!("a closed port answered");
        };
        assert!(!err.to_string().contains("DEVICECODE"), "{err}");
    }
}

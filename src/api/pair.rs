//! Device pairing: the machine asks for a code, a signed-in person approves it on the web,
//! and the machine collects its own revocable key.

use std::time::Duration;

use serde::Deserialize;

use crate::api::error::{ApiError, ErrorInfo};
use crate::api::{Api, Call};
use crate::api::{endpoints, retry};
use crate::{io, limits};

/// No `Debug`: it holds the device code.
#[derive(Deserialize)]
pub struct Started {
    pub device_code: String,
    pub user_code: String,
    /// Seconds the code stays valid.
    pub ttl: u64,
    pub interval: u64,
}

pub enum Poll {
    Pending,
    Ready(String),
}

#[derive(Deserialize)]
struct Ready {
    api_key: String,
}

/// Asks for a code under this machine's name, which the key list shows once it is approved.
pub async fn start(api: &Api) -> Result<Started, ApiError> {
    let body = serde_json::json!({ "hostname": io::proc::hostname() });
    let resp = api
        .call(
            "pair_start",
            Call {
                body: Some(&body),
                ..Call::default()
            },
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    let started: Started = serde_json::from_slice(&resp.body).map_err(|e| unreadable(&e))?;
    if !(1..=900).contains(&started.ttl) || !(1..=30).contains(&started.interval) {
        return Err(ErrorInfo::local(
            "bad_response",
            "invalid pairing lifetime or polling interval",
        )
        .into());
    }
    Ok(started)
}

pub async fn poll(api: &Api, device_code: &str) -> Result<Poll, ApiError> {
    let body = serde_json::json!({ "device_code": device_code });
    let resp = api
        .call(
            "pair_poll",
            Call {
                body: Some(&body),
                ..Call::default()
            },
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    if resp.head.status == 202 {
        return Ok(Poll::Pending);
    }
    let ready: Ready = serde_json::from_slice(&resp.body).map_err(|e| unreadable(&e))?;
    Ok(Poll::Ready(ready.api_key))
}

/// Polls until the code is approved, backing off on any error worth retrying, until it expires.
pub async fn wait(api: &Api, started: &Started) -> Result<String, ApiError> {
    let limits = &limits::get().pair;
    let collect = async {
        let mut attempt = 1;
        loop {
            match poll(api, &started.device_code).await {
                Ok(Poll::Ready(key)) => return Ok(key),
                Ok(Poll::Pending) => {
                    attempt = 1;
                    io::clock::sleep(
                        Duration::from_secs(started.interval).max(limits.poll_interval_ms.ms()),
                    )
                    .await;
                }
                Err(e) if retry::retryable(&e.info) => {
                    let Some(delay) = retry::delay(&e.info, e.retry_after, attempt) else {
                        return Err(e);
                    };
                    attempt += 1;
                    io::clock::sleep(delay.max(limits.backoff_ms.ms())).await;
                }
                Err(e) => return Err(e),
            }
        }
    };
    io::clock::timeout(Duration::from_secs(started.ttl), collect)
        .await
        .unwrap_or_else(
            |_| Err(ErrorInfo::local("pair_expired", "the pairing code expired").into()),
        )
}

/// Where a signed-in person approves the code.
pub fn page(user_code: &str) -> String {
    let page = endpoints::url(endpoints::get("pair_page"), &[]);
    let Ok(mut url) = url::Url::parse(&page) else {
        return page;
    };
    // Only the approval code enters the browser; the credential-release code stays here.
    url.query_pairs_mut().append_pair("code", user_code);
    url.to_string()
}

fn unreadable(e: &serde_json::Error) -> ApiError {
    crate::api::error::ErrorInfo::local("bad_response", e.to_string()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_link_carries_only_the_approval_code_and_encodes_it() {
        let url = url::Url::parse(&page("012345&device_code=secret")).unwrap();
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "code");
        assert_eq!(pairs[0].1, "012345&device_code=secret");
    }
}

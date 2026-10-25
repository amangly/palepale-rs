//! The signed-in account: balance, pass and spend.

use serde::Deserialize;

use crate::api::error::{ApiError, ErrorInfo};
use crate::api::{Api, Call};
use crate::limits;

#[derive(Debug, Default, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub account_hint: String,
    #[serde(default, deserialize_with = "crate::api::decimal::optional_integer")]
    pub available_microdollars: Option<i64>,
    #[serde(deserialize_with = "crate::api::decimal::integer")]
    pub balance_microdollars: i64,
    #[serde(default)]
    pub can_spend: bool,
    #[serde(default, deserialize_with = "crate::api::decimal::optional_integer")]
    pub reserved_microdollars: Option<i64>,
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub used_tokens: Option<u64>,
    #[serde(default)]
    pub reserved_tokens: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Overview {
    #[serde(deserialize_with = "crate::api::decimal::integer")]
    pub confirmed_requests: u64,
    #[serde(deserialize_with = "crate::api::decimal::integer")]
    pub unknown_requests: u64,
    #[serde(deserialize_with = "crate::api::decimal::integer")]
    pub spent_microdollars: i64,
    #[serde(deserialize_with = "crate::api::decimal::integer")]
    pub prompt_tokens: u64,
    #[serde(deserialize_with = "crate::api::decimal::integer")]
    pub completion_tokens: u64,
    #[serde(deserialize_with = "crate::api::decimal::integer")]
    pub cached_tokens: u64,
    #[serde(deserialize_with = "crate::api::decimal::integer")]
    pub reasoning_tokens: u64,
}

pub async fn overview(api: &Api) -> Result<Overview, ApiError> {
    let response = api
        .call(
            "overview",
            Call::default(),
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    serde_json::from_slice(&response.body)
        .map_err(|e| ErrorInfo::local("bad_response", e.to_string()).into())
}

#[derive(Debug, Deserialize)]
pub struct Key {
    #[serde(rename = "id")]
    pub reference: String,
    #[serde(rename = "label")]
    pub name: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub revoked: bool,
}

pub async fn keys(api: &Api) -> Result<Vec<Key>, ApiError> {
    #[derive(Deserialize)]
    struct Response {
        data: Vec<Key>,
    }
    let response = api
        .call(
            "keys",
            Call::default(),
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    serde_json::from_slice::<Response>(&response.body)
        .map(|response| response.data)
        .map_err(|e| ErrorInfo::local("bad_response", e.to_string()).into())
}

pub async fn revoke_key(api: &Api, reference: &str) -> Result<(), ApiError> {
    let response = api
        .call(
            "revoke_key",
            Call {
                args: &[("ref", reference)],
                ..Call::default()
            },
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    match response.head.status {
        204 => Ok(()),
        _ => Err(ErrorInfo::local("bad_response", "Key revocation was not acknowledged").into()),
    }
}

pub async fn create_key(api: &Api, label: &str) -> Result<String, ApiError> {
    let body = serde_json::json!({"label": label});
    let response = api
        .call(
            "create_key",
            Call {
                body: Some(&body),
                ..Call::default()
            },
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    #[derive(Deserialize)]
    struct Issued {
        key: String,
    }
    let issued: Issued = serde_json::from_slice(&response.body).map_err(|_| {
        ErrorInfo::local("bad_response", "Key creation returned an invalid response")
    })?;
    if !crate::auth::is_inference(&issued.key) {
        return Err(ErrorInfo::local(
            "bad_response",
            "Key creation did not return an inference key",
        )
        .into());
    }
    Ok(issued.key)
}

impl Me {
    pub fn available(&self) -> i64 {
        self.available_microdollars
            .unwrap_or(self.balance_microdollars)
    }
}

pub async fn me(api: &Api) -> Result<Me, ApiError> {
    let resp = api
        .call(
            "me",
            Call::default(),
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    serde_json::from_slice(&resp.body)
        .map_err(|e| ErrorInfo::local("bad_response", e.to_string()).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bun_wallet_and_usage_keep_decimal_integer_precision() {
        let me: Me = serde_json::from_value(serde_json::json!({
            "account_hint": "fixture", "balance_microdollars": "9007199254740993",
            "available_microdollars": "9007199254740991", "reserved_microdollars": "2", "can_spend": true
        })).unwrap();
        assert_eq!(me.balance_microdollars, 9_007_199_254_740_993);
        assert_eq!(me.available(), 9_007_199_254_740_991);
        assert!(serde_json::from_str::<Me>("{}").is_err());
        let totals: Overview = serde_json::from_value(serde_json::json!({
            "confirmed_requests":"1", "unknown_requests":"2", "spent_microdollars":"42",
            "prompt_tokens":"10", "completion_tokens":"3", "cached_tokens":"2", "reasoning_tokens":"1"
        })).unwrap();
        assert_eq!(totals.spent_microdollars, 42);
        assert_eq!(totals.unknown_requests, 2);
    }

    #[test]
    fn account_status_uses_available_funds_after_reservations() {
        let me: Me = serde_json::from_value(serde_json::json!({"account_hint": "1234", "balance_microdollars": 12_300_000, "available_microdollars": 10_000_000, "can_spend": true})).unwrap();
        assert_eq!(me.available(), 10_000_000);
        let legacy: Me = serde_json::from_value(
            serde_json::json!({"balance_microdollars": 12_300_000, "can_spend": true}),
        )
        .unwrap();
        assert_eq!(legacy.available(), 12_300_000);
    }
}

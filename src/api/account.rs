//! The signed-in account: balance, pass and spend.

use serde::Deserialize;

use crate::api::error::{ApiError, ErrorInfo};
use crate::api::{Api, Call};
use crate::limits;

#[derive(Debug, Default, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub account_hint: String,
    #[serde(default)]
    pub available_microdollars: Option<i64>,
    #[serde(default)]
    pub balance_microdollars: i64,
    #[serde(default)]
    pub can_spend: bool,
    #[serde(default)]
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
    pub confirmed_requests: u64,
    pub unknown_requests: u64,
    pub spent_microdollars: i64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
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

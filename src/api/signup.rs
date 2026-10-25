//! Creates an account once. A lost response must not trigger an automatic retry.
use crate::{
    api::{
        Api, Call,
        error::{ApiError, ErrorInfo},
    },
    limits,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Account {
    #[serde(rename = "ownerCredential")]
    pub owner_credential: String,
}

pub async fn register(api: &Api) -> Result<Account, ApiError> {
    let response = api
        .call(
            "signup",
            Call::default(),
            limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    let account: Account = serde_json::from_slice(&response.body).map_err(|_| {
        ErrorInfo::local(
            "bad_response",
            "Account creation returned an invalid response",
        )
    })?;
    if !crate::auth::is_owner(&account.owner_credential) {
        return Err(ErrorInfo::local(
            "bad_response",
            "Account creation did not return an owner credential",
        )
        .into());
    }
    Ok(account)
}

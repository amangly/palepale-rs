//! Public profile names; the server keeps their instructions.

use serde::Deserialize;

use crate::api::error::{ApiError, ErrorInfo};
use crate::api::{Api, Call};
use crate::limits;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize)]
struct Listing {
    data: Vec<Profile>,
}

pub async fn load(api: &Api, model: &str) -> Result<Vec<Profile>, ApiError> {
    let response = api
        .call(
            "agent_profiles",
            Call {
                args: &[("model", model)],
                ..Call::default()
            },
            limits::get().models.fetch_timeout_ms.ms(),
        )
        .await?;
    let listing: Listing = serde_json::from_slice(&response.body).map_err(|_| {
        ErrorInfo::local(
            "bad_profile_catalog",
            &crate::commands::godmode::text().no_profiles,
        )
    })?;
    Ok(listing.data)
}

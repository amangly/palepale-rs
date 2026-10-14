//! Public profile names; the server keeps their instructions.

use serde::Deserialize;
use std::sync::LazyLock;

use crate::api::error::{ApiError, ErrorInfo};
use crate::api::{Api, Call};
use crate::limits;

static SUPPORTED: LazyLock<Vec<String>> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct Config {
        supported: Vec<String>,
    }
    toml::from_str::<Config>(include_str!("../../data/agent_profiles.toml"))
        .expect("data/agent_profiles.toml is checked by tests")
        .supported
});

pub fn supported(id: &str) -> bool {
    SUPPORTED.iter().any(|supported| supported == id)
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct Listing {
    data: Vec<Profile>,
}

pub async fn load(api: &Api) -> Result<Vec<Profile>, ApiError> {
    let response = api
        .call(
            "agent_profiles",
            Call::default(),
            limits::get().models.fetch_timeout_ms.ms(),
        )
        .await?;
    let mut listing: Listing = serde_json::from_slice(&response.body).map_err(|_| {
        ErrorInfo::local(
            "bad_profile_catalog",
            &crate::commands::godmode::text().no_profiles,
        )
    })?;
    listing.data.retain(|profile| supported(&profile.id));
    if listing.data.is_empty() {
        return Err(ErrorInfo::local(
            "empty_profile_catalog",
            &crate::commands::godmode::text().no_profiles,
        )
        .into());
    }
    Ok(listing.data)
}

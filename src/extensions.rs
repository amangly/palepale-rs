use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    User,
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mcp,
    Skills,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

static TEXT: LazyLock<Text> = LazyLock::new(|| {
    toml::from_str(include_str!("../data/extensions.toml")).expect("extension copy parses")
});

pub fn text() -> &'static Text {
    &TEXT
}

pub fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub skills_title: String,
    pub mcp_title: String,
    pub empty_skills: String,
    pub empty_mcp: String,
    pub list_hint: String,
    pub detail_hint: String,
    pub mcp_trust_hint: String,
    pub mcp_review_hint: String,
    pub mcp_local_hint: String,
    pub trust_hint: String,
    pub login_hint: String,
    pub skill_hint: String,
    pub form_hint: String,
    pub search_hint: String,
    pub add_skill: String,
    pub add_name: String,
    pub add_endpoint: String,
    pub add_args: String,
    pub add_bearer: String,
    pub add_client_id: String,
    pub working: String,
    pub project_skill_roots: Vec<String>,
    pub user_skill_roots: Vec<String>,
    pub skills_frame: String,
    pub mcp_frame: String,
    pub skill_frame: String,
    pub skill_error: String,
    pub skill_implicit_only: String,
    pub catalog_cut: String,
    pub mcp_description: String,
    pub untrusted: String,
    pub unreviewed: String,
    pub missing_auth: String,
    pub authentication_required: String,
    pub connection_failed: String,
    pub cancelled: String,
    pub timeout: String,
    pub unsupported: String,
    pub login_ready: String,
    pub login_wait: String,
    pub callback_ok: String,
    pub callback_bad: String,
}

// backend.rs
// Module: Palepale backend connection text
// Build: Rust 1.98.1 / Windows, macOS, Linux

use std::sync::LazyLock;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub disconnected: String,
    pub connected: String,
    pub unavailable: String,
    pub accounts_deferred: String,
    pub releases_unconfigured: String,
}

static TEXT: LazyLock<Text> = LazyLock::new(|| {
    toml::from_str(include_str!("../data/backend.toml")).expect("backend text must parse")
});

pub fn text() -> &'static Text {
    &TEXT
}

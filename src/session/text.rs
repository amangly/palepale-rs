use std::sync::LazyLock;

use serde::Deserialize;

use crate::limits::Limit;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub list_title: String,
    pub empty: String,
    pub picker_hint: String,
    pub resumed: String,
    pub locked: String,
    pub missing: String,
    pub different_project: String,
    pub interrupted: String,
    pub confirmation_required: String,
    pub history_error: String,
    pub max_bytes: Limit<usize>,
    pub list_count: Limit<usize>,
}

pub fn get() -> &'static Text {
    static TEXT: LazyLock<Text> = LazyLock::new(|| {
        toml::from_str(include_str!("../../data/session.toml"))
            .expect("data/session.toml is checked by session tests")
    });
    &TEXT
}

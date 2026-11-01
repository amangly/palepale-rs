//! Explicit model choices survive a new session; command-line flags still win.
use crate::{io, paths::Paths};
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub fn load(paths: &Paths) -> Prefs {
    io::fs::read_string(&paths.prefs())
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn remember_model(paths: &Paths, model: &str) {
    let prefs = Prefs {
        model: Some(model.to_owned()),
    };
    if let Ok(bytes) = serde_json::to_vec(&prefs) {
        // A preference write failure must not interrupt a working conversation.
        let _ = io::fs::write_atomic(&paths.prefs(), &bytes, io::fs::Access::Private);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remembered_model_survives_and_invalid_files_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path().join("home"), dir.path().join("project"));
        assert!(load(&paths).model.is_none());
        remember_model(&paths, "selected-model");
        assert_eq!(load(&paths).model.as_deref(), Some("selected-model"));
        io::fs::write_atomic(&paths.prefs(), b"{broken", io::fs::Access::Private).unwrap();
        assert!(load(&paths).model.is_none());
    }
}

use super::fs::{self, Access};
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use std::path::Path;

use crate::limits;

pub fn install(
    root: &Path,
    catalog: &[u8],
    plugin: &[u8],
    key: &str,
    api_origin: &str,
) -> anyhow::Result<()> {
    ensure!(
        key.starts_with("pk_")
            && key.len() == 46
            && key[3..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "An inference key is required"
    );
    ensure!(
        catalog.len() < limits::get().opencode.catalog_bytes.value
            && plugin.len() < limits::get().opencode.plugin_bytes.value,
        "OpenCode setup response is too large"
    );
    let fresh: Value = serde_json::from_slice(catalog).context("Invalid model configuration")?;
    validate_provider(&fresh, api_origin)?;
    let path = if fs::read_string(&root.join("opencode.jsonc"))?.is_some() {
        root.join("opencode.jsonc")
    } else {
        root.join("opencode.json")
    };
    let old = fs::read_string(&path)?;
    let merged = merge(old.as_deref().unwrap_or("{}"), &fresh)?;
    let bytes = serde_json::to_vec_pretty(&merged)?;
    if let Some(old) = old {
        fs::write_new(
            &path.with_extension(format!("palepale-{:016x}.bak", fastrand::u64(..))),
            old.as_bytes(),
            Access::Private,
        )?;
    }
    // The secret is separate from exportable configuration and is never printed.
    fs::write_atomic(&root.join("palepale.key"), key.as_bytes(), Access::Private)?;
    fs::write_atomic(
        &root.join("plugins/palepale-models.js"),
        plugin,
        Access::Private,
    )?;
    fs::write_atomic(&path, &bytes, Access::Private)?;
    Ok(())
}

fn validate_provider(fresh: &Value, api_origin: &str) -> anyhow::Result<()> {
    let provider = fresh["providers"]["palepale"]
        .as_object()
        .context("Missing Palepale provider")?;
    ensure!(
        provider.get("package").and_then(Value::as_str)
            == Some("@opencode/ai/providers/openai-compatible"),
        "Unsupported provider package"
    );
    ensure!(
        provider
            .get("models")
            .and_then(Value::as_object)
            .is_some_and(|m| !m.is_empty()),
        "No compatible Palepale models are available"
    );
    let base = fresh["providers"]["palepale"]["settings"]["baseURL"]
        .as_str()
        .context("Missing Palepale API URL")?;
    let expected = url::Url::parse(&format!("{}/v1", api_origin.trim_end_matches('/')))?;
    let actual = url::Url::parse(base.trim_end_matches('/'))?;
    ensure!(
        actual == expected,
        "OpenCode provider points to a different API; no files were changed"
    );
    Ok(())
}

fn merge(old: &str, fresh: &Value) -> anyhow::Result<Value> {
    let mut config: Value = jsonc_parser::parse_to_serde_value(old, &Default::default())
        .context("OpenCode configuration is invalid; no files were changed")?;
    let obj = config
        .as_object_mut()
        .context("OpenCode config must be an object")?;
    ensure!(
        !obj.contains_key("provider"),
        "OpenCode V1 config detected. Migrate to OpenCode V2 before connecting Palepale; no files were changed"
    );
    let providers = obj
        .entry("providers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("providers must be an object")?;
    providers.insert("palepale".into(), fresh["providers"]["palepale"].clone());
    providers.get_mut("palepale").unwrap()["settings"]["apiKey"] = json!("{file:./palepale.key}");
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Value {
        json!({"providers":{"palepale":{"package":"@opencode/ai/providers/openai-compatible","settings":{"baseURL":"https://api.palepale.tech/v1"},"models":{"fixture":{}}}}})
    }
    #[test]
    fn merges_jsonc_without_dropping_user_configuration() {
        let result = merge(
            "{/* original */\"model\":\"other/model\",\"providers\":{\"other\":{}},}",
            &catalog(),
        )
        .unwrap();
        assert_eq!(result["model"], "other/model");
        assert!(result["providers"].get("other").is_some());
        assert_eq!(
            result["providers"]["palepale"]["settings"]["apiKey"],
            "{file:./palepale.key}"
        );
        assert!(merge("{broken", &catalog()).is_err());
        assert!(merge("{\"provider\":{}}", &catalog()).is_err());
    }
    #[test]
    fn setup_backs_up_original_and_keeps_secrets_separate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("opencode.jsonc");
        fs::write_atomic(
            &path,
            b"{// keep\n\"model\":\"other/model\"}",
            Access::Private,
        )
        .unwrap();
        let key = format!("pk_{}", "x".repeat(43));
        install(
            dir.path(),
            &serde_json::to_vec(&catalog()).unwrap(),
            b"export default {}",
            &key,
            "https://api.palepale.tech",
        )
        .unwrap();
        let saved = fs::read_string(&path).unwrap().unwrap();
        assert!(!saved.contains(&key));
        assert!(saved.contains("other/model"));
        assert_eq!(
            fs::read_string(&dir.path().join("palepale.key"))
                .unwrap()
                .unwrap(),
            key
        );
    }
    #[test]
    fn invalid_or_redirected_catalog_never_writes_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = format!("pk_{}", "x".repeat(43));
        let mut redirected = catalog();
        redirected["providers"]["palepale"]["settings"]["baseURL"] =
            json!("https://other.example/v1");
        for invalid in [json!({"providers":{"palepale":{}}}), redirected] {
            assert!(
                install(
                    dir.path(),
                    &serde_json::to_vec(&invalid).unwrap(),
                    b"plugin",
                    &key,
                    "https://api.palepale.tech"
                )
                .is_err()
            );
            assert!(
                fs::read_string(&dir.path().join("palepale.key"))
                    .unwrap()
                    .is_none()
            );
        }
    }
}

use super::{client_state, clients, fs};
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use std::path::Path;

const KEY: &str = "secrets/providers.palepale.api_key";

pub fn active_home(root: &Path) -> anyhow::Result<std::path::PathBuf> {
    if let Some(profile) = fs::read_string(&root.join("active-profile"))? {
        let name = profile.trim();
        let valid = !name.is_empty()
            && name.len() <= 64
            && name != "default"
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
            && name.as_bytes()[0].is_ascii_alphanumeric();
        let path = root.join("profiles").join(name);
        if valid && path.is_dir() {
            return Ok(path);
        }
    }
    Ok(root.into())
}

fn read_config(root: &Path) -> anyhow::Result<(String, Value)> {
    let raw = fs::read_string(&root.join("config.jsonc"))?.unwrap_or_else(|| "{}".into());
    let config: Value = jsonc_parser::parse_to_serde_value(&raw, &Default::default())
        .context("Invalid NodeCode configuration; no files were changed")?;
    ensure!(
        config.is_object(),
        "NodeCode configuration must be an object"
    );
    Ok((raw, config))
}

pub fn install(
    root: &Path,
    bytes: &[u8],
    key: &str,
    origin: &str,
    selection: (Option<&str>, &str),
) -> anyhow::Result<String> {
    clients::validate_key(key)?;
    let catalog = clients::catalog(bytes, origin)?;
    ensure!(
        catalog["protocols"]
            .as_array()
            .is_some_and(|p| p.iter().any(|p| p == "responses")),
        "The gateway does not support NodeCode's Responses protocol"
    );
    let models = catalog["nodecode"]["models"]
        .as_array()
        .context("Refresh the Palepale gateway to enable NodeCode setup")?;
    ensure!(
        !models.is_empty(),
        "No compatible Palepale models are available"
    );
    for model in models {
        let id = model["id"].as_str().context("Invalid model id")?;
        ensure!(
            !id.is_empty()
                && id.len() <= 96
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
            "Invalid model id"
        );
        ensure!(
            model["context_window"].as_u64().is_some_and(|n| n > 0),
            "Invalid model context window"
        );
    }
    let (selected, preferred) = selection;
    let wanted = models
        .iter()
        .find(|m| m["id"].as_str() == Some(selected.unwrap_or(preferred)));
    ensure!(
        selected.is_none() || wanted.is_some(),
        "This model is not available for NodeCode"
    );
    let model = wanted.unwrap_or(&models[0]);
    let id = model["id"].as_str().context("Invalid model id")?;
    let (raw, before) = read_config(root)?;
    let mut config = before.clone();
    let template: Value = serde_json::from_str(include_str!("../../data/nodecode.json"))?;
    let mut provider = template["provider"].clone();
    provider["base_url"] = json!(format!("{}/v1", origin.trim_end_matches('/')));
    provider["models"] = json!(
        models
            .iter()
            .map(|m| json!({"id":m["id"],"context_window":m["context_window"]}))
            .collect::<Vec<_>>()
    );
    config
        .as_object_mut()
        .unwrap()
        .entry("providers")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("providers must be an object")?
        .insert("palepale".into(), provider);
    let general = config
        .as_object_mut()
        .unwrap()
        .entry("general")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("general must be an object")?;
    let mut pin = json!({"provider":"palepale", "model":id});
    if let Some(effort) = model["reasoning_efforts"]
        .as_array()
        .and_then(|e| e.first())
        .and_then(Value::as_str)
    {
        pin["effort"] = json!(effort);
    }
    general.insert("default_model".into(), pin);
    let failover = config
        .as_object_mut()
        .unwrap()
        .entry("failover")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("failover must be an object")?;
    for (key, value) in template["failover"].as_object().unwrap() {
        failover.insert(key.clone(), value.clone());
    }
    // Native secret files work on Windows too; no shell command carries the key.
    let state = client_state::remember(root, &before, &config)?;
    if fs::read_string(&root.join("palepale-connection.json"))?.is_none() {
        ensure!(
            fs::read_string(&root.join(KEY))?.is_none(),
            "A NodeCode Palepale secret already exists; reconnect it through NodeCode before using this setup"
        );
        fs::write_new(
            &root.join(format!("config.palepale-{:016x}.bak", fastrand::u64(..))),
            raw.as_bytes(),
            fs::Access::Private,
        )?;
    }
    clients::replace_files(&[
        (root.join(KEY), key.as_bytes().to_vec()),
        state,
        (
            root.join("config.jsonc"),
            serde_json::to_vec_pretty(&config)?,
        ),
    ])?;
    Ok(id.into())
}

pub fn disconnect(root: &Path) -> anyhow::Result<()> {
    let (_, current) = read_config(root)?;
    if let Some(restored) = client_state::restore(root, &current)? {
        client_state::save(
            &root.join("config.jsonc"),
            &serde_json::to_vec_pretty(&restored)?,
        )?;
        client_state::forget(root, &[KEY])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ORIGIN: &str = "https://api.example.test";
    fn key() -> String {
        format!("pk_{}", "a".repeat(43))
    }
    fn catalog() -> Vec<u8> {
        serde_json::to_vec(&json!({"api_base_url":ORIGIN,"protocols":["responses"],"nodecode":{"models":[{"id":"fixture","context_window":1000000,"reasoning_efforts":["high"]}]}})).unwrap()
    }
    #[test]
    fn setup_refresh_and_logout_preserve_settings_and_keep_secrets_separate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let original = "{// retained in backup\n\"providers\":{\"other\":{\"sdk\":\"google\"}},\"general\":{\"theme\":\"mine\",\"default_model\":{\"provider\":\"other\",\"model\":\"old\"}},\"cells\":{\"keep\":true}}";
        fs::write_atomic(
            &root.join("config.jsonc"),
            original.as_bytes(),
            fs::Access::Private,
        )
        .unwrap();
        assert_eq!(
            install(root, &catalog(), &key(), ORIGIN, (None, "fixture")).unwrap(),
            "fixture"
        );
        install(root, &catalog(), &key(), ORIGIN, (Some("fixture"), "other")).unwrap();
        let (raw, mut saved) = read_config(root).unwrap();
        assert!(!raw.contains(&key()));
        assert_eq!(saved["providers"]["palepale"]["sdk"], "openai-responses");
        assert_eq!(saved["providers"]["palepale"]["request_max_retries"], 0);
        assert_eq!(saved["providers"]["palepale"]["stream_max_retries"], 0);
        assert_eq!(saved["failover"]["on"], json!([]));
        assert_eq!(saved["failover"]["first_token_seconds"], 0);
        assert_eq!(fs::read_string(&root.join(KEY)).unwrap().unwrap(), key());
        saved["general"]["theme"] = json!("changed");
        client_state::save(
            &root.join("config.jsonc"),
            &serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
        disconnect(root).unwrap();
        disconnect(root).unwrap();
        let (_, restored) = read_config(root).unwrap();
        assert_eq!(restored["general"]["theme"], "changed");
        assert_eq!(restored["general"]["default_model"]["model"], "old");
        assert!(restored["providers"].get("palepale").is_none());
        assert!(restored.get("failover").is_none());
        assert_eq!(restored["providers"]["other"]["sdk"], "google");
        assert!(fs::read_string(&root.join(KEY)).unwrap().is_none());
    }
    #[test]
    fn invalid_origin_model_and_config_leave_files_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(
            install(
                root,
                &catalog(),
                &key(),
                "https://other.test",
                (None, "fixture")
            )
            .is_err()
        );
        assert!(
            install(
                root,
                &catalog(),
                &key(),
                ORIGIN,
                (Some("missing"), "fixture")
            )
            .is_err()
        );
        fs::write_atomic(&root.join("config.jsonc"), b"{broken", fs::Access::Private).unwrap();
        assert!(install(root, &catalog(), &key(), ORIGIN, (None, "fixture")).is_err());
        assert_eq!(
            fs::read_string(&root.join("config.jsonc"))
                .unwrap()
                .unwrap(),
            "{broken"
        );
        assert!(fs::read_string(&root.join(KEY)).unwrap().is_none());
    }
}

use super::fs::{self, Access};
use anyhow::{Context, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::LazyLock,
};

#[derive(Clone, Copy)]
pub enum Kind {
    Codex,
    Claude,
}

#[derive(Deserialize)]
pub struct Config {
    codex_provider: Value,
    codex_reset: Vec<String>,
    claude_env: BTreeMap<String, String>,
    claude_model_keys: Vec<String>,
    pub setup_timeout_secs: u64,
    catalog_bytes: usize,
    auth_timeout_ms: u64,
    auth_refresh_ms: u64,
}
pub fn config() -> &'static Config {
    static CONFIG: LazyLock<Config> = LazyLock::new(|| {
        serde_json::from_str(include_str!("../../data/clients.json")).expect("client configuration")
    });
    &CONFIG
}

fn validate_key(key: &str) -> anyhow::Result<()> {
    ensure!(
        key.starts_with("pk_")
            && key.len() == 46
            && key[3..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "An inference key is required"
    );
    Ok(())
}

pub fn credential(path: &Path) -> anyhow::Result<String> {
    ensure!(path.is_absolute(), "The credential path must be absolute");
    let key = fs::read_string(path)?
        .context("Reconnect this app with Palepale to restore its credential")?;
    validate_key(&key)?;
    Ok(key)
}

pub fn install(
    kind: Kind,
    root: &Path,
    bytes: &[u8],
    key: &str,
    origin: &str,
    selection: (Option<&str>, &str),
) -> anyhow::Result<String> {
    let (selected, preferred) = selection;
    validate_key(key)?;
    ensure!(
        bytes.len() <= config().catalog_bytes,
        "Client catalog is too large"
    );
    let catalog: Value = serde_json::from_slice(bytes).context("Invalid client catalog")?;
    let actual = url::Url::parse(
        catalog["api_base_url"]
            .as_str()
            .context("Missing API origin")?,
    )?;
    let expected = url::Url::parse(origin)?;
    ensure!(
        actual == expected
            && (actual.scheme() == "https"
                || (actual.scheme() == "http"
                    && matches!(actual.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))))
            && actual.username().is_empty()
            && actual.password().is_none()
            && actual.query().is_none()
            && actual.fragment().is_none(),
        "Client catalog points to a different or insecure API; no files were changed"
    );
    let (name, protocol, id_key) = match kind {
        Kind::Codex => ("codex", "responses", "slug"),
        Kind::Claude => ("claude", "messages", "id"),
    };
    ensure!(
        catalog["protocols"]
            .as_array()
            .is_some_and(|a| a.iter().any(|p| p == protocol)),
        "This gateway does not support {name} yet"
    );
    let models = catalog[name]["models"]
        .as_array()
        .context("Missing client models")?;
    ensure!(
        !models.is_empty(),
        "No compatible Palepale models are available"
    );
    let wanted = selected.unwrap_or(preferred);
    let model = models.iter().find(|m| m[id_key].as_str() == Some(wanted));
    ensure!(
        selected.is_none() || model.is_some(),
        "This model is not available for {name}"
    );
    let model = model.unwrap_or(&models[0]);
    let id = model[id_key].as_str().context("Invalid model id")?;
    ensure!(
        !id.is_empty()
            && id.len() <= 96
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "Invalid model id"
    );
    let exe = fs::canonical(&super::proc::current_exe()?);
    let root = fs::canonical(root);
    let key_path = root.join("palepale.key");
    let mut writes = vec![(key_path.clone(), key.as_bytes().to_vec())];
    match kind {
        Kind::Codex => {
            let path = root.join("config.toml");
            let old = fs::read_string(&path)?.unwrap_or_default();
            let mut doc = old
                .parse::<toml_edit::DocumentMut>()
                .context("Invalid Codex configuration; no files were changed")?;
            for key in &config().codex_reset {
                doc.remove(key);
            }
            doc["model_provider"] = toml_edit::value("palepale");
            doc["model"] = toml_edit::value(id);
            doc["web_search"] = toml_edit::value("disabled");
            doc["model_reasoning_summary"] = toml_edit::value("none");
            let catalog_path = root.join("palepale-models.json");
            doc["model_catalog_json"] = toml_edit::value(catalog_path.to_string_lossy().as_ref());
            let mut provider = config().codex_provider.clone();
            provider["base_url"] = json!(format!("{}/v1", origin.trim_end_matches('/')));
            provider["auth"] = json!({"command":exe,"args":["client-key",key_path],"timeout_ms":config().auth_timeout_ms,"refresh_interval_ms":config().auth_refresh_ms});
            let provider: toml::Value = serde_json::from_value(provider)?;
            let table = toml::to_string(&provider)?.parse::<toml_edit::DocumentMut>()?;
            if !doc.contains_key("model_providers") {
                doc["model_providers"] = toml_edit::Item::Table(toml_edit::Table::new());
            }
            let providers = doc["model_providers"]
                .as_table_like_mut()
                .context("model_providers must be a table")?;
            providers.insert("palepale", toml_edit::Item::Table(table.as_table().clone()));
            writes.push((
                catalog_path,
                serde_json::to_vec_pretty(&json!({"models":models}))?,
            ));
            writes.push((path, doc.to_string().into_bytes()));
        }
        Kind::Claude => {
            let path = root.join("settings.json");
            let old = fs::read_string(&path)?.unwrap_or_else(|| "{}".into());
            let mut doc: Value = serde_json::from_str(&old)
                .context("Invalid Claude Code configuration; no files were changed")?;
            let object = doc
                .as_object_mut()
                .context("Claude Code settings must be an object")?;
            let env = object
                .entry("env")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .context("env must be an object")?;
            for (key, value) in &config().claude_env {
                env.insert(key.clone(), json!(value));
            }
            for key in &config().claude_model_keys {
                env.insert(key.clone(), json!(id));
            }
            env.insert(
                "ANTHROPIC_BASE_URL".into(),
                json!(origin.trim_end_matches('/')),
            );
            env.insert(
                "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME".into(),
                json!(model["name"].as_str().unwrap_or(id)),
            );
            let output = model["max_output_tokens"]
                .as_u64()
                .filter(|n| *n > 0)
                .context("Missing model output limit")?;
            env.insert(
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
                json!(output.to_string()),
            );
            let context = model["context_window"]
                .as_u64()
                .filter(|n| *n > output)
                .context("Missing model context limit")?;
            env.insert(
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(),
                json!(context.to_string()),
            );
            object.insert("model".into(), json!(id));
            object.insert("alwaysThinkingEnabled".into(), json!(false));
            object.insert(
                "apiKeyHelper".into(),
                json!(format!(
                    "{} client-key {}",
                    shell_path(&exe)?,
                    shell_path(&key_path)?
                )),
            );
            writes.push((path, serde_json::to_vec_pretty(&doc)?));
        }
    }
    commit_files(&writes)?;
    Ok(id.to_owned())
}

fn shell_path(path: &Path) -> anyhow::Result<String> {
    let value = path.to_str().context("The app path must be UTF-8")?;
    #[cfg(windows)]
    {
        ensure!(
            !value.contains(['\r', '\n', '"', '%', '!', '$', '`']),
            "The app path contains shell expansion characters"
        );
        Ok(format!("\"{}\"", value.replace('\\', "/")))
    }
    #[cfg(not(windows))]
    {
        Ok(format!("'{}'", value.replace('\'', "'\\''")))
    }
}

fn commit_files(writes: &[(PathBuf, Vec<u8>)]) -> anyhow::Result<()> {
    let originals: Vec<_> = writes
        .iter()
        .map(|(p, _)| fs::read_string(p))
        .collect::<Result<_, _>>()?;
    let suffix = format!("palepale-{:016x}.bak", fastrand::u64(..));
    for ((path, bytes), old) in writes.iter().zip(&originals) {
        if let Some(old) = old
            && old.as_bytes() != bytes
        {
            fs::write_new(
                &path.with_extension(&suffix),
                old.as_bytes(),
                Access::Private,
            )?;
        }
    }
    for (index, (path, bytes)) in writes.iter().enumerate() {
        if originals[index]
            .as_ref()
            .is_some_and(|old| old.as_bytes() == bytes)
        {
            continue;
        }
        if let Err(error) = fs::write_atomic(path, bytes, Access::Private) {
            // Restore earlier files if the final configuration cannot be installed.
            for (previous, old) in writes[..index].iter().zip(&originals) {
                match old {
                    Some(old) => fs::write_atomic(&previous.0, old.as_bytes(), Access::Private)?,
                    None => {
                        fs::remove(&previous.0)?;
                    }
                }
            }
            return Err(error.into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Vec<u8> {
        serde_json::to_vec(&json!({"api_base_url":"https://api.palepale.tech","protocols":["responses","messages"],"codex":{"models":[{"slug":"fixture"}]},"claude":{"models":[{"id":"fixture","name":"Fixture","max_output_tokens":1024,"context_window":32768}]}})).unwrap()
    }
    fn key() -> String {
        format!("pk_{}", "x".repeat(43))
    }
    #[test]
    fn codex_preserves_settings_and_auth_and_selects_the_api_provider() {
        let dir = tempfile::tempdir().unwrap();
        let original = "# keep comment\nmodel = 'previous'\napproval_policy = 'on-request'\n[model_providers.other]\nname = 'Other'\n";
        fs::write_atomic(
            &dir.path().join("config.toml"),
            original.as_bytes(),
            Access::Private,
        )
        .unwrap();
        fs::write_atomic(
            &dir.path().join("auth.json"),
            b"existing-subscription",
            Access::Private,
        )
        .unwrap();
        install(
            Kind::Codex,
            dir.path(),
            &catalog(),
            &key(),
            "https://api.palepale.tech",
            (None, "missing"),
        )
        .unwrap();
        let saved = fs::read_string(&dir.path().join("config.toml"))
            .unwrap()
            .unwrap();
        assert!(saved.contains("# keep comment"));
        assert!(!saved.contains(&key()));
        let doc: toml::Value = toml::from_str(&saved).unwrap();
        assert_eq!(doc["model_provider"].as_str(), Some("palepale"));
        assert_eq!(doc["model"].as_str(), Some("fixture"));
        assert_eq!(doc["approval_policy"].as_str(), Some("on-request"));
        assert_eq!(
            doc["model_providers"]["other"]["name"].as_str(),
            Some("Other")
        );
        assert_eq!(
            doc["model_providers"]["palepale"]["auth"]["args"][0].as_str(),
            Some("client-key")
        );
        assert_eq!(credential(&dir.path().join("palepale.key")).unwrap(), key());
        assert_eq!(
            fs::read_string(&dir.path().join("auth.json"))
                .unwrap()
                .unwrap(),
            "existing-subscription"
        );
        install(
            Kind::Codex,
            dir.path(),
            &catalog(),
            &key(),
            "https://api.palepale.tech",
            (None, "fixture"),
        )
        .unwrap();
        assert_eq!(
            saved,
            fs::read_string(&dir.path().join("config.toml"))
                .unwrap()
                .unwrap()
        );
    }
    #[test]
    fn claude_keeps_permissions_and_clears_competing_auth_without_exporting_secrets() {
        let dir = tempfile::tempdir().unwrap();
        fs::write_atomic(&dir.path().join("settings.json"),br#"{"permissions":{"deny":["Bash(rm *)"]},"env":{"KEEP":"yes","ANTHROPIC_AUTH_TOKEN":"old","CLAUDE_CODE_USE_VERTEX":"1"}}"#,Access::Private).unwrap();
        install(
            Kind::Claude,
            dir.path(),
            &catalog(),
            &key(),
            "https://api.palepale.tech",
            (Some("fixture"), "fixture"),
        )
        .unwrap();
        let saved = fs::read_string(&dir.path().join("settings.json"))
            .unwrap()
            .unwrap();
        assert!(!saved.contains(&key()));
        let doc: Value = serde_json::from_str(&saved).unwrap();
        assert_eq!(doc["permissions"]["deny"][0], "Bash(rm *)");
        assert_eq!(doc["env"]["KEEP"], "yes");
        assert_eq!(doc["env"]["ANTHROPIC_AUTH_TOKEN"], "");
        assert_eq!(doc["env"]["CLAUDE_CODE_USE_VERTEX"], "");
        assert_eq!(doc["env"]["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "fixture");
        assert!(doc["apiKeyHelper"].as_str().unwrap().contains("client-key"));
    }
    #[test]
    fn invalid_config_catalog_model_or_owner_key_never_changes_files() {
        for kind in [Kind::Codex, Kind::Claude] {
            let dir = tempfile::tempdir().unwrap();
            let filename = match kind {
                Kind::Codex => "config.toml",
                Kind::Claude => "settings.json",
            };
            fs::write_atomic(&dir.path().join(filename), b"broken{", Access::Private).unwrap();
            assert!(
                install(
                    kind,
                    dir.path(),
                    &catalog(),
                    &key(),
                    "https://api.palepale.tech",
                    (None, "fixture")
                )
                .is_err()
            );
            assert!(
                fs::read_string(&dir.path().join("palepale.key"))
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                fs::read_string(&dir.path().join(filename))
                    .unwrap()
                    .unwrap(),
                "broken{"
            );
            fs::remove(&dir.path().join(filename)).unwrap();
            for (origin, credential, selected) in [
                ("https://other.example", key(), None),
                (
                    "https://api.palepale.tech",
                    format!("po_{}", "x".repeat(43)),
                    None,
                ),
                ("https://api.palepale.tech", key(), Some("absent")),
            ] {
                assert!(
                    install(
                        kind,
                        dir.path(),
                        &catalog(),
                        &credential,
                        origin,
                        (selected, "fixture")
                    )
                    .is_err()
                );
                assert!(
                    fs::read_string(&dir.path().join(filename))
                        .unwrap()
                        .is_none()
                );
                assert!(
                    fs::read_string(&dir.path().join("palepale.key"))
                        .unwrap()
                        .is_none()
                );
            }
        }
    }
}

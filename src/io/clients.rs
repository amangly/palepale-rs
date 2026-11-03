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
    claude_reset: Vec<String>,
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
            let before = toml_json(&old)?;
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
            writes.push(super::client_state::remember(
                &root,
                &prior_settings(kind, &root, &before)?,
                &toml_json(&doc.to_string())?,
            )?);
        }
        Kind::Claude => {
            let path = root.join("settings.json");
            let old = fs::read_string(&path)?.unwrap_or_else(|| "{}".into());
            let mut doc: Value = serde_json::from_str(&old)
                .context("Invalid Claude Code configuration; no files were changed")?;
            let before = doc.clone();
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
            for key in &config().claude_reset {
                env.remove(key);
            }
            for key in &config().claude_model_keys {
                env.insert(key.clone(), json!(id));
            }
            env.insert(
                "ANTHROPIC_BASE_URL".into(),
                json!(origin.trim_end_matches('/')),
            );
            let mut options = Vec::new();
            let mut output = u64::MAX;
            let mut context = u64::MAX;
            let mut ids = std::collections::BTreeSet::new();
            for entry in models {
                let model_id = entry["id"].as_str().context("Invalid model id")?;
                ensure!(
                    !model_id.is_empty()
                        && model_id.len() <= 96
                        && model_id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
                    "Invalid model id"
                );
                let max_output = entry["max_output_tokens"]
                    .as_u64()
                    .filter(|n| *n > 0)
                    .context("Missing model output limit")?;
                let window = entry["context_window"]
                    .as_u64()
                    .filter(|n| *n > max_output)
                    .context("Missing model context limit")?;
                output = output.min(max_output);
                context = context.min(window);
                if ids.insert(model_id) {
                    let label = entry["name"].as_str().unwrap_or(model_id);
                    let duplicate_label = models
                        .iter()
                        .filter(|m| m["name"].as_str() == Some(label))
                        .count()
                        > 1;
                    let mut option = json!({"model":model_id,"label":label});
                    if duplicate_label {
                        option["description"] = json!(model_id);
                    }
                    options.push(option);
                }
            }
            // These Claude settings are session-wide, including after switching models.
            env.insert(
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(),
                json!(output.to_string()),
            );
            env.insert(
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(),
                json!(context.to_string()),
            );
            object.insert("model".into(), json!(id));
            object.insert(
                "modelPicker".into(),
                json!({"replaceBuiltInOptions":true,"options":options}),
            );
            object.insert("availableModels".into(), json!(ids));
            object.insert("alwaysThinkingEnabled".into(), json!(false));
            object.insert(
                "apiKeyHelper".into(),
                json!(format!(
                    "{} client-key {}",
                    shell_path(&exe)?,
                    shell_path(&key_path)?
                )),
            );
            writes.push(super::client_state::remember(
                &root,
                &prior_settings(kind, &root, &before)?,
                &doc,
            )?);
            writes.push((path, serde_json::to_vec_pretty(&doc)?));
        }
    }
    commit_files(&writes)?;
    Ok(id.to_owned())
}

fn toml_json(text: &str) -> anyhow::Result<Value> {
    Ok(serde_json::to_value(toml::from_str::<toml::Value>(text)?)?)
}

fn owns_claude(root: &Path, value: &Value) -> anyhow::Result<bool> {
    let helper = format!(" client-key {}", shell_path(&root.join("palepale.key"))?);
    Ok(value["apiKeyHelper"]
        .as_str()
        .is_some_and(|s| s.ends_with(&helper)))
}

fn legacy_settings(kind: Kind, root: &Path, current: &Value) -> anyhow::Result<Value> {
    let mut restored = current.clone();
    let object = restored
        .as_object_mut()
        .context("Client settings must be an object")?;
    match kind {
        Kind::Codex => {
            if current["model_provider"] == "palepale" {
                for key in [
                    "model_provider",
                    "model",
                    "model_catalog_json",
                    "web_search",
                    "model_reasoning_summary",
                ] {
                    object.remove(key);
                }
            }
            if let Some(providers) = object
                .get_mut("model_providers")
                .and_then(Value::as_object_mut)
            {
                providers.remove("palepale");
                if providers.is_empty() {
                    object.remove("model_providers");
                }
            }
        }
        Kind::Claude if owns_claude(root, current)? => {
            for key in [
                "apiKeyHelper",
                "model",
                "modelPicker",
                "availableModels",
                "alwaysThinkingEnabled",
            ] {
                object.remove(key);
            }
            if let Some(env) = object.get_mut("env").and_then(Value::as_object_mut) {
                for key in config()
                    .claude_env
                    .keys()
                    .chain(&config().claude_model_keys)
                    .chain(&config().claude_reset)
                    .map(String::as_str)
                    .chain([
                        "ANTHROPIC_BASE_URL",
                        "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
                        "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
                    ])
                {
                    env.remove(key);
                }
                if env.is_empty() {
                    object.remove("env");
                }
            }
        }
        Kind::Claude => {}
    }
    Ok(restored)
}

fn prior_settings(kind: Kind, root: &Path, current: &Value) -> anyhow::Result<Value> {
    if super::client_state::restore(root, current)?.is_some() {
        Ok(current.clone())
    } else {
        legacy_settings(kind, root, current)
    }
}

pub fn disconnect(kind: Kind, root: &Path) -> anyhow::Result<()> {
    let root = fs::canonical(root);
    let path = root.join(match kind {
        Kind::Codex => "config.toml",
        Kind::Claude => "settings.json",
    });
    if let Some(old) = fs::read_string(&path)? {
        let current = match kind {
            Kind::Codex => toml_json(&old)?,
            Kind::Claude => serde_json::from_str(&old)?,
        };
        ensure!(current.is_object(), "Client settings must be an object");
        if matches!(kind, Kind::Codex) || owns_claude(&root, &current)? {
            let restored = match super::client_state::restore(&root, &current)? {
                Some(saved) => saved,
                None => legacy_settings(kind, &root, &current)?,
            };
            if restored != current {
                let bytes = match kind {
                    Kind::Codex => {
                        let value: toml::Value = serde_json::from_value(restored)?;
                        let fresh = toml::to_string(&value)?.parse::<toml_edit::DocumentMut>()?;
                        let mut doc = old.parse::<toml_edit::DocumentMut>()?;
                        update_table(doc.as_table_mut(), fresh.as_table());
                        doc.to_string().into_bytes()
                    }
                    Kind::Claude => serde_json::to_vec_pretty(&restored)?,
                };
                super::client_state::save(&path, &bytes)?;
            }
        }
    }
    let artifacts: &[&str] = match kind {
        Kind::Codex => &["palepale.key", "palepale-models.json"],
        Kind::Claude => &["palepale.key"],
    };
    super::client_state::forget(&root, artifacts)
}

fn update_table(current: &mut dyn toml_edit::TableLike, fresh: &dyn toml_edit::TableLike) {
    let keys: Vec<_> = current.iter().map(|(k, _)| k.to_owned()).collect();
    for key in keys {
        if !fresh.contains_key(&key) {
            current.remove(&key);
        }
    }
    for (key, value) in fresh.iter() {
        if let (Some(old), Some(new)) = (
            current
                .get_mut(key)
                .and_then(toml_edit::Item::as_table_like_mut),
            value.as_table_like(),
        ) {
            update_table(old, new);
        } else if current.get(key).map(ToString::to_string) != Some(value.to_string()) {
            if let Some(old) = current.get_mut(key) {
                *old = value.clone();
            } else {
                current.insert(key, value.clone());
            }
        }
    }
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

pub(super) fn commit_files(writes: &[(PathBuf, Vec<u8>)]) -> anyhow::Result<()> {
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
        assert_eq!(doc["modelPicker"]["options"][0]["model"], "fixture");
        assert!(doc["apiKeyHelper"].as_str().unwrap().contains("client-key"));
    }
    #[test]
    fn reconnect_preserves_the_original_settings_and_later_user_edits() {
        for kind in [Kind::Codex, Kind::Claude] {
            let dir = tempfile::tempdir().unwrap();
            let (name, original) = match kind {
                Kind::Codex => (
                    "config.toml",
                    "# existing\nmodel = 'original'\nweb_search = 'cached'\n[model_providers.other]\nname = 'Other'\n",
                ),
                Kind::Claude => (
                    "settings.json",
                    r#"{"model":"original","env":{"ANTHROPIC_AUTH_TOKEN":"fixture-original-token"},"alwaysThinkingEnabled":false}"#,
                ),
            };
            let path = dir.path().join(name);
            fs::write_atomic(&path, original.as_bytes(), Access::Private).unwrap();
            for _ in 0..2 {
                install(
                    kind,
                    dir.path(),
                    &catalog(),
                    &key(),
                    "https://api.palepale.tech",
                    (None, "fixture"),
                )
                .unwrap();
            }
            let connected = fs::read_string(&path).unwrap().unwrap();
            let edited = match kind {
                Kind::Codex => format!("approval_policy = 'never'\n{connected}"),
                Kind::Claude => {
                    let mut doc: Value = serde_json::from_str(&connected).unwrap();
                    doc["permissions"] = json!({"deny":["Bash(rm *)"]});
                    serde_json::to_string(&doc).unwrap()
                }
            };
            fs::write_atomic(&path, edited.as_bytes(), Access::Private).unwrap();
            disconnect(kind, dir.path()).unwrap();
            let saved = fs::read_string(&path).unwrap().unwrap();
            match kind {
                Kind::Codex => {
                    let doc = toml_json(&saved).unwrap();
                    assert_eq!(doc["model"], "original");
                    assert_eq!(doc["approval_policy"], "never");
                    assert_eq!(doc["web_search"], "cached");
                    assert_eq!(doc["model_providers"]["other"]["name"], "Other");
                    assert!(saved.contains("# existing"));
                }
                Kind::Claude => {
                    let doc: Value = serde_json::from_str(&saved).unwrap();
                    assert_eq!(doc["model"], "original");
                    assert_eq!(doc["env"]["ANTHROPIC_AUTH_TOKEN"], "fixture-original-token");
                    assert_eq!(doc["alwaysThinkingEnabled"], false);
                    assert_eq!(doc["permissions"]["deny"][0], "Bash(rm *)");
                }
            }
            assert!(!dir.path().join("palepale.key").exists());
            disconnect(kind, dir.path()).unwrap();
            assert_eq!(saved, fs::read_string(&path).unwrap().unwrap());
        }
    }

    #[test]
    fn disconnect_removes_old_installations_even_after_reconnecting() {
        for kind in [Kind::Codex, Kind::Claude] {
            for reconnect in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                install(
                    kind,
                    dir.path(),
                    &catalog(),
                    &key(),
                    "https://api.palepale.tech",
                    (None, "fixture"),
                )
                .unwrap();
                fs::remove(&dir.path().join("palepale-connection.json")).unwrap();
                if reconnect {
                    install(
                        kind,
                        dir.path(),
                        &catalog(),
                        &key(),
                        "https://api.palepale.tech",
                        (None, "fixture"),
                    )
                    .unwrap();
                }
                disconnect(kind, dir.path()).unwrap();
                let name = match kind {
                    Kind::Codex => "config.toml",
                    Kind::Claude => "settings.json",
                };
                let saved = fs::read_string(&dir.path().join(name)).unwrap().unwrap();
                let doc = match kind {
                    Kind::Codex => toml_json(&saved).unwrap(),
                    Kind::Claude => serde_json::from_str(&saved).unwrap(),
                };
                assert_eq!(doc, json!({}));
                assert!(!dir.path().join("palepale.key").exists());
            }
        }
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

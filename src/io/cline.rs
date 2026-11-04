use super::{clients, fs};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const STATE_FILE: &str = "palepale-cline.json";
const PROVIDER: &str = "palepale";

#[derive(Deserialize, Serialize)]
struct Connection {
    version: u8,
    previous_provider: Option<String>,
}

pub fn settings_dir(
    data: Option<PathBuf>,
    config: Option<PathBuf>,
    home: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let data = match data {
        Some(path) => path,
        None => config
            .or_else(|| home.map(|p| p.join(".cline")))
            .context("No Cline directory or user home directory")?
            .join("data"),
    };
    ensure!(
        data.is_absolute(),
        "CLINE_DIR and CLINE_DATA_DIR must be absolute paths"
    );
    Ok(data.join("settings"))
}

fn read_document(path: &Path) -> anyhow::Result<Value> {
    let mut doc: Value = match fs::read_string(path)? {
        Some(text) => serde_json::from_str(&text)
            .context("Invalid Cline configuration; close Cline and check its settings")?,
        None => json!({"version":1,"providers":{}}),
    };
    ensure!(
        doc["version"] == 1 && doc["providers"].is_object(),
        "Unsupported Cline configuration format"
    );
    // Keep unknown fields and every other provider when the shared file changes.
    ensure!(
        doc.as_object_mut().is_some(),
        "Cline settings must be an object"
    );
    Ok(doc)
}

fn connection(root: &Path) -> anyhow::Result<Option<Connection>> {
    let state = fs::read_string(&root.join(STATE_FILE))?
        .map(|text| serde_json::from_str::<Connection>(&text))
        .transpose()
        .context("Invalid Palepale Cline connection record")?;
    ensure!(
        state.as_ref().is_none_or(|s| s.version == 1),
        "Unsupported Palepale Cline connection record"
    );
    Ok(state)
}

fn model_entries(rows: &[Value]) -> anyhow::Result<Value> {
    let mut entries = serde_json::Map::new();
    for row in rows {
        let id = row["id"].as_str().context("Missing Cline model ID")?;
        ensure!(
            !id.is_empty()
                && id.len() <= 96
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
            "Invalid Cline model ID"
        );
        ensure!(!entries.contains_key(id), "Duplicate Cline model ID");
        let output = row["max_output_tokens"]
            .as_u64()
            .filter(|n| *n > 0)
            .context("Missing Cline output limit")?;
        let context = row["context_window"]
            .as_u64()
            .filter(|n| *n > output)
            .context("Missing Cline context limit")?;
        ensure!(
            row["capabilities"]["tool_calling"] == true,
            "Cline requires tool support"
        );
        let modalities = row["input_modalities"]
            .as_array()
            .context("Missing Cline input capabilities")?;
        ensure!(
            modalities.iter().all(|m| m == "text" || m == "image")
                && modalities.iter().any(|m| m == "text"),
            "Unsupported Cline input capabilities"
        );
        let vision = modalities.iter().any(|m| m == "image");
        let mut capabilities = vec!["streaming", "tools"];
        if vision {
            capabilities.push("images");
        }
        // Let the gateway use each model's default effort across Cline's model picker.
        let mut entry = json!({"id":id,"name":row["name"].as_str().unwrap_or(id),"maxTokens":output,"contextWindow":context,"capabilities":capabilities,"supportsVision":vision,"supportsAttachments":false,"supportsReasoning":false,"modalities":{"input":modalities,"output":["text"]}});
        for (source, target) in [
            ("input_per_1m_usd", "inputPrice"),
            ("cached_input_per_1m_usd", "cacheReadsPrice"),
            ("output_per_1m_usd", "outputPrice"),
        ] {
            let value: f64 = row["pricing"][source]
                .as_str()
                .context("Missing Cline retail price")?
                .parse()
                .context("Invalid Cline retail price")?;
            ensure!(
                value.is_finite() && value >= 0.0,
                "Invalid Cline retail price"
            );
            entry[target] = json!(value);
        }
        entries.insert(id.to_owned(), entry);
    }
    ensure!(
        !entries.is_empty(),
        "No compatible Palepale models are available for Cline"
    );
    Ok(Value::Object(entries))
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
            .is_some_and(|p| p.iter().any(|v| v == "chat_completions")),
        "This gateway does not support Cline yet"
    );
    let models = model_entries(
        catalog["cline"]["models"]
            .as_array()
            .context("Missing Cline model catalog")?,
    )?;
    let wanted = selection.0.unwrap_or(selection.1);
    ensure!(
        selection.0.is_none() || models.get(wanted).is_some(),
        "This model is not available for Cline"
    );
    let id = if models.get(wanted).is_some() {
        wanted
    } else {
        catalog["cline"]["models"][0]["id"]
            .as_str()
            .context("Missing Cline model")?
    };
    let mut providers = read_document(&root.join("providers.json"))?;
    let mut registry = read_document(&root.join("models.json"))?;
    let state = match connection(root)? {
        Some(state) => state,
        None => {
            ensure!(
                providers["providers"].get(PROVIDER).is_none()
                    && registry["providers"].get(PROVIDER).is_none(),
                "A Palepale provider already exists in Cline. Remove it in Cline before connecting"
            );
            Connection {
                version: 1,
                previous_provider: providers["lastUsedProvider"].as_str().map(str::to_owned),
            }
        }
    };
    let template: Value = serde_json::from_str(include_str!("../../data/cline.json"))?;
    let base_url = format!("{}/v1", origin.trim_end_matches('/'));
    let mut settings = template.clone();
    settings
        .as_object_mut()
        .expect("cline template")
        .remove("name");
    settings["apiKey"] = json!(key);
    settings["model"] = json!(id);
    settings["baseUrl"] = json!(base_url);
    let old_entry = &providers["providers"][PROVIDER];
    let updated_at = if old_entry["settings"] == settings {
        old_entry["updatedAt"].clone()
    } else {
        json!(super::clock::now().to_string())
    };
    providers["providers"][PROVIDER] =
        json!({"settings":settings,"tokenSource":"manual","updatedAt":updated_at});
    providers["lastUsedProvider"] = json!(PROVIDER);
    registry["providers"][PROVIDER] = json!({"provider":{"name":template["name"],"baseUrl":base_url,"defaultModelId":id,"protocol":template["protocol"],"client":template["client"],"capabilities":template["capabilities"]},"models":models});
    clients::replace_files(&[
        (root.join(STATE_FILE), serde_json::to_vec_pretty(&state)?),
        (
            root.join("models.json"),
            serde_json::to_vec_pretty(&registry)?,
        ),
        (
            root.join("providers.json"),
            serde_json::to_vec_pretty(&providers)?,
        ),
    ])?;
    Ok(id.to_owned())
}

pub fn disconnect(root: &Path) -> anyhow::Result<()> {
    let Some(state) = connection(root)? else {
        return Ok(());
    };
    let mut writes = Vec::new();
    for name in ["providers.json", "models.json"] {
        let path = root.join(name);
        if fs::read_string(&path)?.is_none() {
            continue;
        }
        let mut doc = read_document(&path)?;
        doc["providers"]
            .as_object_mut()
            .expect("validated providers")
            .remove(PROVIDER);
        if name == "providers.json" && doc["lastUsedProvider"] == PROVIDER {
            let object = doc.as_object_mut().expect("validated document");
            object.remove("lastUsedProvider");
            if let Some(previous) = &state.previous_provider
                && object["providers"].get(previous).is_some()
            {
                object.insert("lastUsedProvider".into(), json!(previous));
            }
        }
        writes.push((path, serde_json::to_vec_pretty(&doc)?));
    }
    clients::replace_files(&writes)?;
    fs::remove(&root.join(STATE_FILE))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::fs::Access;
    use super::*;

    fn catalog() -> Vec<u8> {
        serde_json::to_vec(&json!({"api_base_url":"https://api.palepale.tech","protocols":["chat_completions"],"cline":{"models":[
            {"id":"vision","name":"Vision","max_output_tokens":1024,"context_window":32768,"input_modalities":["text","image"],"capabilities":{"tool_calling":true},"pricing":{"input_per_1m_usd":"0.5","cached_input_per_1m_usd":"0.05","output_per_1m_usd":"1.5"}},
            {"id":"text","max_output_tokens":2048,"context_window":65536,"input_modalities":["text"],"capabilities":{"tool_calling":true},"pricing":{"input_per_1m_usd":"0","cached_input_per_1m_usd":"0","output_per_1m_usd":"0"}}
        ]}})).unwrap()
    }

    #[test]
    fn connect_refresh_and_disconnect_preserve_other_providers_and_later_edits() {
        let root = tempfile::tempdir().unwrap();
        let key = format!("pk_{}", "x".repeat(43));
        let provider_path = root.path().join("providers.json");
        let model_path = root.path().join("models.json");
        fs::write_atomic(&provider_path, br#"{"version":1,"lastUsedProvider":"other","providers":{"other":{"settings":{"apiKey":"fixture-other"}}},"modes":{}}"#,Access::Private).unwrap();
        fs::write_atomic(
            &model_path,
            br#"{"version":1,"providers":{"other":{"models":{"keep":{}}}}}"#,
            Access::Private,
        )
        .unwrap();
        install(
            root.path(),
            &catalog(),
            &key,
            "https://api.palepale.tech",
            (None, "missing"),
        )
        .unwrap();
        let first = fs::read_string(&provider_path).unwrap().unwrap();
        install(
            root.path(),
            &catalog(),
            &key,
            "https://api.palepale.tech",
            (None, "missing"),
        )
        .unwrap();
        assert_eq!(first, fs::read_string(&provider_path).unwrap().unwrap());
        let mut providers = read_document(&provider_path).unwrap();
        assert_eq!(
            providers["providers"][PROVIDER]["settings"]["provider"],
            PROVIDER
        );
        assert_eq!(
            providers["providers"][PROVIDER]["settings"]["model"],
            "vision"
        );
        let models = read_document(&model_path).unwrap();
        assert!(models["providers"]["other"]["models"]["keep"].is_object());
        assert_eq!(
            models["providers"][PROVIDER]["models"]["vision"]["supportsVision"],
            true
        );
        assert_eq!(
            models["providers"][PROVIDER]["models"]["text"]["supportsVision"],
            false
        );
        assert_eq!(
            models["providers"][PROVIDER]["models"]["vision"]["inputPrice"],
            0.5
        );
        assert!(
            !fs::read_string(&model_path)
                .unwrap()
                .unwrap()
                .contains(&key)
        );
        assert!(
            !fs::read_string(&root.path().join(STATE_FILE))
                .unwrap()
                .unwrap()
                .contains(&key)
        );
        providers["providers"][PROVIDER]["settings"]["model"] = json!("text");
        providers["providers"]["other"]["edited"] = json!(true);
        fs::write_atomic(
            &provider_path,
            &serde_json::to_vec(&providers).unwrap(),
            Access::Private,
        )
        .unwrap();
        disconnect(root.path()).unwrap();
        let restored = read_document(&provider_path).unwrap();
        assert_eq!(restored["lastUsedProvider"], "other");
        assert_eq!(restored["providers"]["other"]["edited"], true);
        assert!(restored["providers"].get(PROVIDER).is_none());
        assert!(
            read_document(&model_path).unwrap()["providers"]
                .get(PROVIDER)
                .is_none()
        );
        assert!(
            !fs::read_string(&provider_path)
                .unwrap()
                .unwrap()
                .contains(&key)
        );
        assert!(!root.path().join(STATE_FILE).exists());
        disconnect(root.path()).unwrap();
    }

    #[test]
    fn invalid_catalog_config_and_collision_do_not_write_credentials() {
        let key = format!("pk_{}", "x".repeat(43));
        for (document, model, origin) in [
            ("broken{", None, "https://api.palepale.tech"),
            (
                r#"{"version":2,"providers":{}}"#,
                None,
                "https://api.palepale.tech",
            ),
            (
                r#"{"version":1,"providers":{"palepale":{}}}"#,
                None,
                "https://api.palepale.tech",
            ),
            (
                r#"{"version":1,"providers":{}}"#,
                Some("absent"),
                "https://api.palepale.tech",
            ),
            (
                r#"{"version":1,"providers":{}}"#,
                None,
                "https://other.test",
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("providers.json");
            fs::write_atomic(&path, document.as_bytes(), Access::Private).unwrap();
            assert!(install(root.path(), &catalog(), &key, origin, (model, "vision")).is_err());
            assert_eq!(fs::read_string(&path).unwrap().unwrap(), document);
            assert!(!root.path().join(STATE_FILE).exists());
            assert!(!root.path().join("models.json").exists());
        }
    }

    #[test]
    fn data_directory_override_has_priority_and_relative_paths_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            settings_dir(
                Some(root.path().join("custom")),
                Some(root.path().join("config")),
                None
            )
            .unwrap(),
            root.path().join("custom/settings")
        );
        assert_eq!(
            settings_dir(None, Some(root.path().join("config")), None).unwrap(),
            root.path().join("config/data/settings")
        );
        assert!(settings_dir(Some(PathBuf::from("relative")), None, None).is_err());
    }
}

use super::{clients, fs};
use anyhow::{Context, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const STATE_FILE: &str = "palepale-antigravity.json";

#[derive(serde::Serialize, serde::Deserialize)]
struct Connection {
    version: u8,
    previous_models: Option<Value>,
    previous_cli_model: Option<Value>,
    previous_cli_provider: Option<Value>,
    installed_models: Value,
    installed_model: String,
    origin: String,
}

pub fn settings_dir(
    explicit: Option<PathBuf>,
    server_root: Option<PathBuf>,
    home: Option<PathBuf>,
) -> anyhow::Result<PathBuf> {
    let path = match (explicit, server_root) {
        (Some(path), _) => path,
        (None, Some(root)) => {
            ensure!(
                root.is_absolute(),
                "Antigravity server path must be absolute"
            );
            root.parent()
                .context("Antigravity server path needs a parent")?
                .join(".antigravity")
        }
        _ => home.context("No user home directory")?.join(".antigravity"),
    };
    ensure!(
        path.is_absolute(),
        "Antigravity configuration path must be absolute"
    );
    Ok(path)
}

fn cli_path(root: &Path) -> PathBuf {
    root.parent()
        .unwrap_or(root)
        .join(".gemini")
        .join("antigravity-cli")
        .join("settings.json")
}

fn read_document(path: &Path) -> anyhow::Result<Value> {
    let text = fs::read_string(path)?.unwrap_or_else(|| "{}".to_owned());
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("Invalid Antigravity configuration: {}", path.display()))?;
    ensure!(
        value.is_object(),
        "Antigravity configuration must be a JSON object"
    );
    Ok(value)
}

fn read_state(root: &Path) -> anyhow::Result<Option<Connection>> {
    let state: Option<Connection> = fs::read_string(&root.join(STATE_FILE))?
        .map(|text| serde_json::from_str(&text))
        .transpose()
        .context("Invalid Palepale Antigravity connection record")?;
    ensure!(
        state.as_ref().is_none_or(|state| state.version == 1),
        "Unsupported Antigravity connection record"
    );
    Ok(state)
}

fn model_ids(catalog: &Value) -> anyhow::Result<Vec<String>> {
    let rows = catalog["antigravity"]["models"]
        .as_array()
        .or_else(|| catalog["cline"]["models"].as_array())
        .context("Missing Antigravity model catalog")?;
    ensure!(
        !rows.is_empty(),
        "No compatible Palepale models are available for Antigravity"
    );
    let mut ids = std::collections::BTreeSet::new();
    rows.iter()
        .map(|row| {
            let id = row
                .get("modelId")
                .or_else(|| row.get("id"))
                .and_then(Value::as_str)
                .context("Invalid Antigravity model id")?;
            ensure!(
                !id.is_empty()
                    && id.len() <= 96
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
                "Invalid Antigravity model id"
            );
            ensure!(ids.insert(id), "Duplicate Antigravity model id");
            Ok(id.to_owned())
        })
        .collect()
}

fn managed_gateway(settings: &Value, state: &Connection) -> bool {
    settings["gateway"]["url"] == state.origin
        && settings["gateway"]["customHeaders"]["X-Palepale-Client"] == "antigravity"
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
        "This gateway does not support Antigravity's OpenAI gateway mode"
    );
    let models = model_ids(&catalog)?;
    let wanted = selection.0.unwrap_or(selection.1);
    ensure!(
        selection.0.is_none() || models.iter().any(|id| id == wanted),
        "This model is not available for Antigravity"
    );
    let selected = models
        .iter()
        .find(|id| id.as_str() == wanted)
        .unwrap_or(&models[0]);
    let root = fs::canonical(root);
    let settings_path = root.join("settings.json");
    let cli_path = cli_path(&root);
    let mut settings = read_document(&settings_path)?;
    let mut cli = read_document(&cli_path)?;
    let previous_state = read_state(&root)?;
    ensure!(
        settings.get("gateway").is_none_or(Value::is_null)
            || previous_state
                .as_ref()
                .is_some_and(|state| managed_gateway(&settings, state)),
        "An existing Antigravity gateway is configured; disconnect it before connecting Palepale"
    );
    let installed_models = json!({
        "defaultModelId": selected,
        "list": models.iter().map(|id| json!({"modelId": id})).collect::<Vec<_>>()
    });
    let mut state = previous_state.unwrap_or_else(|| Connection {
        version: 1,
        previous_models: settings.get("models").cloned(),
        previous_cli_model: cli.get("model").cloned(),
        previous_cli_provider: cli.get("modelProvider").cloned(),
        installed_models: Value::Null,
        installed_model: String::new(),
        origin: String::new(),
    });
    state.installed_models = installed_models.clone();
    state.installed_model = selected.clone();
    state.origin = origin.trim_end_matches('/').to_owned();
    settings["gateway"] = json!({
        "enabled": true,
        "url": state.origin,
        "apiKey": key,
        "wireProtocol": "openai",
        "customHeaders": {"X-Palepale-Client": "antigravity"}
    });
    settings["models"] = installed_models;
    cli["model"] = json!(selected);
    cli.as_object_mut()
        .expect("validated object")
        .remove("modelProvider");
    // Write recovery data before changing settings. It contains no credential.
    save(&root.join(STATE_FILE), &serde_json::to_value(&state)?)?;
    save(&settings_path, &settings)?;
    save(&cli_path, &cli)?;
    Ok(selected.clone())
}

fn save(path: &Path, value: &Value) -> anyhow::Result<()> {
    Ok(fs::write_atomic(
        path,
        &serde_json::to_vec_pretty(value)?,
        fs::Access::Private,
    )?)
}

fn restore(document: &mut Value, field: &str, previous: Option<Value>) {
    let object = document.as_object_mut().expect("validated object");
    match previous {
        Some(value) => {
            object.insert(field.into(), value);
        }
        None => {
            object.remove(field);
        }
    }
}

fn ide_executable() -> anyhow::Result<PathBuf> {
    let launcher = super::proc::which("antigravity-ide");
    #[cfg(windows)]
    {
        // A terminal opened before IDE installation may still have the old PATH.
        let on_path = launcher
            .as_deref()
            .and_then(Path::parent)
            .and_then(Path::parent);
        let user = crate::settings::env("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("Programs/Antigravity IDE"));
        let system =
            crate::settings::env("ProgramFiles").map(|p| PathBuf::from(p).join("Antigravity IDE"));
        [on_path, user.as_deref(), system.as_deref()]
            .into_iter()
            .flatten()
            .map(|dir| dir.join("Antigravity IDE.exe"))
            .find(|exe| exe.is_file())
            .context("Install Antigravity IDE or add its bin directory to PATH")
    }
    #[cfg(not(windows))]
    launcher.context("Install Antigravity IDE and add antigravity-ide to PATH")
}

fn ide_args(root: &Path, args: &[String]) -> Vec<String> {
    let mut forwarded = Vec::new();
    // Electron hands launches for the same profile to the already-running process.
    if !args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| arg == "--user-data-dir" || arg.starts_with("--user-data-dir="))
    {
        forwarded.push("--user-data-dir".into());
        forwarded.push(root.join("palepale-ide").to_string_lossy().into_owned());
    }
    forwarded.extend_from_slice(args);
    forwarded
}

pub async fn launch(root: &Path, origin: &str, ide: bool, args: &[String]) -> anyhow::Result<()> {
    let state = read_state(root)?.context("Run palepale antigravity first")?;
    let settings = read_document(&root.join("settings.json"))?;
    ensure!(
        managed_gateway(&settings, &state) && state.origin == origin.trim_end_matches('/'),
        "Antigravity gateway changed; reconnect with palepale antigravity"
    );
    let key = settings["gateway"]["apiKey"]
        .as_str()
        .context("Missing Antigravity key")?;
    clients::validate_key(key)?;
    let models = settings["models"]["list"]
        .as_array()
        .context("Missing Antigravity models")?
        .iter()
        .filter_map(|model| model["modelId"].as_str())
        .collect::<Vec<_>>()
        .join(",");
    let env = vec![
        ("AGY_LLM_GATEWAY_URL".into(), state.origin),
        ("AGY_LLM_GATEWAY_API_KEY".into(), key.to_owned()),
        ("AGY_LLM_GATEWAY_WIRE_PROTOCOL".into(), "openai".into()),
        ("AGY_LLM_GATEWAY_MODELS".into(), models),
        (
            "AGY_LLM_GATEWAY_HEADERS".into(),
            "X-Palepale-Client: antigravity".into(),
        ),
    ];
    let exe = if ide {
        ide_executable()?
    } else {
        super::proc::which("agy").context("Install Antigravity CLI and add agy to PATH")?
    };
    let mut forwarded = if ide {
        ide_args(root, args)
    } else {
        args.to_vec()
    };
    if !ide
        && !args
            .iter()
            .any(|arg| arg == "--model" || arg.starts_with("--model="))
    {
        forwarded.splice(..0, ["--model".into(), state.installed_model]);
    }
    let status = super::proc::run_attached(&exe, &forwarded, &env)
        .await
        .context("Could not start Antigravity")?;
    ensure!(status.success(), "Antigravity exited with {status}");
    Ok(())
}

pub fn disconnect(root: &Path) -> anyhow::Result<()> {
    let root = fs::canonical(root);
    let Some(state) = read_state(&root)? else {
        return Ok(());
    };
    let settings_path = root.join("settings.json");
    let cli_path = cli_path(&root);
    let mut settings = read_document(&settings_path)?;
    let mut cli = read_document(&cli_path)?;
    if managed_gateway(&settings, &state) {
        restore(&mut settings, "gateway", None);
        if settings.get("models") == Some(&state.installed_models) {
            restore(&mut settings, "models", state.previous_models);
        }
        save(&settings_path, &settings)?;
    }
    // Preserve selections changed by the user after setup.
    if cli["model"] == state.installed_model && cli.get("modelProvider").is_none() {
        restore(&mut cli, "model", state.previous_cli_model);
        restore(&mut cli, "modelProvider", state.previous_cli_provider);
        save(&cli_path, &cli)?;
    }
    fs::remove(&root.join(STATE_FILE))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ide_launch_isolates_the_default_profile_and_preserves_explicit_profile_and_ssh_args() {
        let home = tempfile::tempdir().unwrap();
        let ssh = [
            "--remote".to_owned(),
            "ssh-remote+dev".into(),
            "/workspace".into(),
        ];
        let args = ide_args(home.path(), &ssh);
        assert_eq!(args[0], "--user-data-dir");
        assert_eq!(Path::new(&args[1]), home.path().join("palepale-ide"));
        assert_eq!(args[2..], ssh);
        for custom in [
            vec!["--user-data-dir=custom".into()],
            vec!["--user-data-dir".into(), "custom".into()],
        ] {
            assert_eq!(ide_args(home.path(), &custom), custom);
        }
    }

    fn catalog() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "api_base_url":"https://api.palepale.tech", "protocols":["chat_completions"],
            "antigravity":{"models":[{"modelId":"fixture"},{"modelId":"vision"}]}
        }))
        .unwrap()
    }

    fn connect(root: &Path, model: &str) -> anyhow::Result<String> {
        install(
            root,
            &catalog(),
            &format!("pk_{}", "x".repeat(43)),
            "https://api.palepale.tech",
            (Some(model), "fixture"),
        )
    }

    #[test]
    fn reconnect_then_disconnect_restores_original_settings_and_leaves_acp_untouched() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".antigravity");
        let original = json!({"theme":"dark", "models":{"defaultModelId":"old"}});
        let original_cli = json!({"model":"old", "modelProvider":"old-provider", "theme":"dark"});
        save(&root.join("settings.json"), &original).unwrap();
        save(&cli_path(&root), &original_cli).unwrap();
        assert_eq!(connect(&root, "fixture").unwrap(), "fixture");
        connect(&root, "vision").unwrap();
        let settings = read_document(&root.join("settings.json")).unwrap();
        assert_eq!(settings["gateway"]["wireProtocol"], "openai");
        assert_eq!(settings["models"]["defaultModelId"], "vision");
        assert!(settings["gateway"].get("models").is_none());
        assert_eq!(settings["models"]["list"].as_array().unwrap().len(), 2);
        let state = fs::read_string(&root.join(STATE_FILE)).unwrap().unwrap();
        assert!(!state.contains("pk_"));
        assert!(!home.path().join(".gemini/antigravity-acp").exists());
        disconnect(&root).unwrap();
        disconnect(&root).unwrap();
        assert_eq!(
            read_document(&root.join("settings.json")).unwrap(),
            original
        );
        assert_eq!(read_document(&cli_path(&root)).unwrap(), original_cli);
    }

    #[test]
    fn invalid_selection_or_foreign_gateway_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".antigravity");
        assert!(connect(&root, "missing").is_err());
        assert!(!root.exists());
        let original = json!({"gateway":{"url":"https://other.example"}});
        save(&root.join("settings.json"), &original).unwrap();
        assert!(connect(&root, "fixture").is_err());
        assert_eq!(
            read_document(&root.join("settings.json")).unwrap(),
            original
        );
        assert!(!root.join(STATE_FILE).exists());
    }

    #[test]
    fn disconnect_preserves_later_edits_and_handles_invalid_json_without_panicking() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(".antigravity");
        connect(&root, "fixture").unwrap();
        save(&cli_path(&root), &json!([])).unwrap();
        assert!(disconnect(&root).is_err());
        assert!(
            read_document(&root.join("settings.json"))
                .unwrap()
                .get("gateway")
                .is_some()
        );
        save(&cli_path(&root), &json!({"model":"user-model"})).unwrap();
        let mut settings = read_document(&root.join("settings.json")).unwrap();
        settings["models"] = json!({"defaultModelId":"user-model"});
        save(&root.join("settings.json"), &settings).unwrap();
        disconnect(&root).unwrap();
        assert_eq!(
            read_document(&cli_path(&root)).unwrap()["model"],
            "user-model"
        );
        assert_eq!(
            read_document(&root.join("settings.json")).unwrap()["models"],
            settings["models"]
        );
    }

    #[test]
    fn server_root_selects_remote_home_without_writing_runtime_files() {
        let home = tempfile::tempdir().unwrap();
        let server = home.path().join(".antigravity-ide-server");
        assert_eq!(
            settings_dir(None, Some(server), None).unwrap(),
            home.path().join(".antigravity")
        );
        assert!(settings_dir(None, Some(PathBuf::from("relative")), None).is_err());
    }
}

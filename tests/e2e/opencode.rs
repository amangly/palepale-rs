use crate::fake_palepale::{CLIENT_KEY, Fake};
use crate::{CLEARED, Run, Sandbox, binary, launch_env};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn setup_merges_existing_jsonc_and_exports_only_an_inference_key() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let config_home = sandbox.home().join("config");
    let root = config_home.join("opencode");
    std::fs::create_dir_all(&root).unwrap();
    let original = "{// user settings\n\"model\":\"other/model\",\"providers\":{\"other\":{}},}";
    std::fs::write(root.join("opencode.jsonc"), original).unwrap();
    let mut command = tokio::process::Command::new(binary());
    command
        .args(["opencode"])
        .current_dir(sandbox.project.path())
        .envs(launch_env(sandbox.home(), &fake.url, Some(CLIENT_KEY)))
        .env("XDG_CONFIG_HOME", &config_home)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    for name in CLEARED {
        if *name != "PALEPALE_API_KEY" {
            command.env_remove(name);
        }
    }
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .unwrap()
        .unwrap();
    let run = Run(output);
    run.success();
    assert!(!run.stdout().contains(CLIENT_KEY));
    assert!(!run.stderr().contains(CLIENT_KEY));
    assert_eq!(fake.hits("/v1/account/summary"), 1);
    assert_eq!(fake.hits("/docs/opencode.json"), 1);
    assert_eq!(fake.hits("/docs/opencode-plugin.js"), 1);
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
    let config = std::fs::read_to_string(root.join("opencode.jsonc")).unwrap();
    assert!(!config.contains(CLIENT_KEY));
    let config: Value = serde_json::from_str(&config).unwrap();
    assert_eq!(config["model"], "other/model");
    assert!(config["providers"]["other"].is_object());
    assert_eq!(
        config["providers"]["palepale"]["settings"]["baseURL"],
        format!("{}/v1", fake.url)
    );
    assert_eq!(
        config["providers"]["palepale"]["settings"]["apiKey"],
        "{file:./palepale.key}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("palepale.key")).unwrap(),
        CLIENT_KEY
    );
    assert!(root.join("plugins/palepale-models.js").is_file());
    let backups: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "bak"))
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(
        std::fs::read_to_string(backups[0].path()).unwrap(),
        original
    );
}

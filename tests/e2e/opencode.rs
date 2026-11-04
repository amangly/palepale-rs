use crate::fake_palepale::{CLIENT_KEY, Fake};
use crate::{CLEARED, Run, Sandbox, binary, launch_env};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn cline_setup_exports_the_catalog_and_disconnects_offline() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let root = sandbox.home().join("cline");
    for disconnect in [false, true, true] {
        let mut command = tokio::process::Command::new(binary());
        command
            .args(["cline", if disconnect { "--logout" } else { "fixture" }])
            .current_dir(sandbox.project.path())
            .envs(launch_env(
                sandbox.home(),
                &fake.url,
                if disconnect { None } else { Some(CLIENT_KEY) },
            ))
            .env("CLINE_DIR", &root)
            .env_remove("CLINE_DATA_DIR")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        for name in CLEARED {
            if *name != "PALEPALE_API_KEY" {
                command.env_remove(name);
            }
        }
        if disconnect {
            command
                .env_remove("PALEPALE_API_KEY")
                .env("PALEPALE_OFFLINE", "1");
        }
        let run = Run(
            tokio::time::timeout(Duration::from_secs(30), command.output())
                .await
                .unwrap()
                .unwrap(),
        );
        run.success();
        assert!(!run.stdout().contains(CLIENT_KEY));
        assert!(!run.stderr().contains(CLIENT_KEY));
        let settings = root.join("data/settings");
        let providers: Value = serde_json::from_str(
            &std::fs::read_to_string(settings.join("providers.json")).unwrap(),
        )
        .unwrap();
        let models: Value =
            serde_json::from_str(&std::fs::read_to_string(settings.join("models.json")).unwrap())
                .unwrap();
        if disconnect {
            assert!(providers["providers"].get("palepale").is_none());
            assert!(models["providers"].get("palepale").is_none());
        } else {
            assert_eq!(
                providers["providers"]["palepale"]["settings"]["baseUrl"],
                format!("{}/v1", fake.url)
            );
            assert_eq!(
                providers["providers"]["palepale"]["settings"]["apiKey"],
                CLIENT_KEY
            );
            assert_eq!(
                models["providers"]["palepale"]["models"]["fixture"]["supportsVision"],
                true
            );
        }
    }
    assert_eq!(fake.hits("/docs/clients.json"), 1);
    assert_eq!(fake.hits("/v1/account/summary"), 1);
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

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

#[tokio::test(flavor = "multi_thread")]
async fn codex_and_claude_connect_in_api_mode_without_inference_or_key_output() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    for (client, variable, file) in [
        ("codex", "CODEX_HOME", "config.toml"),
        ("claude", "CLAUDE_CONFIG_DIR", "settings.json"),
    ] {
        let root = sandbox.home().join(client);
        let mut command = tokio::process::Command::new(binary());
        command
            .args([client, "fixture"])
            .current_dir(sandbox.project.path())
            .envs(launch_env(sandbox.home(), &fake.url, Some(CLIENT_KEY)))
            .env(variable, &root)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        for name in CLEARED {
            if *name != "PALEPALE_API_KEY" {
                command.env_remove(name);
            }
        }
        let run = Run(
            tokio::time::timeout(Duration::from_secs(30), command.output())
                .await
                .unwrap()
                .unwrap(),
        );
        run.success();
        assert!(!run.stdout().contains(CLIENT_KEY));
        assert!(!run.stderr().contains(CLIENT_KEY));
        let saved = std::fs::read_to_string(root.join(file)).unwrap();
        assert!(!saved.contains(CLIENT_KEY));
        assert!(saved.contains("client-key"));
        assert!(saved.contains("fixture"));
        if client == "claude" {
            let config: Value = serde_json::from_str(&saved).unwrap();
            assert_eq!(config["modelPicker"]["replaceBuiltInOptions"], true);
            assert_eq!(config["modelPicker"]["options"][1]["model"], "fixture-two");
            assert_eq!(
                config["modelPicker"]["options"].as_array().unwrap().len(),
                2
            );
        }
        let broken_project = sandbox.project.path().join("broken-project");
        std::fs::create_dir_all(broken_project.join(".palepale")).unwrap();
        std::fs::write(broken_project.join(".palepale/config.toml"), "broken{").unwrap();
        let helper = tokio::process::Command::new(binary())
            .arg("client-key")
            .arg(root.join("palepale.key"))
            .current_dir(&broken_project)
            .envs(launch_env(sandbox.home(), &fake.url, None))
            .env_remove("PALEPALE_API_KEY")
            .stdin(Stdio::null())
            .output()
            .await
            .unwrap();
        assert!(helper.status.success());
        assert_eq!(String::from_utf8(helper.stdout).unwrap().trim(), CLIENT_KEY);
    }
    assert_eq!(fake.hits("/docs/clients.json"), 2);
    assert_eq!(fake.hits("/v1/account/summary"), 2);
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_is_offline_idempotent_and_keeps_other_client_settings() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    for (client, variable, file, original) in [
        (
            "codex",
            "CODEX_HOME",
            "config.toml",
            "model = 'previous'\napproval_policy = 'on-request'\n",
        ),
        (
            "claude",
            "CLAUDE_CONFIG_DIR",
            "settings.json",
            r#"{"model":"previous","permissions":{"deny":["Bash(rm *)"]}}"#,
        ),
        (
            "opencode",
            "XDG_CONFIG_HOME",
            "opencode.json",
            r#"{"model":"other/model","providers":{"other":{}}}"#,
        ),
    ] {
        let env_root = sandbox.home().join(client);
        let root = if client == "opencode" {
            env_root.join("opencode")
        } else {
            env_root.clone()
        };
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(file), original).unwrap();
        for disconnect in [false, true, true] {
            let mut command = tokio::process::Command::new(binary());
            command.arg(client);
            if disconnect {
                command.arg("--logout");
            }
            command
                .current_dir(sandbox.project.path())
                .envs(launch_env(
                    sandbox.home(),
                    &fake.url,
                    if disconnect { None } else { Some(CLIENT_KEY) },
                ))
                .env(variable, &env_root)
                .stdin(Stdio::null())
                .kill_on_drop(true);
            for name in CLEARED {
                if *name != "PALEPALE_API_KEY" {
                    command.env_remove(name);
                }
            }
            if disconnect {
                command
                    .env_remove("PALEPALE_API_KEY")
                    .env("PALEPALE_OFFLINE", "1");
            }
            let run = Run(
                tokio::time::timeout(Duration::from_secs(30), command.output())
                    .await
                    .unwrap()
                    .unwrap(),
            );
            run.success();
            assert!(!run.stdout().contains(CLIENT_KEY));
            assert!(!run.stderr().contains(CLIENT_KEY));
        }
        assert!(!root.join("palepale.key").exists());
        assert!(!root.join("palepale-models.json").exists());
        assert!(!root.join("plugins/palepale-models.js").exists());
        let saved = std::fs::read_to_string(root.join(file)).unwrap();
        if client == "codex" {
            let actual: toml::Value = toml::from_str(&saved).unwrap();
            let expected: toml::Value = toml::from_str(original).unwrap();
            assert_eq!(actual, expected);
        } else {
            assert_eq!(
                serde_json::from_str::<Value>(&saved).unwrap(),
                serde_json::from_str::<Value>(original).unwrap()
            );
        }
    }
    assert_eq!(fake.hits("/v1/account/summary"), 3);
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

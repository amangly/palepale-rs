use crate::fake_palepale::{CLIENT_KEY, Fake};
use crate::{CLEARED, Run, Sandbox, binary, launch_env};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn antigravity_setup_and_offline_disconnect_use_the_remote_users_config() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let root = sandbox.home().join(".antigravity");
    for disconnect in [false, false, true, true] {
        let mut command = tokio::process::Command::new(binary());
        for name in CLEARED {
            command.env_remove(name);
        }
        command
            .args([
                "antigravity",
                if disconnect { "--logout" } else { "fixture" },
            ])
            .current_dir(sandbox.project.path())
            .envs(launch_env(
                sandbox.home(),
                &fake.url,
                if disconnect { None } else { Some(CLIENT_KEY) },
            ))
            .env_remove("ANTIGRAVITY_CONFIG_DIR")
            .env(
                "ANTIGRAVITY_IDE_SERVER_ROOT",
                sandbox.home().join(".antigravity-ide-server"),
            )
            .stdin(Stdio::null())
            .kill_on_drop(true);
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
        let settings: Value =
            serde_json::from_slice(&std::fs::read(root.join("settings.json")).unwrap()).unwrap();
        let cli: Value = serde_json::from_slice(
            &std::fs::read(sandbox.home().join(".gemini/antigravity-cli/settings.json")).unwrap(),
        )
        .unwrap();
        assert!(!sandbox.home().join(".antigravity-ide-server").exists());
        assert!(!sandbox.home().join(".gemini/antigravity-acp").exists());
        if disconnect {
            assert!(settings.get("gateway").is_none());
            assert!(settings.get("models").is_none());
            assert!(cli.get("model").is_none());
        } else {
            assert_eq!(settings["gateway"]["url"], fake.url);
            assert_eq!(settings["gateway"]["apiKey"], CLIENT_KEY);
            assert_eq!(settings["gateway"]["wireProtocol"], "openai");
            assert_eq!(settings["models"]["list"][0]["modelId"], "fixture");
            assert_eq!(cli["model"], "fixture");
        }
    }
}

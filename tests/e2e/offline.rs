// offline.rs
// Module: Backend deferral and credential isolation checks
// Build: Rust 1.98.1 / Windows, macOS, Linux

use crate::Sandbox;

#[tokio::test(flavor = "multi_thread")]
async fn unconfigured_client_ignores_credentials_and_cached_provider_models() {
    let sandbox = Sandbox::default();
    std::fs::write(
        sandbox.home().join("auth.json"),
        "invalid encrypted credentials",
    )
    .unwrap();
    std::fs::create_dir_all(sandbox.home().join("cache")).unwrap();
    std::fs::write(
        sandbox.home().join("cache/models.json"),
        "invalid cached catalog",
    )
    .unwrap();
    let models = sandbox
        .run("", &["models"], Some("secret-that-must-not-appear"))
        .await;
    let out = models.success().stdout();
    assert!(out.contains("Backend not configured"), "{out}");
    assert!(!out.contains("demo-2") && !out.contains("secret-that-must-not-appear"));
    let request = sandbox
        .run(
            "",
            &["--print", "hello"],
            Some("secret-that-must-not-appear"),
        )
        .await;
    assert_eq!(request.code(), Some(1));
    assert!(request.stderr().contains("Backend not configured"));
    assert!(!request.stderr().contains("secret-that-must-not-appear"));
    assert!(sandbox.sessions().is_empty());
    assert!(!sandbox.home().join("update.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn help_exposes_device_login_but_offline_account_commands_require_a_backend() {
    let sandbox = Sandbox::default();
    let help = sandbox.run("", &["help"], None).await;
    let out = help.success().stdout();
    assert!(out.contains("palepale login") && !out.contains("palepale signup"));
    for command in ["login", "signup", "logout"] {
        let run = sandbox.run("", &[command], None).await;
        assert_eq!(run.code(), Some(1));
        assert!(run.stderr().contains("configured backend"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unrelated_environment_cannot_configure_the_client() {
    let fake = crate::fake_palepale::Fake::start().await;
    let sandbox = Sandbox::default();
    let output = tokio::process::Command::new(crate::binary())
        .args(["--print", "hello"])
        .current_dir(sandbox.project.path())
        .envs(crate::launch_env(sandbox.home(), "", None))
        .env("OTHER_API_URL", &fake.url)
        .env("OTHER_CHAT_URL", &fake.url)
        .env("OTHER_API_KEY", crate::fake_palepale::ENV_KEY)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Backend not configured"));
    assert_eq!(fake.hits("/v1/models"), 0);
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

use crate::Sandbox;

#[tokio::test]
async fn discord_help_status_and_logout_work_without_network_or_credentials() {
    let sandbox = Sandbox::default();
    for args in [vec!["discord"], vec!["discord", "status"]] {
        let run = sandbox.run("", &args, None).await;
        assert!(run.success().stdout().contains("not configured"));
    }
    let help = sandbox.run("", &["discord", "help"], None).await;
    assert!(help.success().stdout().contains("--owner"));
    std::fs::write(
        sandbox.home().join("discord.token"),
        "fixture-private-token",
    )
    .unwrap();
    std::fs::write(sandbox.home().join("discord.toml"), "invalid setup").unwrap();
    std::fs::write(sandbox.home().join("auth.json"), "preserve-login").unwrap();
    let run = sandbox.run("", &["discord", "logout"], None).await;
    assert!(!run.success().stdout().contains("fixture-private-token"));
    assert!(!sandbox.home().join("discord.token").exists());
    assert!(!sandbox.home().join("discord.toml").exists());
    assert_eq!(
        std::fs::read_to_string(sandbox.home().join("auth.json")).unwrap(),
        "preserve-login"
    );
}

#[tokio::test]
async fn discord_setup_rejects_missing_owner_before_contacting_discord() {
    let sandbox = Sandbox::default();
    let run = sandbox
        .run("http://127.0.0.1:1", &["discord", "setup"], None)
        .await;
    assert_eq!(run.code(), Some(1));
    assert!(run.stderr().contains("owner user ID"), "{}", run.stderr());
    let run = sandbox
        .run(
            "http://127.0.0.1:1",
            &["discord", "setup", "--owner", "bad/id"],
            None,
        )
        .await;
    assert_eq!(run.code(), Some(1));
    assert!(run.stderr().contains("owner user ID"));
    assert!(!sandbox.home().join("discord.token").exists());
}

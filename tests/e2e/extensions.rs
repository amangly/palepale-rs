use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::json;

use crate::fake_palepale::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};

#[tokio::test(flavor = "multi_thread")]
async fn mcp_cli_registers_checks_and_removes_only_palepales_selected_config() {
    let sandbox = Sandbox::default();
    let home = sandbox.home();
    std::fs::write(
        home.join("config.toml"),
        "# keep my settings\nmodel = \"demo-2\"\n",
    )
    .unwrap();
    let codex = home.join(".codex/config.toml");
    std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
    std::fs::write(&codex, "# another client\n").unwrap();
    std::fs::write(home.join(".claude.json"), "{}\n").unwrap();
    let program = fixture().to_str().unwrap();
    let log = sandbox.project.path().join("native-rpc.log");
    let log = log.to_str().unwrap();
    let added = sandbox
        .run(
            crate::DEAD_URL,
            &["mcp", "add", "fixture", "--", program, "--modern", log],
            None,
        )
        .await;
    assert!(
        added
            .success()
            .stdout()
            .contains("Added fixture to Palepale (user)")
    );
    let source = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(source.starts_with("# keep my settings\n"));
    assert_eq!(
        toml::from_str::<toml::Value>(&source).unwrap()["mcp_servers"]["fixture"]["command"]
            .as_str(),
        Some(program)
    );
    let duplicate = sandbox
        .run(
            crate::DEAD_URL,
            &["mcp", "add", "fixture", "--url", "https://example.com/mcp"],
            None,
        )
        .await;
    assert!(!duplicate.0.status.success() && duplicate.stderr().contains("already configured"));
    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        source
    );
    let list = sandbox.run(crate::DEAD_URL, &["mcp", "list"], None).await;
    assert!(
        list.success()
            .stdout()
            .contains("fixture  ready  0 tools  user")
    );
    let check = sandbox
        .run(crate::DEAD_URL, &["mcp", "check", "fixture"], None)
        .await;
    assert!(check.success().stdout().contains("fixture: connected"));
    assert!(check.stdout().contains("echo"));
    sandbox
        .run(crate::DEAD_URL, &["mcp", "remove", "fixture"], None)
        .await
        .success();
    let source = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(source.contains("# keep my settings") && source.contains("demo-2"));
    assert!(!source.contains("[mcp_servers.fixture]"));
    assert_eq!(
        std::fs::read_to_string(&codex).unwrap(),
        "# another client\n"
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".claude.json")).unwrap(),
        "{}\n"
    );
    assert!(sandbox.sessions().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_cli_keeps_project_review_and_accepts_literal_server_arguments() {
    let sandbox = Sandbox::default();
    let added = sandbox
        .run(
            crate::DEAD_URL,
            &[
                "mcp",
                "add",
                "project-server",
                "--scope",
                "project",
                "--",
                "server with spaces",
                "--model",
                "a b",
            ],
            None,
        )
        .await;
    assert!(added.success().stdout().contains("Palepale (project)"));
    assert!(!sandbox.home().join("config.toml").exists());
    let config = sandbox.project.path().join(".palepale/config.toml");
    let config: toml::Value = toml::from_str(&std::fs::read_to_string(config).unwrap()).unwrap();
    assert_eq!(
        config["mcp_servers"]["project-server"]["args"][1].as_str(),
        Some("a b")
    );
    let check = sandbox
        .run(crate::DEAD_URL, &["mcp", "check", "project-server"], None)
        .await;
    assert!(!check.0.status.success() && check.stderr().contains("Trust this project"));
    sandbox.trust_project();
    let check = sandbox
        .run(crate::DEAD_URL, &["mcp", "check", "project-server"], None)
        .await;
    assert!(!check.0.status.success() && check.stderr().contains("Review and connect"));
    assert!(!sandbox.home().join("mcp/reviewed.json").exists());
    for arguments in [
        vec![
            "mcp",
            "add",
            "both",
            "--url",
            "https://example.com/mcp",
            "--",
            "program",
        ],
        vec!["mcp", "add", "missing"],
        vec!["mcp", "add", "insecure", "--url", "http://example.com/mcp"],
    ] {
        let invalid = sandbox.run(crate::DEAD_URL, &arguments, None).await;
        assert!(!invalid.0.status.success());
    }
    assert!(!sandbox.home().join("config.toml").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_added_during_a_conversation_is_available_on_the_next_model_request() {
    let sandbox = Sandbox::default();
    std::fs::copy(fixture(), sandbox.home().join("native fixture.exe")).unwrap();
    let fake = Fake::start().await;
    fake.set_home(sandbox.home());
    fake.script([
        Reply::sse("extensions/mcp-install.sse"),
        Reply::sse("extensions/mcp-echo.sse"),
        Reply::sse("extensions/mcp-uninstall.sse"),
        Reply::sse("hello.sse"),
    ]);
    sandbox
        .run(
            &fake.url,
            &[
                "--print",
                "Set up a server in Palepale, verify it, then remove it",
            ],
            WITH_KEY,
        )
        .await
        .success();
    let requests = fake.chat_bodies();
    assert_eq!(requests.len(), 4);
    let tools = |i: usize| {
        requests[i]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect::<Vec<_>>()
    };
    assert!(!tools(0).contains(&"mcp__fixture__echo"));
    assert!(tools(1).contains(&"mcp__fixture__echo"));
    assert!(!tools(3).contains(&"mcp__fixture__echo"));
    let system = requests[0]["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.contains("<palepale_setup>")
            && system.contains(&sandbox.home().join("config.toml").display().to_string())
    );
    let log = std::fs::read_to_string(sandbox.home().join("native-rpc.log")).unwrap();
    assert_eq!(log.lines().filter(|line| *line == "echo").count(), 1);
    assert!(!sandbox.home().join(".codex").exists());
    assert!(!sandbox.home().join(".claude.json").exists());
}

fn fixture() -> &'static Path {
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let binary = dir.path().join(if cfg!(windows) {
                "native MCP fixture.exe"
            } else {
                "native MCP fixture"
            });
            let output = std::process::Command::new("rustc")
                .args(["--edition=2024", "tests/fixtures/mcp_server.rs", "-o"])
                .arg(&binary)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            (dir, binary)
        })
        .1
}

fn setup(sandbox: &Sandbox, large: bool) -> PathBuf {
    let skill = sandbox
        .project
        .path()
        .join(".agents/skills/native-check/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "---\nname: native-check\ndescription: Check the native MCP fixture.\n---\nInspect the native fixture and report its answer.\n").unwrap();
    sandbox.trust_project();
    let log = sandbox.project.path().join("native-rpc.log");
    let mut args = vec!["--modern".to_owned(), log.display().to_string()];
    if large {
        args.push("--large".into());
    }
    let config = json!({"mcp_servers":{"fixture":{"command":fixture(),"args":args,"read_only_tools":["echo"]}}});
    std::fs::write(
        sandbox.home().join("config.toml"),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    log
}

#[tokio::test(flavor = "multi_thread")]
async fn skill_invocation_and_native_mcp_tool_complete_in_one_agent_conversation() {
    let sandbox = Sandbox::default();
    let log = setup(&sandbox, false);
    let fake = Fake::start().await;
    fake.script([
        Reply::sse("extensions/mcp-echo.sse"),
        Reply::sse("hello.sse"),
    ]);
    let run = sandbox
        .run(
            &fake.url,
            &["--mode", "auto", "--print", "$native-check verify"],
            WITH_KEY,
        )
        .await;
    assert!(run.success().stdout().contains("Hello there!"));
    let requests = fake.chat_bodies();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["function"]["name"] == "mcp__fixture__echo")
    );
    assert!(requests[0]["messages"].as_array().unwrap().iter().any(|m| {
        m["content"]
            .as_str()
            .is_some_and(|s| s.contains("Inspect the native fixture and report its answer."))
    }));
    assert!(requests[1]["messages"].as_array().unwrap().iter().any(|m| {
        m["role"] == "tool"
            && m["content"]
                .as_str()
                .is_some_and(|s| s.contains("native fixture") && s.contains("42"))
    }));
    let log = std::fs::read_to_string(log).unwrap();
    assert_eq!(log.lines().filter(|s| *s == "echo").count(), 1);
    assert!(log.contains("no-palepale-key"));
    let transcript = std::fs::read_to_string(&sandbox.sessions()[0]).unwrap();
    assert!(transcript.contains("selected_skill") && transcript.contains("mcp__fixture__echo"));
    assert!(!transcript.contains(crate::fake_palepale::ENV_KEY));
}

#[tokio::test(flavor = "multi_thread")]
async fn deferred_mcp_tools_load_between_model_requests_in_the_native_loop() {
    let sandbox = Sandbox::default();
    setup(&sandbox, true);
    let fake = Fake::start().await;
    fake.script([
        Reply::sse("extensions/mcp-search.sse"),
        Reply::sse("extensions/mcp-echo.sse"),
        Reply::sse("hello.sse"),
    ]);
    sandbox
        .run(
            &fake.url,
            &["--mode", "auto", "--print", "Use the fixture echo tool"],
            WITH_KEY,
        )
        .await
        .success();
    let bodies = fake.chat_bodies();
    assert_eq!(bodies.len(), 3);
    let names = |i: usize| {
        bodies[i]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect::<Vec<_>>()
    };
    assert!(names(0).contains(&"mcp_search"));
    assert!(!names(0).contains(&"mcp__fixture__echo"));
    assert!(names(1).contains(&"mcp__fixture__echo"));
    assert!(bodies[2]["messages"].as_array().unwrap().iter().any(|m| {
        m["role"] == "tool"
            && m["content"]
                .as_str()
                .is_some_and(|s| s.contains("native fixture"))
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_manual_and_plan_modes_do_not_execute_unapproved_mcp_calls() {
    for (mode, reply) in [
        ("manual", "extensions/mcp-echo.sse"),
        ("plan", "extensions/mcp-danger.sse"),
    ] {
        let sandbox = Sandbox::default();
        let log = setup(&sandbox, false);
        let fake = Fake::start().await;
        fake.script([Reply::sse(reply), Reply::sse("hello.sse")]);
        sandbox
            .run(
                &fake.url,
                &["--mode", mode, "--print", "Use the fixture"],
                WITH_KEY,
            )
            .await
            .success();
        let log = std::fs::read_to_string(log).unwrap();
        assert!(
            !log.lines().any(|s| s == "echo" || s == "write"),
            "{mode}: {log}"
        );
        let transcript = std::fs::read_to_string(&sandbox.sessions()[0]).unwrap();
        assert!(transcript.contains(if mode == "manual" {
            "non_interactive"
        } else {
            "denied"
        }));
    }
}

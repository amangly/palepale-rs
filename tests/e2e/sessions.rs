use crate::fake_palepale::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};
use serde_json::{Value, json};
use std::io::Write;

#[tokio::test(flavor = "multi_thread")]
async fn resume_keeps_the_session_and_recovers_unknown_actions_without_replaying_them() {
    let fake = Fake::start().await;
    fake.script([
        Reply::sse("hello.sse"),
        Reply::sse("resume-repeat.sse"),
        Reply::sse("hello.sse"),
    ]);
    let sandbox = Sandbox::default();
    sandbox
        .run(
            &fake.url,
            &["--mode", "auto", "-p", "first objective"],
            WITH_KEY,
        )
        .await
        .success();
    let path = sandbox.sessions().remove(0);
    let before = std::fs::read_to_string(&path).unwrap();
    let last: Value = serde_json::from_str(before.lines().last().unwrap()).unwrap();
    let action = json!({"type":"message","id":"interrupted-call","parent":last["id"],
        "ts":"2026-10-15T01:00:00Z", "message":{"role":"assistant","model":"demo-2","finish":"tool_calls",
        "parts":[{"type":"tool_call","id":"uncertain","name":"bash","arguments":"{\"command\":\"echo repeated > replay-marker\"}"}]}});
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "{action}").unwrap();
    write!(file, "{{\"type\":").unwrap();
    drop(file);
    let resumed = sandbox
        .run(
            &fake.url,
            &["--continue", "-p", "continue the objective"],
            WITH_KEY,
        )
        .await;
    resumed.success();
    assert!(resumed.stderr().contains("direct confirmation"));
    assert_eq!(sandbox.sessions(), vec![path.clone()]);
    assert!(!sandbox.project.path().join("replay-marker").exists());
    let body = fake.chat_bodies().pop().unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message["content"] == "first objective")
    );
    assert!(messages.iter().any(|message| {
        message["role"] == "tool"
            && message["tool_call_id"] == "uncertain"
            && message["content"]
                .as_str()
                .unwrap()
                .contains("outcome is unknown")
    }));
    let records: Vec<Value> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        records
            .iter()
            .any(|record| record["type"] == "state" && record["mode"] == "auto")
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record["message"]["refused"] == "interrupted")
            .count(),
        1
    );
    assert!(
        records
            .iter()
            .any(|record| record["message"]["call_id"] == "repeat-uncertain"
                && record["message"]["refused"] == "unconfirmed")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn local_prompt_defaults_are_selected_separately_and_restored_on_resume() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    std::fs::create_dir_all(sandbox.home().join("prompts")).unwrap();
    std::fs::write(
        sandbox.home().join("prompts/review.md"),
        "LOCAL_PROMPT_MARKER",
    )
    .unwrap();
    sandbox
        .run(&fake.url, &["prompt", "review"], WITH_KEY)
        .await
        .success();
    sandbox
        .run(&fake.url, &["-p", "hello"], WITH_KEY)
        .await
        .success();
    sandbox
        .run(&fake.url, &["prompt", "chat"], WITH_KEY)
        .await
        .success();
    let session_id = sandbox.sessions()[0]
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    sandbox
        .run(
            &fake.url,
            &["--resume", &session_id, "-p", "again"],
            WITH_KEY,
        )
        .await
        .success();
    for body in fake.chat_bodies() {
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .starts_with("LOCAL_PROMPT_MARKER")
        );
        assert!(body.get("agent_profile").is_none());
    }
    let listing = sandbox.run(&fake.url, &["sessions"], WITH_KEY).await;
    assert!(listing.success().stdout().contains(&session_id));
}

#[tokio::test(flavor = "multi_thread")]
async fn incompatible_model_metadata_stops_before_a_paid_request() {
    let fake = Fake::start().await;
    let mut catalog: Value =
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap();
    catalog["data"][0]["capabilities"]["streaming"] = false.into();
    fake.model_lists([catalog]);
    let sandbox = Sandbox::default();
    let run = sandbox.run(&fake.url, &["-p", "hello"], WITH_KEY).await;
    assert_eq!(run.code(), Some(1));
    assert!(run.stderr().contains("streaming"));
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
    assert!(sandbox.sessions().is_empty());
}

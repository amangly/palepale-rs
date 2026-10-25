use serde_json::Value;

use crate::fake_palepale::{Fake, Reply};
use crate::{Sandbox, WITH_KEY};

#[tokio::test(flavor = "multi_thread")]
async fn a_child_uses_restricted_tools_and_returns_to_the_parent() {
    let fake = Fake::start().await;
    fake.script([
        Reply::sse("task.sse"),
        Reply::sse("hello.sse"),
        Reply::sse("hello.sse"),
    ]);
    let sandbox = Sandbox::default();
    let run = sandbox
        .run(
            &fake.url,
            &["--mode", "auto", "--json", "Inspect this project"],
            WITH_KEY,
        )
        .await;
    let output = run.success().stdout();
    assert!(output.contains("\"type\":\"child\""));
    let bodies = fake.chat_bodies();
    assert_eq!(bodies.len(), 3);
    let operations = fake.chat_operations();
    assert_eq!(operations.len(), 3);
    assert!(
        operations
            .iter()
            .all(|key| key.len() == 32 && key.bytes().all(|c| c.is_ascii_hexdigit()))
    );
    assert_eq!(
        operations
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    let tools: Vec<_> = bodies[1]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    assert!(tools.contains(&"read"));
    assert!(!tools.contains(&"write"));
    assert!(!tools.contains(&"bash"));
    assert!(!tools.contains(&"task"));
    assert!(
        bodies[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("Hello there!")))
    );
}

fn chat_only_catalog(fake: &Fake) {
    let mut catalog: Value =
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap();
    for model in catalog["data"].as_array_mut().unwrap() {
        model["capabilities"]["tool_calling"] = false.into();
    }
    fake.model_lists([catalog.clone(), catalog]);
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_only_models_answer_without_tools_and_cannot_execute_unsolicited_calls() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    chat_only_catalog(&fake);
    fake.script([Reply::sse("hello.sse")]);
    let run = sandbox.run(&fake.url, &["-p", "hello"], WITH_KEY).await;
    assert_eq!(run.success().stdout(), "Hello there!\n");
    let bodies = fake.chat_bodies();
    assert!(bodies[0].get("tools").is_none());
    assert!(
        bodies[0]["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("chat only")
    );

    fake.script([Reply::sse("scenarios/spill_read/1.sse")]);
    let run = sandbox
        .run(
            &fake.url,
            &["--mode", "auto", "-p", "read a file"],
            WITH_KEY,
        )
        .await;
    assert_eq!(run.code(), Some(1));
    assert!(
        run.stderr().contains("without advertised tool support"),
        "{}",
        run.stderr()
    );
    assert_eq!(fake.chat_bodies().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn print_streams_the_reply_and_records_the_session() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox
        .run(&fake.url, &["-p", "hi", "there"], WITH_KEY)
        .await;
    assert_eq!(run.success().stdout(), "Hello there!\n");

    let body = &fake.chat_bodies()[0];
    assert_eq!(body["model"], "demo-2");
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    let prompt = body["messages"][1]["content"].as_str().unwrap();
    assert!(prompt.contains("<workspace>"));
    assert!(prompt.ends_with("\n\nhi there"));

    let sessions = sandbox.sessions();
    assert_eq!(sessions.len(), 1);
    let lines: Vec<Value> = std::fs::read_to_string(&sessions[0])
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["type"], "session");
    assert!(lines.iter().any(|line| line["type"] == "state"));
    let reply = &lines
        .iter()
        .find(|line| line["message"]["role"] == "assistant")
        .unwrap()["message"];
    assert_eq!(reply["role"], "assistant");
    assert_eq!(reply["parts"][0]["type"], "reasoning");
    assert_eq!(reply["request_id"], "req_fake");
    // 600 uncached * $6 + 400 cached * $0.60 + 100 out * $10, in micro-dollars
    assert_eq!(reply["usage"]["cost_micros"], 4840);
}

#[tokio::test(flavor = "multi_thread")]
async fn json_mode_emits_one_event_per_line() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["--json", "hi"], WITH_KEY).await;
    let stdout = run.success().stdout();
    for line in stdout.lines() {
        serde_json::from_str::<Value>(line).unwrap_or_else(|e| panic!("{e}: {line}"));
    }
    // The stream is a contract for scripts: its golden changes only on purpose.
    let golden =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/json/hello.jsonl");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(golden.parent().unwrap()).unwrap();
        std::fs::write(&golden, &stdout).unwrap();
    }
    let want = std::fs::read_to_string(&golden).unwrap_or_default();
    assert_eq!(stdout, want, "--json changed; if on purpose, `make golden`");
}

#[tokio::test(flavor = "multi_thread")]
async fn paid_requests_are_never_retried_even_without_output() {
    for (status, kind) in [
        (429, "rate_limited"),
        (503, "model_busy"),
        (503, "upstream_unavailable"),
    ] {
        let fake = Fake::start().await;
        fake.script([
            Reply::Error {
                status,
                kind,
                retry_after: Some(0),
            },
            Reply::sse("hello.sse"),
        ]);
        let sandbox = Sandbox::default();
        let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
        assert_eq!(run.code(), Some(1));
        assert!(!run.stderr().contains("retrying"), "{}", run.stderr());
        assert_eq!(fake.hits("/v1/chat/completions"), 1);
        assert!(!run.stdout().contains("Hello there!"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn billing_errors_fail_with_a_hint_and_no_retry() {
    let fake = Fake::start().await;
    fake.script([Reply::Error {
        status: 402,
        kind: "insufficient_balance",
        retry_after: None,
    }]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
    assert_eq!(run.code(), Some(1));
    let err = run.stderr();
    assert!(err.contains("Out of funds"), "{err}");
    assert!(err.contains("req_fake"), "{err}");
    assert!(err.contains("configured backend"), "{err}");
    assert_eq!(fake.hits("/v1/chat/completions"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_error_whose_body_stalls_is_not_redispatched() {
    let fake = Fake::start().await;
    fake.script([Reply::Stall { status: 503 }, Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
    assert_eq!(run.code(), Some(1));
    assert!(!run.stderr().contains("retrying"), "{}", run.stderr());
    assert_eq!(fake.hits("/v1/chat/completions"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mid_stream_error_keeps_the_partial_reply() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("error_midstream.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
    assert_eq!(run.code(), Some(1));
    assert_eq!(run.stdout(), "Partial answ\n");
    assert!(
        run.stderr().contains("model response failed"),
        "{}",
        run.stderr()
    );
    // Output already streamed (and billed), so it is not retried.
    assert_eq!(fake.hits("/v1/chat/completions"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_length_stop_explains_the_cut() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("length.sse")]);
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], WITH_KEY).await;
    assert!(run.success().stderr().contains("output limit"));
}

#[tokio::test(flavor = "multi_thread")]
async fn truncated_tool_calls_record_refusals_and_complete_json_events_without_writing_files() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("truncated-tools.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let run = sandbox
        .run(&fake.url, &["--json", "--mode", "auto", "do it"], WITH_KEY)
        .await;
    assert_eq!(run.code(), Some(1));
    assert!(run.stderr().contains("output limit"));
    let stdout = run.stdout();
    let events: Vec<Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for id in ["cut-a", "cut-b"] {
        let starts: Vec<_> = events
            .iter()
            .filter(|e| e["type"] == "tool_start" && e["call_id"] == id)
            .collect();
        let ends: Vec<_> = events
            .iter()
            .filter(|e| e["type"] == "tool_end" && e["result"]["call_id"] == id)
            .collect();
        assert_eq!(starts.len(), 1);
        assert_eq!(ends.len(), 1);
        assert_eq!(ends[0]["result"]["refused"], "truncated_tool_call");
        assert_eq!(ends[0]["result"]["is_error"], true);
    }
    for file in ["cut-a.txt", "cut-b.txt"] {
        assert!(!sandbox.project.path().join(file).exists());
    }
    let session = std::fs::read_to_string(&sandbox.sessions()[0]).unwrap();
    let records: Vec<Value> = session
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let results: Vec<_> = records
        .iter()
        .filter(|r| r["message"]["role"] == "tool")
        .collect();
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|r| r["message"]["refused"] == "truncated_tool_call")
    );
    let bodies = fake.chat_bodies();
    assert_eq!(bodies.len(), 1);
}
#[tokio::test(flavor = "multi_thread")]
async fn reasoning_uses_the_server_default() {
    let fake = Fake::start().await;
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();

    sandbox
        .run(&fake.url, &["-p", "hi"], WITH_KEY)
        .await
        .success();
    assert!(fake.chat_bodies()[0].get("reasoning_effort").is_none());

    // Neither reasoning nor chat-only models need a client-supplied level.
    let args = ["--model", "fake-coder", "--json", "-p", "hi"];
    let run = sandbox.run(&fake.url, &args, WITH_KEY).await;
    assert!(fake.chat_bodies()[1].get("reasoning_effort").is_none());
    let end = run
        .success()
        .stdout()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .find(|e| e["type"] == "message_end")
        .unwrap();
    assert_eq!(end["message"]["effort"], Value::Null, "{end}");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_models_are_refused_before_spending() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();

    let run = sandbox
        .run(&fake.url, &["--model", "nope", "-p", "hi"], WITH_KEY)
        .await;
    assert_eq!(run.code(), Some(1));
    assert!(run.stderr().contains("unknown model"), "{}", run.stderr());
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn without_a_key_it_says_how_to_log_in() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();

    let run = sandbox.run(&fake.url, &["-p", "hi"], None).await;
    assert_eq!(run.code(), Some(1));
    assert!(
        run.stderr().contains("PALEPALE_API_KEY"),
        "{}",
        run.stderr()
    );
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_flag_after_the_prompt_is_refused_not_sent() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let run = sandbox
        .run(&fake.url, &["-p", "hi", "--json"], WITH_KEY)
        .await;
    assert_eq!(run.code(), Some(2), "{}", run.stderr());
    assert!(
        run.stderr().contains("`--json` goes before the words"),
        "{}",
        run.stderr()
    );
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_command_on_the_command_line_names_where_it_works() {
    let fake = Fake::start().await;
    let run = Sandbox::default().run(&fake.url, &["mode"], WITH_KEY).await;
    assert_ne!(run.code(), Some(0));
    assert!(
        run.stderr().contains("`mode` only works as `/mode`"),
        "{}",
        run.stderr()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_prompt_that_starts_with_a_command_name_is_not_dropped() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let run = sandbox
        .run(&fake.url, &["help", "me", "fix", "the", "build"], WITH_KEY)
        .await;
    assert_eq!(run.code(), Some(1));
    assert!(
        run.stderr().contains("usage: palepale help"),
        "{}",
        run.stderr()
    );
    assert!(!run.stdout().contains("Commands:"), "{}", run.stdout());
}

#[tokio::test(flavor = "multi_thread")]
async fn help_names_the_commands_and_what_print_does_with_them() {
    let fake = Fake::start().await;
    let run = Sandbox::default().run(&fake.url, &["--help"], None).await;
    let out = run.success().stdout();
    assert!(
        out.contains("Commands: help,") && out.contains(", models,"),
        "{out}"
    );
    assert!(
        out.contains(", login,") && out.contains(", logout,") && out.contains(", signup,"),
        "{out}"
    );
    assert!(out.contains("never a command"), "{out}");
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_development_build_says_it_has_no_release_to_update_from() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let update = sandbox.run(&fake.url, &["update"], None).await;
    assert_eq!(update.code(), Some(1));
    assert!(
        update.stderr().contains("debug builds"),
        "{}",
        update.stderr()
    );
    let version = sandbox.run(&fake.url, &["--version"], None).await;
    let out = version.success().stdout();
    assert_eq!(
        out.trim(),
        format!("palepale {}", env!("CARGO_PKG_VERSION"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_with_nothing_to_send_says_so() {
    let fake = Fake::start().await;
    let run = Sandbox::default().run(&fake.url, &["-p"], WITH_KEY).await;
    assert_ne!(run.code(), Some(0));
    assert!(run.stderr().contains("nothing to send"), "{}", run.stderr());
    assert_eq!(fake.hits("/v1/chat/completions"), 0);
}

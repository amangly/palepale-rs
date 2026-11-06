use crate::fake_palepale::{Fake, Reply};
use crate::{DEAD_URL, Sandbox};

#[tokio::test(flavor = "multi_thread")]
async fn agent_selects_native_search_with_same_model_and_budget() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let mut listing: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap();
    listing["data"][0]["capabilities"]["web_search"] = true.into();
    listing["data"][0]["pricing"]["web_search_per_request_usd"] = "0.008166667".into();
    let selected = listing["data"][0]["id"].as_str().unwrap().to_owned();
    fake.model_lists([listing.clone()]);
    fake.script([Reply::sse("websearch.sse"), Reply::Json(serde_json::json!({"choices":[{"message":{"content":"Search answer","annotations":[{"type":"url_citation","url_citation":{"url":"https://example.com/"}}]}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"server_tool_use_details":{"web_search_requests":1}}})), Reply::sse("hello.sse")]);
    let result = sandbox
        .run(
            &fake.url,
            &[
                "--model",
                &selected,
                "--budget",
                "1.00",
                "--json",
                "Find current Bun documentation",
            ],
            Some("test-key"),
        )
        .await;
    assert!(result.success().stdout().contains("https://example.com/"));
    let output = result.stdout();
    let events: Vec<serde_json::Value> = output
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let search = events
        .iter()
        .find(|event| event["type"] == "tool_end" && event["result"]["name"] == "websearch")
        .unwrap();
    assert!(
        search["result"]["details"]["usage"]["cost_micros"]
            .as_u64()
            .unwrap()
            >= 8167
    );
    let bodies = fake.chat_bodies();
    assert_eq!(bodies.len(), 3);
    assert!(bodies.iter().all(|body| body["model"] == selected));
    assert!(
        bodies
            .iter()
            .all(|body| body["budget_id"] == bodies[0]["budget_id"])
    );
    assert!(
        bodies[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "websearch")
    );
    assert_eq!(
        bodies[1]["web_search_options"]["search_context_size"],
        "low"
    );
    assert!(bodies[1].get("tools").is_none());
    assert!(bodies[1].get("max_tokens").is_none());
    assert!(bodies[1].get("max_completion_tokens").is_none());
    assert!(
        bodies[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("https://example.com/")))
    );
    listing["data"][0]["capabilities"]["web_search"] = false.into();
    fake.model_lists([listing]);
    fake.script([Reply::sse("hello.sse")]);
    let sandbox = Sandbox::default();
    let result = sandbox
        .run(
            &fake.url,
            &["--model", &selected, "-p", "Hello"],
            Some("test-key"),
        )
        .await;
    result.success();
    assert!(
        !fake.chat_bodies()[3]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "websearch")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn native_search_failure_is_not_retried() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let mut listing: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap();
    listing["data"][0]["capabilities"]["web_search"] = true.into();
    let selected = listing["data"][0]["id"].as_str().unwrap().to_owned();
    fake.model_lists([listing]);
    fake.script([
        Reply::sse("websearch.sse"),
        Reply::Error {
            status: 503,
            kind: "unavailable",
            retry_after: Some(0),
        },
        Reply::sse("hello.sse"),
    ]);
    sandbox
        .run(
            &fake.url,
            &["--model", &selected, "-p", "Find current docs"],
            Some("test-key"),
        )
        .await
        .success();
    assert_eq!(
        fake.chat_bodies()
            .iter()
            .filter(|body| body.get("web_search_options").is_some())
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn renamed_saved_model_resolves_and_public_presentation_is_visible() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    let old = "deepseek-v4.1-flash-uncensored-standard";
    let current = "deepseek-v4.1-flash-uncensored-zero-refusal";
    let mut listing: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap();
    let mut model = listing["data"][0].clone();
    model["id"] = current.into();
    model.as_object_mut().unwrap().remove("display_name");
    model["presentation"] = serde_json::json!({"name":"DeepSeek V4.1 Flash 1017 Uncensored","badges":["Uncensored","Zero Refusal"]});
    listing["data"] = serde_json::json!([model]);
    fake.model_lists([listing.clone(), listing.clone(), listing]);
    fake.script([Reply::sse("hello.sse"), Reply::sse("hello.sse")]);
    std::fs::write(
        sandbox.home().join("prefs.json"),
        serde_json::json!({"model":old}).to_string(),
    )
    .unwrap();
    let result = sandbox.run(&fake.url, &["models"], None).await;
    let out = result.success().stdout();
    assert!(out.contains("DeepSeek V4.1 Flash 1017 Uncensored"), "{out}");
    assert!(out.contains(&format!("{current} ●")), "{out}");
    assert!(!out.contains("not in this list"), "{out}");
    let prefs: serde_json::Value =
        serde_json::from_slice(&std::fs::read(sandbox.home().join("prefs.json")).unwrap()).unwrap();
    assert_eq!(prefs["model"], current);
    let cached = sandbox.run(&fake.url, &["models"], None).await;
    assert!(
        cached
            .success()
            .stdout()
            .contains("DeepSeek V4.1 Flash 1017 Uncensored")
    );
    sandbox
        .run(&fake.url, &["-p", "hi"], Some("test-key"))
        .await
        .success();
    assert_eq!(fake.chat_bodies()[0]["model"], current);
    sandbox
        .run(&fake.url, &["--model", old, "-p", "hi"], Some("test-key"))
        .await
        .success();
    assert_eq!(fake.chat_bodies()[1]["model"], current);
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_live_models_then_serves_them_from_cache() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();

    let first = sandbox.run(&fake.url, &["models"], None).await;
    let out = first.success().stdout();
    assert!(out.contains("fake-coder"), "{out}");
    assert!(out.contains("Live list"), "{out}");
    assert!(sandbox.home().join("cache/models.json").exists());

    let second = sandbox.run(&fake.url, &["models"], None).await;
    assert!(second.success().stdout().contains("Cached list"));
    assert_eq!(fake.hits("/v1/models"), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_bypasses_a_fresh_cache() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.run(&fake.url, &["models"], None).await.success();
    let out = sandbox.run(&fake.url, &["models", "--refresh"], None).await;
    assert!(out.success().stdout().contains("Live list"));
    assert_eq!(fake.hits("/v1/models"), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_different_cli_release_refreshes_cached_capabilities() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.run(&fake.url, &["models"], None).await.success();
    let path = sandbox.home().join("cache/models.json");
    let mut cache: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    cache["release"] = "v0.0.0-old".into();
    cache["data"][0]["capabilities"]["tool_calling"] = false.into();
    std::fs::write(&path, cache.to_string()).unwrap();
    let run = sandbox.run(&fake.url, &["models"], None).await;
    assert!(run.success().stdout().contains("Live list"));
    assert_eq!(fake.hits("/v1/models"), 2);
    let cache: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(cache["data"][0]["capabilities"]["tool_calling"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_reports_no_models_without_a_cache() {
    let sandbox = Sandbox::default();
    let run = sandbox.run(DEAD_URL, &["models"], None).await;
    let out = run.success().stdout();
    assert!(out.contains("No models available"), "{out}");
    assert!(!out.contains("demo-2"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_live_list_removes_previously_cached_models() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    fake.model_lists([
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap(),
        serde_json::json!({"data": []}),
    ]);
    sandbox.run(&fake.url, &["models"], None).await.success();
    let out = sandbox.run(&fake.url, &["models", "--refresh"], None).await;
    let out = out.success().stdout();
    assert!(!out.contains("fake-coder"), "{out}");
    assert!(out.contains("Live list"), "{out}");
    let cached = sandbox.run(&fake.url, &["models"], None).await;
    assert!(!cached.success().stdout().contains("fake-coder"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cache_from_another_api_is_ignored() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    sandbox.run(&fake.url, &["models"], None).await.success();
    let out = sandbox.run(DEAD_URL, &["models"], None).await;
    let out = out.success().stdout();
    assert!(out.contains("No models available"), "{out}");
    assert!(!out.contains("demo-2"), "{out}");
}

use crate::fake_palepale::Fake;
use crate::{DEAD_URL, Sandbox};

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
async fn an_unusable_live_list_uses_same_origin_cache() {
    let fake = Fake::start().await;
    let sandbox = Sandbox::default();
    fake.model_lists([
        serde_json::from_str(include_str!("../fixtures/api/models.json")).unwrap(),
        serde_json::json!({"data": []}),
    ]);
    sandbox.run(&fake.url, &["models"], None).await.success();
    let out = sandbox.run(&fake.url, &["models", "--refresh"], None).await;
    let out = out.success().stdout();
    assert!(out.contains("fake-coder"), "{out}");
    assert!(out.contains("Could not reach palepale"), "{out}");
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

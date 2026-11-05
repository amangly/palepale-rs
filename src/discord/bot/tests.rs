use super::*;
use crate::api::models::{self, Source};
use axum::{Router, body::Body, response::IntoResponse};
use serde_json::{Value, json};
use tokio::sync::Notify;

#[derive(Clone, Copy)]
enum Reply {
    Normal,
    Stalled,
    Truncated,
    Tool,
    Long,
}

struct Fixture {
    bot: Arc<Bot>,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
    opened: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.bot.cancel.cancel();
        self.server.abort();
    }
}
impl Fixture {
    async fn new(reply: Reply) -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let opened = Arc::new(Notify::new());
        let recorded = calls.clone();
        let ready = opened.clone();
        let app = Router::new().fallback(move |req: axum::extract::Request| {
            let recorded = recorded.clone();
            let ready = ready.clone();
            async move {
                let path = req.uri().path().to_owned();
                let body = axum::body::to_bytes(req.into_body(), 1024 * 1024).await.unwrap();
                let value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                recorded.lock().await.push((path.clone(), value));
                if path == "/v1/account/run-budgets" {
                    return axum::Json(json!({"id":"12345678-1234-1234-1234-123456789abc"})).into_response();
                }
                if path == "/v1/chat/completions" {
                    ready.notify_one();
                    let content = if matches!(reply, Reply::Long) { "a".repeat(30000) } else { "Hello @everyone".into() };
                    let delta = if matches!(reply, Reply::Tool) {
                        json!({"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"bash","arguments":"{}"}}]})
                    } else { json!({"content":content}) };
                    let mut sse = format!("data: {}\n\n", json!({"choices":[{"index":0,"delta":delta}]}));
                    if !matches!(reply, Reply::Truncated | Reply::Stalled) {
                        let finish = if matches!(reply, Reply::Tool) { "tool_calls" } else { "stop" };
                        sse.push_str(&format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"index":0,"delta":{},"finish_reason":finish}],"usage":{"prompt_tokens":10,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":2}}})));
                    }
                    let body = if matches!(reply, Reply::Stalled) {
                        use futures_util::StreamExt;
                        Body::from_stream(futures_util::stream::once(async move { Ok::<_, std::io::Error>(sse) }).chain(futures_util::stream::pending()))
                    } else { Body::from(sse) };
                    return ([("content-type", "text/event-stream")], body).into_response();
                }
                if path.ends_with("/callback") { return axum::http::StatusCode::NO_CONTENT.into_response(); }
                axum::Json(json!({"id":"900","type":0})).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut api = Api::redirected(
            io::http::Http::new(Duration::from_secs(2)).unwrap(),
            "fixture-key",
            &base,
        );
        api.set_budget("2".into());
        let config = Config {
            owner: "1".into(),
            allow_users: vec!["2".into()],
            model: "demo-2".into(),
            ..data().defaults.clone()
        };
        let catalog = Catalog {
            models: models::fixture(),
            source: Source::Live,
            fetched_at: None,
            note: None,
        };
        let bot = Arc::new(Bot::new(
            api,
            Discord::fixture(base),
            config,
            catalog,
            "9".into(),
            CancellationToken::new(),
        ));
        Self {
            bot,
            calls,
            opened,
            server,
        }
    }
    async fn chats(&self) -> Vec<Value> {
        self.calls
            .lock()
            .await
            .iter()
            .filter(|(p, _)| p == "/v1/chat/completions")
            .map(|(_, v)| v.clone())
            .collect()
    }
    async fn displayed(&self) -> Vec<String> {
        self.calls
            .lock()
            .await
            .iter()
            .filter_map(|(_, v)| v["content"].as_str().map(str::to_owned))
            .collect()
    }
}
fn message(id: &str, user: &str, text: &str) -> Incoming {
    crate::discord::incoming(
        "MESSAGE_CREATE",
        &json!({"id":id,"channel_id":"10","author":{"id":user},"content":text}),
        "9",
    )
    .unwrap()
}

#[tokio::test]
async fn paid_requests_are_deduplicated_authorized_isolated_and_budgeted() {
    let f = Fixture::new(Reply::Normal).await;
    f.bot
        .handle(message("100", "3", "not allowed"))
        .await
        .unwrap();
    assert!(f.calls.lock().await.is_empty());
    f.bot
        .handle(message("101", "1", "private first message"))
        .await
        .unwrap();
    f.bot
        .handle(message("101", "1", "duplicate"))
        .await
        .unwrap();
    f.bot
        .handle(message("102", "2", "separate conversation"))
        .await
        .unwrap();
    f.bot
        .handle(message("103", "1", "follow up"))
        .await
        .unwrap();
    let chats = f.chats().await;
    assert_eq!(chats.len(), 3);
    assert_eq!(
        chats[0]["budget_id"],
        "12345678-1234-1234-1234-123456789abc"
    );
    assert_eq!(chats[0]["stream"], true);
    assert!(chats[0]["tools"].is_null() || chats[0]["tools"] == json!([]));
    assert!(!chats[1].to_string().contains("private first message"));
    assert_eq!(chats[2]["messages"].as_array().unwrap().len(), 4);
    let state = f.bot.state.lock().await;
    assert_eq!(state.sessions["dm:10:1"].usage.unwrap().output, 6);
    assert_eq!(state.sessions["dm:10:2"].messages.len(), 2);
    drop(state);
    let calls = f.calls.lock().await;
    assert_eq!(
        calls
            .iter()
            .filter(|(p, _)| p.ends_with("run-budgets"))
            .count(),
        1
    );
    for (_, value) in calls.iter().filter(|(_, v)| v["content"].is_string()) {
        assert_eq!(value["allowed_mentions"]["parse"], json!([]));
    }
}

#[tokio::test]
async fn slash_ask_is_acknowledged_before_a_paid_request() {
    let f = Fixture::new(Reply::Normal).await;
    let event = crate::discord::incoming("INTERACTION_CREATE", &json!({"id":"100","type":2,"application_id":"9","token":"fixture-interaction","channel_id":"10","user":{"id":"1"},"data":{"name":"palepale","options":[{"name":"ask","options":[{"name":"message","value":"Hi"}]}]}}), "9").unwrap();
    f.bot.handle(event).await.unwrap();
    let calls = f.calls.lock().await;
    assert!(calls[0].0.ends_with("/callback"));
    assert_eq!(calls[0].1["type"], 5);
    assert!(
        calls
            .iter()
            .any(|(p, v)| p.ends_with("/@original") && v["content"] == "Hello @everyone")
    );
}

#[tokio::test]
async fn stop_cancels_without_replay_and_reset_cannot_erase_an_active_slot() {
    let mut f = Fixture::new(Reply::Stalled).await;
    Arc::get_mut(&mut f.bot).unwrap().config.max_concurrent = 1;
    let bot = f.bot.clone();
    let task = tokio::spawn(async move {
        bot.handle(message("100", "1", "slow")).await.unwrap();
    });
    tokio::time::timeout(Duration::from_secs(5), f.opened.notified())
        .await
        .unwrap();
    f.bot.handle(message("101", "1", "overlap")).await.unwrap();
    f.bot
        .handle(message("102", "2", "another user"))
        .await
        .unwrap();
    assert!(
        f.displayed()
            .await
            .iter()
            .any(|s| s == &data().text.capacity)
    );
    assert_eq!(
        f.bot.control("dm:10:1", "reset", "").await,
        data().text.busy
    );
    // Another user's stop cannot cancel this conversation.
    assert_eq!(f.bot.control("dm:10:2", "stop", "").await, data().text.idle);
    assert_eq!(
        f.bot.control("dm:10:1", "stop", "").await,
        data().text.stopping
    );
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.chats().await.len(), 1);
    let state = f.bot.state.lock().await;
    assert!(state.sessions["dm:10:1"].active.is_none());
    assert!(state.sessions["dm:10:1"].messages.is_empty());
    drop(state);
    assert!(
        f.displayed()
            .await
            .iter()
            .any(|s| s.contains(&data().text.stopped))
    );
    assert_eq!(
        f.bot.control("dm:10:1", "reset", "").await,
        data().text.reset
    );
}

#[tokio::test]
async fn broken_streams_and_unsolicited_tools_never_dispatch_again() {
    for (reply, warning) in [
        (Reply::Truncated, &data().text.failed),
        (Reply::Tool, &data().text.unsupported),
        (Reply::Long, &data().text.limit),
    ] {
        let f = Fixture::new(reply).await;
        f.bot.handle(message("100", "1", "Hi")).await.unwrap();
        assert_eq!(f.chats().await.len(), 1);
        assert!(
            f.bot.state.lock().await.sessions["dm:10:1"]
                .messages
                .is_empty()
        );
        let displayed = f.displayed().await;
        assert!(
            displayed.iter().any(|s| s.contains(warning)),
            "{displayed:?}"
        );
        assert!(
            displayed
                .iter()
                .all(|s| s.encode_utf16().count() <= data().message_units)
        );
    }
}

#[tokio::test]
async fn guild_mentions_are_required_and_session_eviction_keeps_active_turns() {
    let mut f = Fixture::new(Reply::Normal).await;
    Arc::get_mut(&mut f.bot).unwrap().config.max_sessions = 1;
    let mut event = message("100", "1", "not addressed to bot");
    event.guild = Some("50".into());
    f.bot.handle(event).await.unwrap();
    assert!(f.chats().await.is_empty());
    let mut event = message("101", "1", "<@9> Hello");
    event.guild = Some("50".into());
    event.mentioned = true;
    f.bot.handle(event).await.unwrap();
    assert_eq!(f.chats().await.len(), 1);
    let (_, _, active) = f.bot.begin("dm:10:2").await.unwrap();
    assert!(!f.bot.state.lock().await.sessions.contains_key("50:10:1"));
    assert!(f.bot.begin("dm:10:1").await.is_err());
    assert!(!active.is_cancelled());
    assert_eq!(f.bot.state.lock().await.sessions.len(), 1);
}

#[test]
fn history_eviction_keeps_complete_turns() {
    let mut messages = vec![
        Message::user_text("a"),
        Message::user_text("b"),
        Message::user_text("c"),
        Message::user_text("d"),
    ];
    prune(&mut messages, 1, 1000);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0], Message::user_text("c"));
    prune(&mut messages, 1, 1);
    assert!(messages.is_empty());
}

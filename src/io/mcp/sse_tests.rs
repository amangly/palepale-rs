use super::*;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use futures_util::StreamExt;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct Fixture {
    sender: Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Sender<String>>>>,
    messages: Arc<AtomicUsize>,
    endpoint: String,
    connections: Arc<AtomicUsize>,
    resumed: Arc<AtomicUsize>,
}

async fn events(State(f): State<Fixture>, headers: HeaderMap) -> axum::response::Response {
    if headers.get("x-service").and_then(|v| v.to_str().ok()) != Some("fixture")
        || headers.get("authorization").and_then(|v| v.to_str().ok())
            != Some("Bearer fixture-token")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    f.connections.fetch_add(1, Ordering::SeqCst);
    if headers.get("last-event-id").and_then(|h| h.to_str().ok()) == Some("fixture-last") {
        f.resumed.fetch_add(1, Ordering::SeqCst);
    }
    let (sender, receiver) = tokio::sync::mpsc::channel::<String>(16);
    *f.sender.lock().unwrap() = Some(sender);
    let first = futures_util::stream::once(async move {
        Ok::<_, std::convert::Infallible>(Event::default().event("endpoint").data(f.endpoint))
    });
    let rest = futures_util::stream::unfold(receiver, |mut r| async {
        r.recv().await.map(|s| {
            (
                Ok::<_, std::convert::Infallible>(Event::default().id("fixture-last").data(s)),
                r,
            )
        })
    });
    Sse::new(first.chain(rest)).into_response()
}

async fn message(
    State(f): State<Fixture>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> StatusCode {
    assert_eq!(headers["x-service"], "fixture");
    assert_eq!(headers["authorization"], "Bearer fixture-token");
    f.messages.fetch_add(1, Ordering::SeqCst);
    if request.get("id").is_none() {
        return StatusCode::ACCEPTED;
    }
    let result = match request["method"].as_str().unwrap() {
        "initialize" => {
            json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"legacy-sse","version":"1"}})
        }
        "tools/list" => {
            json!({"tools":[{"name":"echo","inputSchema":{"type":"object","properties":{}}}]})
        }
        "tools/call" => json!({"content":[{"type":"text","text":"SSE works"}]}),
        _ => json!({}),
    };
    let sender = f.sender.lock().unwrap().clone().unwrap();
    sender
        .send(json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string())
        .await
        .unwrap();
    StatusCode::ACCEPTED
}

async fn start(endpoint: &str) -> (Fixture, String) {
    let state = Fixture {
        sender: Arc::default(),
        messages: Arc::default(),
        endpoint: endpoint.into(),
        connections: Arc::default(),
        resumed: Arc::default(),
    };
    let router = Router::new()
        .route("/sse", get(events))
        .route("/messages", post(message))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/sse", listener.local_addr().unwrap());
    tokio::spawn(async {
        axum::serve(listener, router).await.unwrap();
    });
    (state, url)
}

fn config(url: String) -> Config {
    Config {
        url: Some(url),
        oauth: false,
        http_headers: [
            ("X-Service".into(), "fixture".into()),
            ("Authorization".into(), "Bearer fixture-token".into()),
        ]
        .into(),
        startup_timeout_secs: Some(3),
        ..Default::default()
    }
}

#[tokio::test]
async fn legacy_sse_resumes_the_receive_stream_without_reposting_completed_requests() {
    let (state, url) = start("/messages").await;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(url);
    cfg.transport = crate::mcp::Transport::Sse;
    let connection = Arc::new(
        Connection::open(&cfg, dir.path(), dir.path().join("auth"))
            .await
            .unwrap(),
    );
    connection.tools().await.unwrap();
    let messages = state.messages.load(Ordering::SeqCst);
    state.sender.lock().unwrap().take();
    tokio::time::timeout(Duration::from_secs(3), async {
        while state.connections.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(state.messages.load(Ordering::SeqCst), messages);
    assert_eq!(state.resumed.load(Ordering::SeqCst), 1);
    let result = connection
        .clone()
        .call(
            "echo".into(),
            json!({}),
            Duration::from_secs(2),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "SSE works");
    connection.close();
}

#[tokio::test]
async fn http_negotiation_falls_back_to_legacy_sse_and_preserves_headers_on_posts() {
    let (state, url) = start("/messages").await;
    let directory = tempfile::tempdir().unwrap();
    let connection = Arc::new(
        Connection::open(
            &config(url),
            directory.path(),
            directory.path().join("auth"),
        )
        .await
        .unwrap(),
    );
    assert_eq!(connection.tools().await.unwrap().len(), 1);
    let result = connection
        .clone()
        .call(
            "echo".into(),
            json!({}),
            Duration::from_secs(2),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "SSE works");
    assert_eq!(state.messages.load(Ordering::SeqCst), 4);
    connection.close();
}

#[tokio::test]
async fn sse_refuses_cross_origin_message_endpoints_and_stops_without_leaking_headers() {
    let (state, url) = start("https://example.com/steal").await;
    let directory = tempfile::tempdir().unwrap();
    let mut config = config(url);
    config.transport = crate::mcp::Transport::Sse;
    let error = Connection::open(&config, directory.path(), directory.path().join("auth"))
        .await
        .err()
        .unwrap();
    assert!(error.message.contains("share the server origin"));
    assert_eq!(state.messages.load(Ordering::SeqCst), 0);
}

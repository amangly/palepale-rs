use super::*;

#[tokio::test]
async fn rest_bounds_retries_handles_204_and_never_repeats_ambiguous_writes() {
    use axum::{Router, response::IntoResponse};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let calls = attempts.clone();
    let app = Router::new().fallback(move |req: axum::extract::Request| {
        let calls = calls.clone();
        async move {
            assert_eq!(req.headers()["authorization"], "Bot fixture-discord-token");
            if req.uri().path() == "/retry" && calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return (
                    axum::http::StatusCode::TOO_MANY_REQUESTS,
                    "{\"retry_after\":0.01}",
                )
                    .into_response();
            }
            if req.uri().path().contains("private-token") {
                calls.fetch_add(1, Ordering::SeqCst);
                return (axum::http::StatusCode::BAD_GATEWAY, "secret response").into_response();
            }
            axum::http::StatusCode::NO_CONTENT.into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Discord::fixture(format!("http://{}", listener.local_addr().unwrap()));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    assert!(
        client
            .request(Method::POST, "/retry", None)
            .await
            .unwrap()
            .is_null()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let error = client
        .request(Method::POST, "/private-token", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("private-token"));
    assert!(!error.contains("secret response"));
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    server.abort();
}

#[tokio::test]
async fn gateway_identifies_routes_then_resumes_without_identifying_again() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for resumed in [false, true] {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            ws.send(Message::Text(
                json!({"op":10,"d":{"heartbeat_interval":1000}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            let packet: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(packet["op"], if resumed { 6 } else { 2 });
            if resumed {
                assert_eq!(packet["d"]["session_id"], "fixture-session");
                assert_eq!(packet["d"]["seq"], 2);
            } else {
                assert_eq!(packet["d"]["intents"], 37377);
                ws.send(Message::Text(json!({"op":0,"t":"READY","s":1,"d":{"session_id":"fixture-session","user":{"id":"9"}}}).to_string().into())).await.unwrap();
                ws.send(Message::Text(json!({"op":0,"t":"MESSAGE_CREATE","s":2,"d":{"id":"1","channel_id":"2","author":{"id":"3"},"content":"<@9> Hi","mentions":[{"id":"9"}]}}).to_string().into())).await.unwrap();
            }
            ws.send(Message::Text(json!({"op":7}).to_string().into()))
                .await
                .unwrap();
        }
    });
    let (tx, mut rx) = mpsc::channel(4);
    let mut resume = Resume::default();
    let client = Discord::new("fixture-token".into()).unwrap();
    for _ in 0..2 {
        assert!(matches!(
            client.connection(&url, &mut resume, &tx).await,
            Err(GatewayError::Reconnect)
        ));
    }
    assert!(rx.recv().await.unwrap().mentioned);
    assert!(rx.try_recv().is_err());
    server.await.unwrap();
}

#[tokio::test]
async fn missing_heartbeat_ack_reconnects_and_fatal_codes_stop() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        ws.send(Message::Text(
            json!({"op":10,"d":{"heartbeat_interval":20}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        while ws.next().await.is_some_and(|r| r.is_ok()) {}
    });
    let (tx, _) = mpsc::channel(1);
    let mut resume = Resume::default();
    let client = Discord::new("fixture-token".into()).unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        client.connection(&url, &mut resume, &tx),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(GatewayError::Reconnect)));
    assert!(matches!(
        close(Some(4004), &mut resume),
        GatewayError::Fatal(_)
    ));
    assert!(resume_url("wss://evil.example/gateway").is_none());
    assert!(resume_url("wss://gateway-us-east1-b.discord.gg/").is_some());
    server.await.unwrap();
}

#[test]
fn token_is_protected_and_model_mentions_cannot_ping() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    save_token(&path, "fixture-discord-secret").unwrap();
    let text = super::super::fs::read_string(&path).unwrap().unwrap();
    assert!(!text.contains("fixture-discord-secret"));
    let stored: Stored = serde_json::from_str(&text).unwrap();
    assert_eq!(
        super::super::secret::open(&stored.scheme, &STANDARD.decode(stored.secret).unwrap())
            .unwrap(),
        b"fixture-discord-secret"
    );
    assert_eq!(
        message_body("@everyone")["allowed_mentions"]["parse"],
        json!([])
    );
}

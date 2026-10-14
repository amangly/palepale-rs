use super::*;
use crate::app::App;
use crate::extensions::Scope;
use crate::permission::gate::{Decision, Policy};
use crate::permission::rule::{Action, Rule};
use crate::tools::Registry;
use crate::tools::testing::Project;
use std::collections::BTreeMap;

fn fixture() -> &'static Path {
    static FIXTURE: std::sync::OnceLock<(tempfile::TempDir, std::path::PathBuf)> =
        std::sync::OnceLock::new();
    &FIXTURE
        .get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            let binary = directory.path().join(if cfg!(windows) {
                "fixture.exe"
            } else {
                "fixture"
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
            (directory, binary)
        })
        .1
}

fn config(modern: bool, log: &Path) -> Config {
    Config {
        command: Some(fixture().display().to_string()),
        args: vec![
            if modern { "--modern" } else { "--legacy" }.into(),
            log.display().to_string(),
        ],
        read_only_tools: vec!["echo".into()],
        ..Config::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn slash_prompt_reads_obey_the_active_permission_policy() {
    let mut project = Project::new();
    let log = project.app.paths.project.join("prompt-denied.log");
    let mut config = config(true, &log);
    config.args.push("--catalogs".into());
    crate::mcp::save(&project.app, "fixture", &config, Scope::User).unwrap();
    project
        .app
        .mcp
        .connect(&project.app, "fixture", false)
        .await
        .unwrap();
    project.app.settings.permission.push(Rule {
        permission: "mcp".into(),
        pattern: "fixture.prompts/get".into(),
        action: Action::Deny,
    });
    let output = crate::commands::discovered::prompt(
        json!({"server":"fixture","action":"get","name":"review","arguments":{"topic":"native"}}),
        &project.cx(),
    )
    .await;
    assert!(
        output.is_error
            && !output
                .content
                .contains("Review native fixture instructions")
    );
    assert!(
        !io::fs::read_string(&log)
            .unwrap()
            .unwrap()
            .contains("prompts/get")
    );
    project.app.mcp.disconnect("fixture");
}

#[tokio::test(flavor = "multi_thread")]
async fn tool_list_changes_refresh_the_existing_process_and_invalidate_old_definitions() {
    let project = Project::new();
    let log = project.app.paths.project.join("changed.log");
    let mut config = config(true, &log);
    config.args.push("--changes".into());
    crate::mcp::save(&project.app, "fixture", &config, Scope::User).unwrap();
    project
        .app
        .mcp
        .connect(&project.app, "fixture", false)
        .await
        .unwrap();
    let before = Registry::builtin(&project.app).snapshot();
    let echo = before.get("mcp__fixture__echo").unwrap();
    let result = echo.run(json!({"x":42}), &project.cx()).await;
    assert!(!result.is_error, "{}", result.content);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            project.app.mcp.refresh_changed(&project.app).await;
            if Registry::builtin(&project.app)
                .snapshot()
                .get("mcp__fixture__updated")
                .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(echo.check(&json!({"x":42}), &project.cx()).is_err());
    let log = io::fs::read_string(&log).unwrap().unwrap();
    assert_eq!(log.lines().filter(|line| *line == "discover").count(), 1);
    assert_eq!(
        log.lines()
            .filter(|line| *line == "no-palepale-key")
            .count(),
        1
    );
    assert_eq!(log.lines().filter(|line| *line == "tools/list").count(), 3);
    project.app.mcp.disconnect("fixture");
}

#[tokio::test(flavor = "multi_thread")]
async fn matching_progress_extends_the_idle_timeout_without_replaying_a_call() {
    let project = Project::new();
    let log = project.app.paths.project.join("progress.log");
    let config = config(true, &log);
    let connection = Arc::new(
        Connection::open(
            &config,
            &project.app.paths.project,
            project.app.paths.home.join("auth"),
        )
        .await
        .unwrap(),
    );
    let result = connection
        .clone()
        .call(
            "progress".into(),
            json!({}),
            Duration::from_millis(250),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result["structuredContent"]["answer"], 42);
    assert_eq!(
        io::fs::read_string(&log)
            .unwrap()
            .unwrap()
            .lines()
            .filter(|line| *line == "echo")
            .count(),
        1
    );
    connection.close();
}

#[tokio::test(flavor = "multi_thread")]
async fn native_catalog_tools_paginate_read_resources_and_get_prompts_with_generation_checks() {
    let project = Project::new();
    let log = project.app.paths.project.join("catalog-rpc.log");
    let mut config = config(true, &log);
    config.args.push("--catalogs".into());
    config
        .environment
        .insert("MCP_FIXTURE_VALUE".into(), "configured".into());
    config.cwd = Some("subfolder".into());
    std::fs::create_dir_all(project.app.paths.project.join("subfolder")).unwrap();
    crate::mcp::save(&project.app, "fixture_server", &config, Scope::User).unwrap();
    project
        .app
        .mcp
        .connect(&project.app, "fixture_server", false)
        .await
        .unwrap();
    let registry = Registry::builtin(&project.app).snapshot();
    let cx = project.cx();
    let commands = crate::commands::discovered::list(&project.app).await;
    let slash = commands
        .iter()
        .find(|d| d.spec.name == "fixture_server:review")
        .unwrap();
    assert!(slash.run(&[]).await.is_err());
    let outcome = slash.run(&["native".into()]).await.unwrap();
    assert!(
        matches!(&outcome.effects[0],crate::commands::Effect::McpPrompt(request) if request.contains("native"))
    );
    let resource = registry.get("mcp_resources").unwrap();
    let resources = registry.get("list_mcp_resources").unwrap();
    resources.check(&json!({}), &cx).unwrap();
    assert!(
        resources
            .run(json!({}), &cx)
            .await
            .content
            .contains("memory://fixture/notes")
    );
    let templates = registry.get("list_mcp_resource_templates").unwrap();
    assert!(
        templates
            .run(json!({}), &cx)
            .await
            .content
            .contains("fixture-template")
    );
    let read = registry.get("read_mcp_resource").unwrap();
    assert!(
        read.check(&json!({"uri":"memory://fixture/notes"}), &cx)
            .is_err()
    );
    assert!(
        read.run(
            json!({"server":"fixture_server","uri":"memory://fixture/notes"}),
            &cx
        )
        .await
        .content
        .contains("Native resource body")
    );
    for (args, expected) in [
        (
            json!({"server":"fixture_server","action":"list"}),
            "memory://fixture/other",
        ),
        (
            json!({"server":"fixture_server","action":"templates"}),
            "fixture-template",
        ),
        (
            json!({"server":"fixture_server","action":"read","uri":"memory://fixture/notes"}),
            "Native resource body",
        ),
    ] {
        resource.check(&args, &cx).unwrap();
        let result = resource.run(args, &cx).await;
        assert!(
            !result.is_error && result.content.contains(expected),
            "{}",
            result.content
        );
    }
    let prompt = registry.get("mcp_prompts").unwrap();
    let result = prompt.run(json!({"server":"fixture_server","action":"get","name":"review","arguments":{"topic":"native"}}), &cx).await;
    assert!(
        !result.is_error
            && result
                .content
                .contains("Review native fixture instructions"),
        "{}",
        result.content
    );
    let log = io::fs::read_string(&log).unwrap().unwrap();
    assert!(log.contains("fixture-env") && log.contains("subfolder"));
    project.app.mcp.disconnect("fixture_server");
    assert!(
        resource
            .check(&json!({"server":"fixture_server","action":"list"}), &cx)
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn native_stdio_negotiates_both_lifecycles_and_reaches_the_normal_permission_gate() {
    for modern in [true, false] {
        let project = Project::new();
        let log = project.app.paths.project.join("rpc.log");
        let config = config(modern, &log);
        crate::mcp::save(&project.app, "fixture", &config, Scope::User).unwrap();
        project
            .app
            .mcp
            .connect(&project.app, "fixture", true)
            .await
            .unwrap();
        let view = project.app.mcp.list().remove(0);
        assert_eq!(
            view.tools.len(),
            4,
            "invalid schema should exclude only its tool"
        );
        assert!(view.message.unwrap().contains("invalid"));
        let registry = Registry::builtin(&project.app).snapshot();
        let wire = registry.wire();
        assert!(wire.iter().any(|s| {
            s["function"]["description"]
                .as_str()
                .is_some_and(|s| s.contains("{shell_note}"))
        }));
        let echo = registry.get("mcp__fixture__echo").unwrap();
        assert!(echo.check(&json!({"x":"wrong"}), &project.cx()).is_err());
        let check = echo.check(&json!({"x":1}), &project.cx()).unwrap();
        let rules = vec![Rule {
            permission: "mcp".into(),
            pattern: "fixture.*".into(),
            action: Action::Allow,
        }];
        let plan = Policy {
            rules: &rules,
            mode: crate::mode::get("plan").unwrap(),
            plan_file: "plan.md",
        };
        assert_eq!(plan.decide(&check.asks), Decision::Allow);
        let danger = registry
            .get("mcp__fixture__danger")
            .unwrap()
            .check(&json!({}), &project.cx())
            .unwrap();
        assert!(
            matches!(plan.decide(&danger.asks), Decision::Deny(_)),
            "server readOnlyHint must not bypass PLAN"
        );
        let result = echo.run(json!({"x":1}), &project.cx()).await;
        assert!(!result.is_error, "{}", result.content);
        assert!(result.content.contains("native fixture") && result.content.contains("42"));
        project.app.mcp.disconnect("fixture");
        assert!(echo.check(&json!({"x":1}), &project.cx()).is_err());
        let log = std::fs::read_to_string(log).unwrap();
        assert!(log.contains("discover"));
        assert_eq!(log.contains("initialize"), !modern);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_and_interrupt_cancel_without_replaying_the_remote_write() {
    for interrupted in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let log = directory.path().join("calls.log");
        let connection = Arc::new(
            Connection::open(
                &config(true, &log),
                directory.path(),
                directory.path().join("auth.json"),
            )
            .await
            .unwrap(),
        );
        let cancel = CancellationToken::new();
        let waiting = connection.clone().call(
            "slow_write".into(),
            json!({}),
            Duration::from_millis(350),
            cancel.clone(),
        );
        let interrupt = async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if interrupted {
                cancel.cancel();
            }
        };
        let (result, ()) = tokio::join!(waiting, interrupt);
        assert!(result.is_err());
        tokio::time::sleep(Duration::from_millis(100)).await;
        let log = std::fs::read_to_string(&log).unwrap();
        assert_eq!(log.lines().filter(|s| *s == "write").count(), 1);
        assert!(log.lines().any(|s| s == "cancel"), "{log}");
        connection.close();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn project_definitions_need_trust_and_review_and_replace_user_entries_whole() {
    let directory = tempfile::tempdir().unwrap();
    let app = App::for_tests(directory.path());
    let log = directory.path().join("calls.log");
    crate::mcp::save(&app, "fixture", &config(true, &log), Scope::User).unwrap();
    let project = Config {
        url: Some("http://127.0.0.1:9/mcp".into()),
        ..Config::default()
    };
    crate::mcp::save(&app, "fixture", &project, Scope::Project).unwrap();
    let view = app.mcp.list().remove(0);
    assert_eq!(view.state, crate::mcp::State::Untrusted);
    app.mcp.disconnect("fixture");
    assert!(app.mcp.connect(&app, "fixture", true).await.is_err());
    assert!(!log.exists());
    crate::trust::trust(&app.paths).unwrap();
    app.mcp.reload(&app).unwrap();
    let view = app.mcp.list().remove(0);
    assert_eq!(view.state, crate::mcp::State::Review);
    assert!(view.config.command.is_none());
    assert!(app.mcp.connect(&app, "fixture", false).await.is_err());
    assert!(!log.exists());
}

#[derive(Clone, Default)]
struct HttpState {
    url: String,
    modern: bool,
    auth: bool,
    requests: Arc<std::sync::Mutex<Vec<Value>>>,
    challenge: Arc<std::sync::Mutex<String>>,
    tokens: Arc<std::sync::atomic::AtomicUsize>,
    require_token: Arc<std::sync::atomic::AtomicUsize>,
}

struct HttpFixture {
    state: HttpState,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl HttpFixture {
    async fn start(modern: bool, auth: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = HttpState {
            url: format!("http://{}", listener.local_addr().unwrap()),
            modern,
            auth,
            ..HttpState::default()
        };
        let router = axum::Router::new()
            .fallback(http_handler)
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { state, task }
    }

    fn config(&self) -> Config {
        Config {
            url: Some(format!("{}/mcp", self.state.url)),
            scopes: vec!["read".into()],
            ..Config::default()
        }
    }
}

async fn http_handler(
    axum::extract::State(state): axum::extract::State<HttpState>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let path = request.uri().path().to_owned();
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let bytes = axum::body::to_bytes(request.into_body(), 131072)
        .await
        .unwrap();
    let base = &state.url;
    if path.contains("oauth-protected-resource") {
        return axum::Json(json!({"resource":format!("{base}/mcp"),"authorization_servers":[base],"scopes_supported":["read"]})).into_response();
    }
    if path.contains("oauth-authorization-server") || path.contains("openid-configuration") {
        return axum::Json(json!({"issuer":base,"authorization_endpoint":format!("{base}/authorize"),"token_endpoint":format!("{base}/token"),"registration_endpoint":format!("{base}/register"),"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"authorization_response_iss_parameter_supported":true,"scopes_supported":["read"]})).into_response();
    }
    if path == "/register" {
        let input: Value = serde_json::from_slice(&bytes).unwrap();
        return axum::Json(json!({"client_id":"fixture-client","redirect_uris":input["redirect_uris"],"token_endpoint_auth_method":"none"})).into_response();
    }
    if path == "/token" {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let form: std::collections::HashMap<_, _> =
            url::form_urlencoded::parse(&bytes).into_owned().collect();
        if form.get("grant_type").map(String::as_str) == Some("authorization_code") {
            let verifier = form.get("code_verifier").unwrap();
            let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(Sha256::digest(verifier.as_bytes()));
            assert_eq!(challenge, *state.challenge.lock().unwrap());
            assert_eq!(form.get("resource").unwrap(), &format!("{base}/mcp"));
        }
        let generation = state.tokens.fetch_add(1, Ordering::SeqCst) + 1;
        return axum::Json(json!({"access_token":format!("fixture-access-{generation}"),"refresh_token":format!("fixture-refresh-{generation}"),"token_type":"Bearer","expires_in":3600,"scope":"read"})).into_response();
    }
    if path != "/mcp" {
        return StatusCode::NOT_FOUND.into_response();
    }
    if state.auth {
        let required = state.require_token.load(Ordering::SeqCst).max(1);
        if authorization != format!("Bearer fixture-access-{required}") {
            return (StatusCode::UNAUTHORIZED, [("www-authenticate", format!("Bearer error=\"invalid_token\", resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\""))]).into_response();
        }
    }
    if bytes.is_empty() {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let request: Value = serde_json::from_slice(&bytes).unwrap();
    state.requests.lock().unwrap().push(request.clone());
    let method = request["method"].as_str().unwrap_or_default();
    if request.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let result = match method {
        "server/discover" if state.modern => {
            serde_json::to_value(rmcp::model::DiscoverResult::new(
                vec![ProtocolVersion::V_2026_07_28],
                serde_json::from_value(json!({"tools":{}})).unwrap(),
            ))
            .unwrap()
        }
        "server/discover" => return axum::Json(
            json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32601,"message":"Legacy"}}),
        )
        .into_response(),
        "initialize" => {
            json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{}},"serverInfo":{"name":"http-fixture","version":"1"}})
        }
        "tools/list" => {
            json!({"tools":[{"name":"echo","description":"HTTP echo","inputSchema":{"type":"object","properties":{}}},{"name":"sse_echo","description":"SSE echo","inputSchema":{"type":"object","properties":{}}}]})
        }
        "tools/call" => {
            json!({"content":[{"type":"text","text":"HTTP fixture"}],"structuredContent":[1,2,3],"isError":false})
        }
        _ => json!({}),
    };
    let response = json!({"jsonrpc":"2.0","id":request["id"],"result":result});
    if request.pointer("/params/name").and_then(Value::as_str) == Some("sse_echo") {
        return (
            [("content-type", "text/event-stream")],
            format!("event: message\ndata: {response}\n\n"),
        )
            .into_response();
    }
    axum::Json(response).into_response()
}

#[tokio::test(flavor = "multi_thread")]
async fn streamable_http_supports_json_sse_and_both_protocol_lifecycles() {
    for modern in [true, false] {
        let fixture = HttpFixture::start(modern, false).await;
        let directory = tempfile::tempdir().unwrap();
        let connection = Arc::new(
            Connection::open(
                &fixture.config(),
                directory.path(),
                directory.path().join("auth.json"),
            )
            .await
            .unwrap(),
        );
        assert_eq!(connection.tools().await.unwrap().len(), 2);
        for name in ["echo", "sse_echo"] {
            let result = connection
                .clone()
                .call(
                    name.into(),
                    json!({}),
                    Duration::from_secs(3),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(result["content"][0]["text"], "HTTP fixture");
            assert_eq!(result["structuredContent"], json!([1, 2, 3]));
        }
        let requests = fixture.state.requests.lock().unwrap();
        assert_eq!(
            requests.iter().any(|r| r["method"] == "initialize"),
            !modern
        );
        if modern {
            assert!(
                requests
                    .iter()
                    .filter(|r| r["method"] == "tools/call")
                    .all(|r| r.to_string().contains("2026-07-28"))
            );
        }
        connection.close();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn native_oauth_checks_state_pkce_and_resource_then_refreshes_and_logs_out() {
    use crate::io::mcp_auth::{Login, Store};
    let fixture = HttpFixture::start(true, true).await;
    let directory = tempfile::tempdir().unwrap();
    let credentials = directory.path().join("auth.json");
    let config = fixture.config();
    let unauthorized = Connection::open(&config, directory.path(), credentials.clone())
        .await
        .err()
        .unwrap();
    assert!(unauthorized.authorization);
    let login = Login::begin(
        &config,
        credentials.clone(),
        unauthorized.challenge.as_deref(),
    )
    .await
    .unwrap();
    let url = url::Url::parse(login.url()).unwrap();
    let params: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(params["code_challenge_method"], "S256");
    *fixture.state.challenge.lock().unwrap() = params["code_challenge"].clone();
    let mut callback = url::Url::parse(&params["redirect_uri"]).unwrap();
    callback
        .query_pairs_mut()
        .append_pair("code", "fixture-code")
        .append_pair("state", "wrong-state")
        .append_pair("iss", &fixture.state.url);
    let cancel = CancellationToken::new();
    let wait_cancel = cancel.clone();
    let waiting = tokio::spawn(async move { login.finish(&wait_cancel).await });
    let response = reqwest::get(callback.as_str()).await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(fixture.state.tokens.load(Ordering::SeqCst), 0);
    callback
        .query_pairs_mut()
        .clear()
        .append_pair("code", "fixture-code")
        .append_pair("state", &params["state"])
        .append_pair("iss", &fixture.state.url);
    assert!(
        reqwest::get(callback.as_str())
            .await
            .unwrap()
            .status()
            .is_success()
    );
    waiting.await.unwrap().unwrap();
    let connection = Arc::new(
        Connection::open(&config, directory.path(), credentials.clone())
            .await
            .unwrap(),
    );
    let result = connection
        .clone()
        .call(
            "echo".into(),
            json!({}),
            Duration::from_secs(3),
            cancel.clone(),
        )
        .await
        .unwrap();
    assert_eq!(result["content"][0]["text"], "HTTP fixture");
    fixture.state.require_token.store(2, Ordering::SeqCst);
    connection
        .clone()
        .call("echo".into(), json!({}), Duration::from_secs(3), cancel)
        .await
        .unwrap();
    assert_eq!(fixture.state.tokens.load(Ordering::SeqCst), 2);
    let saved = std::fs::read_to_string(&credentials).unwrap();
    assert!(!saved.contains("fixture-access") && !saved.contains("fixture-refresh"));
    connection.close();
    Store::new(credentials.clone()).remove().unwrap();
    assert!(!credentials.exists());
}

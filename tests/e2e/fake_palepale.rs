use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use futures_util::StreamExt;
use serde_json::{Value, json};

const MODELS: &str = include_str!("../fixtures/api/models.json");

/// Keys the fake accepts, and what each stands for.
pub const ENV_KEY: &str = "test-key";
pub const DEVICE_KEY: &str = "device-key-9999";
pub const ACCOUNT_NUMBER: &str = "1234567890123456";
pub const OWNER_KEY: &str = "po_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
pub const CLIENT_KEY: &str = "pk_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
/// The one password the fake forum's Tapatalk plugin accepts, and the session it hands back.
pub const FORUM_PASSWORD: &str = "hunter2";
pub const FORUM_COOKIE: &str = "bbsessionhash=fake; path=/; HttpOnly";
/// Where the fake forum lives on the fake server.
pub const FORUM_PATH: &str = "/forum";

/// One scripted reply to a chat request.
pub enum Reply {
    /// An SSE fixture from tests/fixtures/sse/.
    Sse(String),
    /// A JSON error envelope with this status and `error.type`.
    Error {
        status: u16,
        kind: &'static str,
        retry_after: Option<u64>,
    },
    /// An error that sends its headers (retry at once) and the start of its body, then nothing.
    Stall { status: u16 },
    /// Headers arrive, then the HTTP body breaks after a keepalive.
    BrokenBody,
}

#[derive(Default)]
struct Inner {
    /// The fake's own address, filled into scripted replies as `{base}` so a scripted model
    /// can point webfetch at this server.
    base: String,
    /// The sandbox's palepale home, filled in as `{home}`, with `/` separators.
    home: String,
    paths: Vec<String>,
    profile_queries: Vec<String>,
    chat_bodies: Vec<Value>,
    chat_operations: Vec<String>,
    model_lists: VecDeque<String>,
    agent_profile_lists: VecDeque<String>,
    model_delays: VecDeque<std::time::Duration>,
    overview_errors: VecDeque<u16>,
    overview_delays: VecDeque<std::time::Duration>,
    script: VecDeque<Reply>,
    /// Answers to pairing polls before the approval, in order.
    pair_polls: VecDeque<PairPoll>,
    pair_collected: bool,
    me_errors: VecDeque<u16>,
    /// Key issuance failures before accepting a request.
    key_failures: usize,
    trial_redemption_polls: usize,
    trial_states: VecDeque<String>,
    trial_available: Option<bool>,
    handoff_failures: usize,
}

type Shared = Arc<Mutex<Inner>>;

/// One scripted answer to a pairing poll.
pub enum PairPoll {
    Pending,
    Error { status: u16, kind: &'static str },
}

/// A local stand-in for palepale: serves fixtures, follows a script, records what it was asked.
#[derive(Clone)]
pub struct Fake {
    pub url: String,
    inner: Shared,
}

impl Reply {
    pub fn sse(name: &str) -> Self {
        Self::Sse(name.to_owned())
    }
}

impl Fake {
    pub fn trial_states(&self, states: impl IntoIterator<Item = &'static str>) {
        self.inner
            .lock()
            .unwrap()
            .trial_states
            .extend(states.into_iter().map(str::to_owned));
    }
    pub fn trial_available(&self, available: bool) {
        self.inner.lock().unwrap().trial_available = Some(available);
    }
    pub fn fail_handoff(&self) {
        self.inner.lock().unwrap().handoff_failures += 1;
    }
    pub fn profile_queries(&self) -> Vec<String> {
        self.inner.lock().unwrap().profile_queries.clone()
    }
    pub fn chat_operations(&self) -> Vec<String> {
        self.inner.lock().unwrap().chat_operations.clone()
    }
    pub fn me_errors(&self, errors: impl IntoIterator<Item = u16>) {
        self.inner.lock().unwrap().me_errors.extend(errors);
    }
    pub async fn start() -> Self {
        let inner: Shared = Arc::default();
        let app = Router::new()
            .route("/v1/models", get(models))
            .route("/v1/agent-profiles", get(agent_profiles))
            .route("/v1/chat/completions", post(chat))
            .route("/v1/account/summary", get(me))
            .route("/v1/account/trial", get(trial_status))
            .route("/v1/trial/flows", post(trial_flow))
            .route("/v1/trial/redeem", post(trial_redeem))
            .route("/v1/account/browser-handoffs", post(browser_handoff))
            .route("/v1/account/keys", get(keys).post(create_key))
            .route("/v1/account/keys/{reference}", delete(revoke_key))
            .route("/v1/account/usage/summary", get(overview))
            .route("/v1/pair/start", post(pair_start))
            .route("/v1/pair/poll", post(pair_poll))
            .route("/v1/accounts", post(signup))
            .route("/web/ok", get(web_ok))
            .route("/skills/index.json", get(skill_index))
            .route("/skills/native-check/SKILL.md", get(skill_body))
            .route(
                "/skills/native-check/references/guide.md",
                get(|| async { "Remote supporting reference" }),
            )
            .route("/web/blocked", get(web_blocked))
            .route("/forum/mobiquo/mobiquo.php", post(mobiquo))
            .layer(middleware::from_fn_with_state(inner.clone(), record))
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        inner.lock().unwrap().base = url.clone();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, inner }
    }

    /// Where `{home}` in a scripted reply points.
    pub fn set_home(&self, home: &std::path::Path) {
        self.inner.lock().unwrap().home = home.to_string_lossy().replace('\\', "/");
    }

    pub fn script(&self, replies: impl IntoIterator<Item = Reply>) {
        self.inner.lock().unwrap().script.extend(replies);
    }

    pub fn model_lists(&self, lists: impl IntoIterator<Item = Value>) {
        self.inner
            .lock()
            .unwrap()
            .model_lists
            .extend(lists.into_iter().map(|list| list.to_string()));
    }

    pub fn agent_profile_lists(&self, lists: impl IntoIterator<Item = Value>) {
        self.inner
            .lock()
            .unwrap()
            .agent_profile_lists
            .extend(lists.into_iter().map(|list| list.to_string()));
    }

    pub fn model_delays(&self, delays: impl IntoIterator<Item = std::time::Duration>) {
        self.inner.lock().unwrap().model_delays.extend(delays);
    }

    pub fn overview_errors(&self, errors: impl IntoIterator<Item = u16>) {
        self.inner.lock().unwrap().overview_errors.extend(errors);
    }

    pub fn overview_delays(&self, delays: impl IntoIterator<Item = std::time::Duration>) {
        self.inner.lock().unwrap().overview_delays.extend(delays);
    }

    /// How many polls answer "pending" before the pairing is approved.
    pub fn pair_after(&self, pending: usize) {
        self.pair_polls((0..pending).map(|_| PairPoll::Pending));
    }

    /// Polls answer these, in order, before the pairing is approved.
    pub fn pair_polls(&self, polls: impl IntoIterator<Item = PairPoll>) {
        self.inner.lock().unwrap().pair_polls.extend(polls);
    }

    pub fn key_failures(&self, count: usize) {
        self.inner.lock().unwrap().key_failures = count;
    }

    pub fn hits(&self, path: &str) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.paths.iter().filter(|p| *p == path).count()
    }

    pub fn chat_bodies(&self) -> Vec<Value> {
        self.inner.lock().unwrap().chat_bodies.clone()
    }
}

async fn record(State(inner): State<Shared>, req: Request, next: Next) -> Response {
    if req.uri().path() == "/v1/agent-profiles" {
        inner
            .lock()
            .unwrap()
            .profile_queries
            .push(req.uri().query().unwrap_or_default().to_owned());
    }
    inner
        .lock()
        .unwrap()
        .paths
        .push(req.uri().path().to_owned());
    next.run(req).await
}

fn authorized(headers: &HeaderMap) -> bool {
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    matches!(
        bearer,
        Some(ENV_KEY | DEVICE_KEY | ACCOUNT_NUMBER | CLIENT_KEY)
    )
}

fn error(status: u16, kind: &str, retry_after: Option<u64>) -> Response {
    let mut resp = (
        StatusCode::from_u16(status).unwrap(),
        [("x-request-id", "req_fake")],
        axum::Json(json!({"error": {"message": format!("fake {kind}"), "type": kind}})),
    )
        .into_response();
    if let Some(secs) = retry_after {
        resp.headers_mut()
            .insert(header::RETRY_AFTER, secs.to_string().parse().unwrap());
    }
    resp
}

/// A plain page, as a documentation site serves it.
async fn web_ok() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<html><head><title>Widget docs</title></head><body>\
         <h1>Widget API</h1><p>The widget spins at 3 rpm.</p></body></html>",
    )
}

async fn skill_index() -> impl IntoResponse {
    axum::Json(
        json!({"skills":[{"name":"native-check","version":"1","files":["SKILL.md","references/guide.md"]}]}),
    )
}

async fn agent_profiles(State(state): State<Shared>, headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Some(catalog) = state.lock().unwrap().agent_profile_lists.pop_front() {
        return axum::Json(serde_json::from_str::<Value>(&catalog).unwrap()).into_response();
    }
    axum::Json(json!({"object":"list","data":[
        {"id":"jb-1","name":"jb-1","description":"GLM 5.3"},
        {"id":"jb-2","name":"CINDER","description":"CINDER"},
        {"id":"jb-3","name":"XYRO","description":"XYRO"}
    ]}))
    .into_response()
}

async fn skill_body() -> &'static str {
    "---\nname: native-check\ndescription: Review using a remote skill source\n---\nRemote skill installed during this conversation. Read references/guide.md when needed."
}

/// A forum's Tapatalk plugin, as far as logging in: `FORUM_PASSWORD` gets a session cookie, any
/// other password the refusal a real plugin sends.
async fn mobiquo(body: String) -> Response {
    use base64::Engine;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let member = |name: &str, value: &str| {
        format!("<member><name>{name}</name><value>{value}</value></member>")
    };
    let ok = body.contains(&b64(FORUM_PASSWORD));
    let members = if ok {
        member("result", "<boolean>1</boolean>")
            + &member("username", &format!("<base64>{}</base64>", b64("demo")))
    } else {
        member("result", "<boolean>0</boolean>")
            + &member(
                "result_text",
                &format!("<base64>{}</base64>", b64("Wrong password")),
            )
    };
    let xml = format!(
        "<?xml version=\"1.0\"?><methodResponse><params><param><value><struct>{members}\
         </struct></value></param></params></methodResponse>"
    );
    let mut resp = ([(header::CONTENT_TYPE, "text/xml")], xml).into_response();
    if ok {
        resp.headers_mut()
            .insert(header::SET_COOKIE, FORUM_COOKIE.parse().unwrap());
    }
    resp
}

/// What Cloudflare serves while it challenges a client: 403, its mitigation header, and an
/// interstitial that must never reach the model as if it were the page.
async fn web_blocked() -> Response {
    (
        StatusCode::FORBIDDEN,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::HeaderName::from_static("cf-mitigated"), "challenge"),
        ],
        "<html><head><title>Just a moment...</title></head><body>\
         <script>window._cf_chl_opt={cType:'managed'};</script>\
         <noscript>Enable JavaScript and cookies to continue</noscript></body></html>",
    )
        .into_response()
}

async fn models(State(inner): State<Shared>) -> impl IntoResponse {
    let (body, delay) = {
        let mut inner = inner.lock().unwrap();
        (
            inner
                .model_lists
                .pop_front()
                .unwrap_or_else(|| MODELS.to_owned()),
            inner.model_delays.pop_front().unwrap_or_default(),
        )
    };
    tokio::time::sleep(delay).await;
    ([(header::CONTENT_TYPE, "application/json")], body)
}

async fn trial_status(State(inner): State<Shared>, headers: HeaderMap) -> Response {
    if !authorized(&headers) && !owner_authorized(&headers) {
        return error(401, "unauthorized", None);
    }
    let mut inner = inner.lock().unwrap();
    let state = inner
        .trial_states
        .pop_front()
        .unwrap_or_else(|| "redeemed".to_owned());
    let application = if state == "none" {
        Value::Null
    } else {
        json!({"id":"11111111-1111-4111-8111-111111111111","state":state})
    };
    axum::Json(json!({"model":"deepseek-v4.1-flash-uncensored-zero-refusal","available":inner.trial_available.unwrap_or(true),"application":application,"wallet":{"balance":"2000000","available":"1999000","held":"1000"}})).into_response()
}
async fn trial_flow(headers: HeaderMap, body: String) -> Response {
    if !trial_signature(&headers, &body, "/v1/trial/flows") {
        return error(403, "trial_device_required", None);
    }
    if !authorized(&headers) {
        return error(401, "unauthorized", None);
    }
    let value: Value = serde_json::from_str(&body).unwrap();
    assert!(matches!(
        value["action"].as_str(),
        Some("trial_request" | "trial_redeem")
    ));
    axum::Json(json!({"flow":"n".repeat(43),"expires_in":600})).into_response()
}
async fn browser_handoff(State(inner): State<Shared>, headers: HeaderMap) -> Response {
    let mut state = inner.lock().unwrap();
    if state.handoff_failures > 0 {
        state.handoff_failures -= 1;
        return error(401, "unauthorized", None);
    }
    if !owner_authorized(&headers) {
        return error(401, "unauthorized", None);
    }
    axum::Json(json!({"code":format!("bh_{}","h".repeat(43))})).into_response()
}
async fn chat(State(inner): State<Shared>, headers: HeaderMap, body: String) -> Response {
    if serde_json::from_str::<Value>(&body).unwrap_or(Value::Null)["funding"] == "trial"
        && !trial_signature(&headers, &body, "/v1/chat/completions")
    {
        return error(403, "trial_device_required", None);
    }
    if !authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    let (reply, base, home) = {
        let mut inner = inner.lock().unwrap();
        inner.chat_operations.push(
            headers
                .get("idempotency-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        inner
            .chat_bodies
            .push(serde_json::from_str(&body).unwrap_or(Value::Null));
        (
            inner.script.pop_front(),
            inner.base.clone(),
            inner.home.clone(),
        )
    };
    match reply {
        Some(Reply::Sse(name)) => {
            let path = format!("{}/tests/fixtures/sse/{name}", env!("CARGO_MANIFEST_DIR"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("{path}"))
                .replace("{base}", &base)
                .replace("{home}", &home);
            (
                [
                    (header::CONTENT_TYPE, "text/event-stream"),
                    (header::HeaderName::from_static("x-request-id"), "req_fake"),
                ],
                text,
            )
                .into_response()
        }
        Some(Reply::Error {
            status,
            kind,
            retry_after,
        }) => error(status, kind, retry_after),
        Some(Reply::Stall { status }) => {
            let start =
                futures_util::stream::once(async { Ok::<_, std::io::Error>(r#"{"error":"#) });
            let body = axum::body::Body::from_stream(start.chain(futures_util::stream::pending()));
            (
                StatusCode::from_u16(status).unwrap(),
                [("x-request-id", "req_fake"), ("retry-after", "0")],
                body,
            )
                .into_response()
        }
        Some(Reply::BrokenBody) => {
            let start =
                futures_util::stream::once(async { Ok::<_, std::io::Error>(": keepalive\n\n") });
            let broken = futures_util::stream::once(async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                Err::<&str, _>(std::io::Error::other("fixture body interruption"))
            });
            (
                [
                    ("content-type", "text/event-stream"),
                    ("x-request-id", "req_body"),
                ],
                axum::body::Body::from_stream(start.chain(broken)),
            )
                .into_response()
        }
        None => error(500, "unscripted", None),
    }
}

async fn me(State(inner): State<Shared>, headers: HeaderMap) -> Response {
    if let Some(status) = inner.lock().unwrap().me_errors.pop_front() {
        return error(
            status,
            if status == 401 {
                "invalid_api_key"
            } else {
                "storage_unavailable"
            },
            None,
        );
    }
    if !authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    axum::Json(json!({"account_hint": "3456", "balance_microdollars": 12_300_000, "available_microdollars": 12_300_000, "reserved_microdollars":0,"token_budget":100000,"used_tokens":25000,"reserved_tokens":0,"can_spend": true})).into_response()
}

fn owner_authorized(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(format!("Bearer {OWNER_KEY}").as_str())
}
async fn keys(headers: HeaderMap) -> Response {
    if !owner_authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    axum::Json(json!({"data":[{"id":"00000000-0000-0000-0000-000000000001", "label":"This terminal", "createdAt":"2026-01-01T00:00:00Z", "revoked":false}]})).into_response()
}
async fn create_key(
    State(inner): State<Shared>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    if !owner_authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    assert!(body["label"].is_string());
    let mut inner = inner.lock().unwrap();
    if inner.key_failures > 0 {
        inner.key_failures -= 1;
        return error(503, "unavailable", None);
    }
    (StatusCode::CREATED, axum::Json(json!({"key":CLIENT_KEY}))).into_response()
}
async fn revoke_key(headers: HeaderMap) -> Response {
    if !owner_authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    StatusCode::NO_CONTENT.into_response()
}
async fn overview(State(inner): State<Shared>, headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    let (delay, failure) = {
        let mut inner = inner.lock().unwrap();
        (
            inner.overview_delays.pop_front().unwrap_or_default(),
            inner.overview_errors.pop_front(),
        )
    };
    tokio::time::sleep(delay).await;
    if let Some(status) = failure {
        return error(status, "usage_unavailable", None);
    }
    axum::Json(json!({"confirmed_requests":42,"unknown_requests":2,"spent_microdollars":3_000_000,"prompt_tokens":12000,"completion_tokens":3000,"cached_tokens":6000,"reasoning_tokens":1000})).into_response()
}

async fn pair_start() -> impl IntoResponse {
    axum::Json(
        json!({"device_code": "synthetic-device-code-00000000000000000001", "user_code": "012345", "ttl": 60, "interval": 1}),
    )
}

async fn pair_poll(State(inner): State<Shared>, axum::Json(body): axum::Json<Value>) -> Response {
    let code = body["device_code"].as_str().unwrap_or_default();
    let mut inner = inner.lock().unwrap();
    if code != "synthetic-device-code-00000000000000000001" || inner.pair_collected {
        return error(410, "pair_expired", None);
    }
    match inner.pair_polls.pop_front() {
        Some(PairPoll::Pending) => {
            return (
                StatusCode::ACCEPTED,
                axum::Json(json!({"status": "pending"})),
            )
                .into_response();
        }
        Some(PairPoll::Error { status, kind }) => return error(status, kind, None),
        None => {}
    }
    inner.pair_collected = true;
    axum::Json(json!({"status": "ready", "api_key": DEVICE_KEY})).into_response()
}

async fn signup() -> Response {
    (
        StatusCode::CREATED,
        axum::Json(
            json!({"ownerCredential":OWNER_KEY,"accountId":"00000000-0000-0000-0000-000000000001"}),
        ),
    )
        .into_response()
}

fn trial_signature(headers: &HeaderMap, body: &str, path: &str) -> bool {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as BASE64};
    use sha2::{Digest, Sha256};
    let Some(proof) = headers
        .get("x-palepale-trial-proof")
        .and_then(|h| h.to_str().ok())
    else {
        return false;
    };
    let parts: Vec<_> = proof.split('.').collect();
    if parts.len() != 5 || parts[0] != "v1" {
        return false;
    }
    let public = BASE64
        .decode(parts[3])
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok());
    let signature = BASE64
        .decode(parts[4])
        .ok()
        .and_then(|b| ed25519_dalek::Signature::from_slice(&b).ok());
    let (Some(public), Some(signature)) = (public, signature) else {
        return false;
    };
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&public) else {
        return false;
    };
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    let digest = |bytes: &[u8]| {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let idempotency = headers
        .get("idempotency-key")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let message = [
        "palepale-trial-v1",
        "POST",
        path,
        parts[1],
        parts[2],
        parts[3],
        &digest(token.as_bytes()),
        idempotency,
        &digest(body.as_bytes()),
    ]
    .join("\n");
    key.verify_strict(message.as_bytes(), &signature).is_ok()
}
async fn trial_redeem(State(inner): State<Shared>, headers: HeaderMap, body: String) -> Response {
    if !authorized(&headers) || !trial_signature(&headers, &body, "/v1/trial/redeem") {
        return error(403, "trial_device_required", None);
    }
    let value: Value = serde_json::from_str(&body).unwrap();
    if value["code"] != format!("pt_{}", "x".repeat(43)) {
        return error(400, "trial_invalid", None);
    }
    let mut inner = inner.lock().unwrap();
    inner.trial_redemption_polls += 1;
    axum::Json(if inner.trial_redemption_polls == 1 {
        json!({"pending":true})
    } else {
        json!({"ok":true,"amount":"2000000"})
    })
    .into_response()
}

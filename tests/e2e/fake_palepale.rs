use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt;
use serde_json::{Value, json};

const MODELS: &str = include_str!("../fixtures/api/models.json");

/// Keys the fake accepts, and what each stands for.
pub const ENV_KEY: &str = "test-key";
pub const DEVICE_KEY: &str = "device-key-9999";
pub const ACCOUNT_NUMBER: &str = "1234567890123456";
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
}

#[derive(Default)]
struct Inner {
    /// The fake's own address, filled into scripted replies as `{base}` so a scripted model
    /// can point webfetch at this server.
    base: String,
    /// The sandbox's palepale home, filled in as `{home}`, with `/` separators.
    home: String,
    paths: Vec<String>,
    chat_bodies: Vec<Value>,
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
    /// Signups to refuse with 428 before accepting one.
    stale_proofs: usize,
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
    pub fn me_errors(&self, errors: impl IntoIterator<Item = u16>) {
        self.inner.lock().unwrap().me_errors.extend(errors);
    }
    pub async fn start() -> Self {
        let inner: Shared = Arc::default();
        let app = Router::new()
            .route("/v1/models", get(models))
            .route("/v1/agent-profiles", get(agent_profiles))
            .route("/v1/chat/completions", post(chat))
            .route("/api/me", get(me))
            .route("/api/me/keys", get(keys))
            .route("/api/me/keys/{reference}/revoke", post(revoke_key))
            .route("/api/me/overview", get(overview))
            .route("/api/pair/start", post(pair_start))
            .route("/api/pair/{code}", get(pair_poll))
            .route("/api/pow", get(pow))
            .route("/api/signup", post(signup))
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

    pub fn stale_proofs(&self, count: usize) {
        self.inner.lock().unwrap().stale_proofs = count;
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
    matches!(bearer, Some(ENV_KEY | DEVICE_KEY | ACCOUNT_NUMBER))
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

async fn agent_profiles(State(state): State<Shared>) -> impl IntoResponse {
    if let Some(catalog) = state.lock().unwrap().agent_profile_lists.pop_front() {
        return axum::Json(serde_json::from_str::<Value>(&catalog).unwrap());
    }
    axum::Json(json!({"object":"list","data":[
        {"id":"jb-1","name":"jb-1","description":"GLM 5.3"},
        {"id":"jb-2","name":"CINDER","description":"CINDER"},
        {"id":"jb-3","name":"XYRO","description":"XYRO"}
    ]}))
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

async fn chat(State(inner): State<Shared>, headers: HeaderMap, body: String) -> Response {
    if !authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    let (reply, base, home) = {
        let mut inner = inner.lock().unwrap();
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

async fn keys(headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    axum::Json(json!({"keys":[{"ref":"00000000-0000-0000-0000-000000000001", "name":"This terminal", "hint":"9999", "created_at":"2026-01-01T00:00:00Z", "expires_at":"2027-01-01T00:00:00Z", "revoked_at":null, "current":true}]})).into_response()
}

async fn revoke_key(Path(reference): Path<String>, headers: HeaderMap) -> Response {
    if !authorized(&headers) {
        return error(401, "invalid_api_key", None);
    }
    if reference.ends_with('1') {
        return error(409, "cannot_revoke_current", None);
    }
    axum::Json(json!({"ok":true})).into_response()
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
        json!({"device_code": "synthetic-device-code-00000000000000000001", "user_code": "012345", "ttl": 60}),
    )
}

async fn pair_poll(State(inner): State<Shared>, Path(code): Path<String>) -> Response {
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

const POW_BITS: u32 = 8;

async fn pow() -> impl IntoResponse {
    axum::Json(json!({"challenge": "fakechallenge", "bits": POW_BITS, "ttl": 60}))
}

async fn signup(State(inner): State<Shared>, headers: HeaderMap) -> Response {
    use sha2::{Digest, Sha256};
    let proof = headers
        .get("x-pow")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let Some((challenge, nonce)) = proof.split_once('.') else {
        return error(428, "pow_required", None);
    };
    let hash = Sha256::digest(format!("{challenge}:{nonce}"));
    let zeros = hash
        .iter()
        .scan(true, |counting, b| {
            let bits = if *counting { b.leading_zeros() } else { 0 };
            *counting &= *b == 0;
            Some(bits)
        })
        .sum::<u32>();
    let mut inner = inner.lock().unwrap();
    if challenge != "fakechallenge" || zeros < POW_BITS || inner.stale_proofs > 0 {
        inner.stale_proofs = inner.stale_proofs.saturating_sub(1);
        return error(428, "pow_required", None);
    }
    axum::Json(json!({
        "account_number": ACCOUNT_NUMBER,
        "formatted": "1234 5678 9012 3456",
        "api_base": "https://api.palepale.sh/v1"
    }))
    .into_response()
}

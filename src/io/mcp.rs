//! Official SDK transports, child processes, and cancellation.
#![allow(clippy::disallowed_methods)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, bail};
use rmcp::model::{
    ClientCapabilities, ClientConfig, ClientRequest, Implementation, ProtocolVersion,
};
use rmcp::service::{NotificationContext, PeerRequestOptions, RunningService};
use rmcp::transport::auth::AuthClient;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt, RoleClient};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::extensions::text;
use crate::mcp::Config;
use crate::{io, limits, settings};

pub struct Handler {
    changed: Arc<AtomicBool>,
}

impl ClientHandler for Handler {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("palepale", env!("CARGO_PKG_VERSION")),
        )
    }
    async fn on_tool_list_changed(&self, _: NotificationContext<RoleClient>) {
        self.changed.store(true, Ordering::Release);
    }
}

pub struct Connection {
    service: RunningService<RoleClient, Handler>,
    serial: Arc<tokio::sync::Mutex<()>>,
    changed: Arc<AtomicBool>,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ConnectError {
    pub message: String,
    pub authorization: bool,
    pub challenge: Option<String>,
}

fn initialize_error(e: rmcp::service::ClientInitializeError) -> ConnectError {
    let mut cause: &dyn std::error::Error = &e;
    while let Some(source) = cause.source() {
        cause = source;
    }
    let detail = cause.to_string();
    let message = if e.is_authorization_required() {
        text().authentication_required.clone()
    } else if detail.contains("rmcp::") {
        text().connection_failed.clone()
    } else {
        detail
    };
    ConnectError {
        authorization: e.is_authorization_required(),
        challenge: e.auth_challenge().map(str::to_owned),
        message,
    }
}

impl Connection {
    pub async fn open(
        config: &Config,
        project: &Path,
        credentials: std::path::PathBuf,
    ) -> Result<Self, ConnectError> {
        let credentials_fallback = credentials.clone();
        let operation = async {
            config.validate()?;
            let changed = Arc::new(AtomicBool::new(false));
            let handler = Handler {
                changed: changed.clone(),
            };
            let lifecycle = ClientLifecycleMode::Auto {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                legacy_version: Some(ProtocolVersion::V_2025_11_25),
            };
            let service = if let Some(command) = &config.command {
                let program =
                    io::proc::which(command).unwrap_or_else(|| std::path::PathBuf::from(command));
                let mut process = tokio::process::Command::new(&program);
                let cwd = config
                    .cwd
                    .as_ref()
                    .map(|path| {
                        if path.is_absolute() {
                            path.clone()
                        } else {
                            project.join(path)
                        }
                    })
                    .unwrap_or_else(|| project.to_path_buf());
                process
                    .args(&config.args)
                    .current_dir(cwd)
                    .env_clear()
                    .kill_on_drop(true)
                    .stderr(std::process::Stdio::null());
                for name in [
                    "PATH",
                    "HOME",
                    "USERPROFILE",
                    "HOMEDRIVE",
                    "HOMEPATH",
                    "SYSTEMROOT",
                    "WINDIR",
                    "COMSPEC",
                    "PATHEXT",
                    "APPDATA",
                    "LOCALAPPDATA",
                    "TEMP",
                    "TMP",
                    "TMPDIR",
                    "LANG",
                    "LC_ALL",
                ] {
                    if let Some(value) = settings::env(name) {
                        process.env(name, value);
                    }
                }
                for name in &config.env_vars {
                    if name.to_ascii_uppercase().starts_with("PALEPALE_") {
                        bail!("Palepale credentials cannot be inherited by MCP servers");
                    }
                    if let Some(value) = settings::env(name) {
                        process.env(name, value);
                    }
                }
                process.envs(&config.environment);
                let mut process = process_wrap::tokio::CommandWrap::from(process);
                process.wrap(process_wrap::tokio::KillOnDrop);
                #[cfg(windows)]
                {
                    let mut flags = process_wrap::tokio::CreationFlags(Default::default());
                    flags.0.0 = 0x08000000;
                    process.wrap(flags).wrap(process_wrap::tokio::JobObject);
                }
                #[cfg(unix)]
                process.wrap(process_wrap::tokio::ProcessGroup::leader());
                let (transport, _) = TokioChildProcess::builder(process)
                    .stderr(std::process::Stdio::null())
                    .spawn()?;
                handler
                    .serve_with_lifecycle(transport, lifecycle)
                    .await
                    .map_err(initialize_error)?
            } else {
                let url = config.url.as_deref().context("Missing MCP URL")?;
                let mut headers = reqwest::header::HeaderMap::new();
                for (name, value) in &config.http_headers {
                    headers.insert(
                        reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
                        reqwest::header::HeaderValue::from_str(value)?,
                    );
                }
                for (name, variable) in &config.env_http_headers {
                    let value = settings::env(variable)
                        .with_context(|| format!("Environment variable {variable} is not set"))?;
                    headers.insert(
                        reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
                        reqwest::header::HeaderValue::from_str(&value)?,
                    );
                }
                for name in [
                    "host",
                    "content-length",
                    "transfer-encoding",
                    "connection",
                    "mcp-session-id",
                    "mcp-protocol-version",
                ] {
                    if headers.contains_key(name) {
                        bail!("MCP header {name} is reserved by the transport");
                    }
                }
                let explicit_auth = headers.contains_key(reqwest::header::AUTHORIZATION);
                if let Some(variable) = &config.bearer_token_env_var {
                    let token = settings::env(variable)
                        .with_context(|| format!("Environment variable {variable} is not set"))?;
                    headers.insert(
                        reqwest::header::AUTHORIZATION,
                        reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))?,
                    );
                }
                let client = reqwest::Client::builder()
                    .default_headers(headers)
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .connect_timeout(limits::get().http.connect_timeout_ms.ms())
                    .build()?;
                // CEILING: rmcp buffers complete JSON bodies/stdio lines; wire-size limits need
                // a bounded SDK transport adapter. Schemas and displayed/model output are capped.
                let mut transport_config =
                    StreamableHttpClientTransportConfig::with_uri(url.to_owned())
                        .max_concurrent_requests(1);
                // A tools/call may mutate state; session recovery must never replay it.
                transport_config.reinit_on_expired_session = false;
                transport_config.max_sse_event_size =
                    limits::get().tools.bash_spill_max_bytes.value;
                if config.transport == crate::mcp::Transport::Sse {
                    let transport =
                        super::mcp_sse::Sse::open(client, config, credentials, explicit_auth)
                            .await?;
                    handler
                        .serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
                        .await
                        .map_err(initialize_error)?
                } else if let Some(variable) = &config.bearer_token_env_var {
                    transport_config.auth_header =
                        Some(settings::env(variable).with_context(|| {
                            format!("Environment variable {variable} is not set")
                        })?);
                    handler
                        .serve_with_lifecycle(
                            StreamableHttpClientTransport::with_client(client, transport_config),
                            lifecycle,
                        )
                        .await
                        .map_err(initialize_error)?
                } else if config.oauth && !explicit_auth {
                    let mut manager = io::mcp_auth::manager(config, credentials).await?;
                    if manager.initialize_from_store().await? {
                        let client = AuthClient::new(client, manager);
                        handler
                            .serve_with_lifecycle(
                                StreamableHttpClientTransport::with_client(
                                    client,
                                    transport_config,
                                ),
                                lifecycle,
                            )
                            .await
                            .map_err(initialize_error)?
                    } else {
                        handler
                            .serve_with_lifecycle(
                                StreamableHttpClientTransport::with_client(
                                    client,
                                    transport_config,
                                ),
                                lifecycle,
                            )
                            .await
                            .map_err(initialize_error)?
                    }
                } else {
                    handler
                        .serve_with_lifecycle(
                            StreamableHttpClientTransport::with_client(client, transport_config),
                            lifecycle,
                        )
                        .await
                        .map_err(initialize_error)?
                }
            };
            Ok::<_, anyhow::Error>(Self {
                service,
                serial: Arc::default(),
                changed,
            })
        };
        let timeout = config
            .startup_timeout_secs
            .unwrap_or(limits::get().extensions.connect_timeout_secs.value);
        let result = match tokio::time::timeout(Duration::from_secs(timeout), operation).await {
            Ok(result) => result.map_err(|e| match e.downcast::<ConnectError>() {
                Ok(error) => error,
                Err(error) => error.into(),
            }),
            Err(_) => Err(ConnectError {
                message: "MCP connection timed out".into(),
                authorization: false,
                challenge: None,
            }),
        };
        if config.url.is_some()
            && config.transport == crate::mcp::Transport::Auto
            && result.as_ref().is_err_and(|e| !e.authorization)
        {
            let mut legacy = config.clone();
            legacy.transport = crate::mcp::Transport::Sse;
            // Only connection negotiation falls back. A tools/call is never replayed.
            return Box::pin(Self::open(&legacy, project, credentials_fallback)).await;
        }
        result
    }

    pub fn close(&self) {
        self.service.cancellation_token().cancel();
    }
    pub fn closed(&self) -> bool {
        self.service.is_closed()
    }
    pub fn changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
    pub fn instructions(&self) -> Option<String> {
        self.service
            .peer()
            .peer_info()
            .and_then(|info| info.instructions.clone())
    }

    pub async fn tools(&self) -> anyhow::Result<Vec<Value>> {
        if !self.supports("tools") {
            return Ok(vec![]);
        }
        let _serial = self.serial.lock().await;
        let mut cursor = None;
        let mut seen = std::collections::HashSet::new();
        let mut tools = Vec::new();
        for _ in 0..limits::get().extensions.list_pages.value {
            let params = cursor
                .clone()
                .map(|cursor| serde_json::from_value(json!({"cursor": cursor})))
                .transpose()?;
            let page = self.service.list_tools(params).await?;
            for tool in page.tools {
                if tools.len() >= limits::get().extensions.tool_count.value {
                    bail!("MCP server exceeded its tool discovery limit");
                }
                tools.push(serde_json::to_value(tool)?);
            }
            cursor = page.next_cursor;
            match &cursor {
                None => return Ok(tools),
                Some(next) if !seen.insert(next.clone()) => {
                    bail!("MCP server repeated a pagination cursor")
                }
                _ => {}
            }
        }
        bail!("MCP server exceeded its pagination limit")
    }

    pub async fn call(
        self: Arc<Self>,
        name: String,
        args: Value,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> anyhow::Result<Value> {
        self.request(
            "tools/call",
            json!({"name":name,"arguments":args}),
            timeout,
            cancel,
        )
        .await
    }

    pub async fn prompts(self: Arc<Self>, timeout: Duration) -> anyhow::Result<Vec<Value>> {
        if !self.supports("prompts") {
            return Ok(vec![]);
        }
        let mut params = json!({});
        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..limits::get().extensions.list_pages.value {
            let result = self
                .clone()
                .request(
                    "prompts/list",
                    params.clone(),
                    timeout,
                    CancellationToken::new(),
                )
                .await?;
            let page = result["prompts"]
                .as_array()
                .context("Malformed MCP prompt list")?;
            items.extend(page.iter().cloned());
            if items.len() > limits::get().extensions.tool_count.value {
                bail!("MCP prompt list exceeds its limit");
            }
            let Some(cursor) = result["nextCursor"].as_str() else {
                return Ok(items);
            };
            if !seen.insert(cursor.to_owned()) {
                bail!("MCP prompt list repeated its cursor");
            }
            params["cursor"] = json!(cursor);
        }
        bail!("MCP prompt list exceeds its page limit")
    }

    pub fn supports(&self, capability: &str) -> bool {
        self.service
            .peer_info()
            .and_then(|p| serde_json::to_value(p.as_ref()).ok())
            .is_some_and(|p| {
                p["capabilities"]
                    .get(capability)
                    .is_some_and(|v| !v.is_null())
            })
    }

    pub async fn request(
        self: Arc<Self>,
        method: &'static str,
        params: Value,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> anyhow::Result<Value> {
        let serial = tokio::select! {
            guard = self.serial.clone().lock_owned() => guard,
            () = cancel.cancelled() => bail!(text().cancelled.clone()),
        };
        // A dropped UI/tool future must still deliver cancellation to the remote request.
        tokio::spawn(async move {
            let _serial = serial;
            if cancel.is_cancelled() || self.closed() { bail!(text().cancelled.clone()); }
            let request: ClientRequest = serde_json::from_value(json!({"method":method, "params":params}))?;
            let peer = self.service.peer();
            let options = PeerRequestOptions::with_timeout(timeout).reset_timeout_on_progress().with_max_total_timeout(limits::get().extensions.call_max_secs.secs());
            let handle = peer.send_cancellable_request(request, options).await?;
            let id = handle.id.clone();
            tokio::select! {
                result = handle.await_response() => {
                    match result {
                        Ok(result) => Ok(serde_json::to_value(result)?),
                        Err(rmcp::service::ServiceError::Timeout { .. }) => bail!(text().timeout.clone()),
                        Err(error) => Err(error.into()),
                    }
                }
                () = cancel.cancelled() => {
                    let notification = serde_json::from_value(json!({"method":"notifications/cancelled","params":{"requestId":id,"reason":"Interrupted"}}))?;
                    let _ = tokio::time::timeout(Duration::from_secs(2), peer.send_notification(notification)).await;
                    bail!(text().cancelled.clone())
                }
            }
        }).await?
    }
}

impl From<anyhow::Error> for ConnectError {
    fn from(e: anyhow::Error) -> Self {
        let authorization = e
            .chain()
            .any(|cause| cause.is::<rmcp::transport::auth::AuthError>());
        Self {
            message: if authorization {
                text().authentication_required.clone()
            } else {
                e.root_cause().to_string()
            },
            authorization,
            challenge: None,
        }
    }
}

impl From<std::io::Error> for ConnectError {
    fn from(e: std::io::Error) -> Self {
        anyhow::Error::from(e).into()
    }
}
impl From<reqwest::Error> for ConnectError {
    fn from(e: reqwest::Error) -> Self {
        anyhow::Error::from(e).into()
    }
}
impl From<rmcp::transport::auth::AuthError> for ConnectError {
    fn from(e: rmcp::transport::auth::AuthError) -> Self {
        Self {
            message: e.to_string(),
            authorization: true,
            challenge: None,
        }
    }
}

#[cfg(test)]
mod sse_tests;
#[cfg(test)]
mod tests;

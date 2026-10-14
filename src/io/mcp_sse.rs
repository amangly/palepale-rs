//! Legacy HTTP+SSE is a transport adapter; lifecycle and RPC stay in the official SDK.
#![allow(clippy::disallowed_methods)]
use super::mcp::ConnectError;
use crate::limits;
use anyhow::Context;
use futures_util::StreamExt;
use rmcp::RoleClient;
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::{Transport, auth::AuthClient};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub struct Sse {
    client: reqwest::Client,
    auth: Option<AuthClient<reqwest::Client>>,
    endpoint: std::sync::Arc<std::sync::RwLock<url::Url>>,
    incoming: mpsc::Receiver<RxJsonRpcMessage<RoleClient>>,
    cancel: CancellationToken,
}

fn response(response: reqwest::Response) -> Result<reqwest::Response, ConnectError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let authorization = matches!(response.status().as_u16(), 401 | 403);
    Err(ConnectError {
        authorization,
        challenge: response
            .headers()
            .get("www-authenticate")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned),
        message: format!(
            "MCP SSE endpoint returned HTTP {}",
            response.status().as_u16()
        ),
    })
}

impl Sse {
    pub async fn open(
        client: reqwest::Client,
        config: &crate::mcp::Config,
        credentials: std::path::PathBuf,
        explicit_auth: bool,
    ) -> Result<Self, ConnectError> {
        let url = url::Url::parse(config.url.as_deref().context("Missing MCP URL")?)
            .map_err(anyhow::Error::from)?;
        let mut auth = None;
        if config.oauth && !explicit_auth && config.bearer_token_env_var.is_none() {
            let mut manager = super::mcp_auth::manager(config, credentials).await?;
            if manager.initialize_from_store().await? {
                auth = Some(AuthClient::new(client.clone(), manager));
            }
        }
        let mut events = open_stream(&client, &auth, &url, None).await?;
        let endpoint = loop {
            let event = events
                .next()
                .await
                .context("SSE stream closed before announcing its endpoint")?
                .map_err(anyhow::Error::from)?;
            if event.event.as_deref() == Some("endpoint") {
                let endpoint = url
                    .join(event.data.as_deref().unwrap_or_default())
                    .map_err(anyhow::Error::from)?;
                if endpoint.origin() != url.origin()
                    || !endpoint.username().is_empty()
                    || endpoint.password().is_some()
                    || endpoint.fragment().is_some()
                {
                    return Err(anyhow::anyhow!(
                        "MCP SSE message endpoint must share the server origin"
                    )
                    .into());
                }
                break endpoint;
            }
        };
        let endpoint = std::sync::Arc::new(std::sync::RwLock::new(endpoint));
        let destination = endpoint.clone();
        let (sender, incoming) = mpsc::channel(32);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let reconnect_client = client.clone();
        let reconnect_auth = auth.clone();
        tokio::spawn(async move {
            let mut last_id = None;
            let mut attempts = 0;
            loop {
                let event =
                    tokio::select! { item = events.next() => item, () = stop.cancelled() => break };
                let Some(Ok(event)) = event else {
                    let mut resumed = false;
                    while attempts < limits::get().extensions.sse_reconnect_attempts.value {
                        attempts += 1;
                        let reconnect = async {
                            tokio::time::sleep(
                                limits::get()
                                    .extensions
                                    .sse_reconnect_ms
                                    .ms()
                                    .saturating_mul(attempts as u32),
                            )
                            .await;
                            tokio::time::timeout(
                                limits::get().extensions.connect_timeout_secs.secs(),
                                open_stream(
                                    &reconnect_client,
                                    &reconnect_auth,
                                    &url,
                                    last_id.as_deref(),
                                ),
                            )
                            .await
                        };
                        let result = tokio::select! { result = reconnect => result, () = stop.cancelled() => return };
                        if let Ok(Ok(stream)) = result {
                            events = stream;
                            resumed = true;
                            break;
                        }
                    }
                    if resumed {
                        continue;
                    }
                    break;
                };
                if let Some(id) = event.id {
                    last_id = if id.is_empty() { None } else { Some(id) };
                }
                if event.event.as_deref() == Some("endpoint") {
                    if let Ok(endpoint) = url.join(event.data.as_deref().unwrap_or_default())
                        && endpoint.origin() == url.origin()
                        && endpoint.username().is_empty()
                        && endpoint.password().is_none()
                        && endpoint.fragment().is_none()
                    {
                        *destination.write().unwrap() = endpoint;
                    } else {
                        break;
                    }
                    continue;
                }
                if event.event.as_deref().is_none_or(|s| s == "message")
                    && let Some(data) = event.data
                {
                    let Ok(message) = serde_json::from_str(&data) else {
                        break;
                    };
                    attempts = 0;
                    tokio::select! { result = sender.send(message) => { if result.is_err() { break; } }, () = stop.cancelled() => break }
                }
            }
        });
        Ok(Self {
            client,
            auth,
            endpoint,
            incoming,
            cancel,
        })
    }
}

impl Transport<RoleClient> for Sse {
    type Error = std::io::Error;
    fn send(
        &mut self,
        message: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let client = self.client.clone();
        let auth = self.auth.clone();
        let endpoint = self.endpoint.read().unwrap().clone();
        let cancel = self.cancel.clone();
        async move {
            let send = async {
                let mut request = client.post(endpoint).json(&message);
                if let Some(auth) = auth {
                    request = request.bearer_auth(
                        auth.get_access_token()
                            .await
                            .map_err(std::io::Error::other)?,
                    );
                }
                request
                    .send()
                    .await
                    .map_err(std::io::Error::other)?
                    .error_for_status()
                    .map_err(std::io::Error::other)?;
                Ok(())
            };
            tokio::select! { result = send => result, () = cancel.cancelled() => Err(std::io::Error::other("MCP SSE connection closed")) }
        }
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        self.incoming.recv().await
    }
    async fn close(&mut self) -> Result<(), Self::Error> {
        self.cancel.cancel();
        Ok(())
    }
}
impl Drop for Sse {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn open_stream(
    client: &reqwest::Client,
    auth: &Option<AuthClient<reqwest::Client>>,
    url: &url::Url,
    last_id: Option<&str>,
) -> Result<
    futures_util::stream::BoxStream<'static, Result<sse_stream::Sse, std::io::Error>>,
    ConnectError,
> {
    let mut request = client
        .get(url.clone())
        .header("accept", "text/event-stream");
    if let Some(auth) = auth {
        request = request.bearer_auth(auth.get_access_token().await.map_err(anyhow::Error::from)?);
    }
    if let Some(id) = last_id {
        request = request.header("last-event-id", id);
    }
    let response = response(request.send().await.map_err(anyhow::Error::from)?)?;
    if !response
        .headers()
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.starts_with("text/event-stream"))
    {
        return Err(anyhow::anyhow!("MCP SSE endpoint did not return an event stream").into());
    }
    let cap = limits::get().tools.bash_spill_max_bytes.value;
    let mut retained = 0usize;
    let mut blank = true;
    let stream = response.bytes_stream().map(move |chunk| {
        let chunk = chunk.map_err(std::io::Error::other)?;
        for byte in &chunk {
            retained = retained.saturating_add(1);
            if retained > cap {
                return Err(std::io::Error::other(
                    "MCP SSE event exceeds its size limit",
                ));
            }
            if *byte == b'\n' {
                if blank {
                    retained = 0;
                }
                blank = true;
            } else if *byte != b'\r' {
                blank = false;
            }
        }
        Ok(chunk)
    });
    Ok(sse_stream::SseStream::from_bytes_stream(stream)
        .map(|event| event.map_err(std::io::Error::other))
        .boxed())
}

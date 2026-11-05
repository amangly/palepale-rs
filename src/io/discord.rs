//! Discord network and credential storage. Errors never include request URLs or tokens.
#![allow(clippy::disallowed_methods)]
use crate::discord::{Incoming, Interaction, data, incoming, snowflake};
use anyhow::{Context, anyhow, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use reqwest::{Method, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use tokio::{
    sync::mpsc,
    time::{Instant, MissedTickBehavior},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::sync::CancellationToken;

#[derive(Serialize, Deserialize)]
struct Stored {
    scheme: String,
    secret: String,
}
pub fn save_token(path: &Path, token: &str) -> anyhow::Result<()> {
    let (scheme, bytes) = super::secret::seal(token.as_bytes())?;
    let stored = Stored {
        scheme: scheme.into(),
        secret: STANDARD.encode(bytes),
    };
    super::fs::write_atomic(
        path,
        &serde_json::to_vec(&stored)?,
        super::fs::Access::Private,
    )?;
    Ok(())
}
pub fn read_token(path: &Path) -> anyhow::Result<String> {
    if let Some(token) = crate::settings::env("DISCORD_BOT_TOKEN") {
        return Ok(token.trim().to_owned());
    }
    let text = super::fs::read_limited(path, 16384)?.context("Run palepale discord setup first")?;
    let read = || -> anyhow::Result<String> {
        let stored: Stored = serde_json::from_str(&text)?;
        Ok(String::from_utf8(super::secret::open(
            &stored.scheme,
            &STANDARD.decode(stored.secret)?,
        )?)?)
    };
    read().map_err(|_| anyhow!("Cannot read the Discord token. Run setup again"))
}

#[derive(Clone)]
pub struct Discord {
    client: reqwest::Client,
    auth: HeaderValue,
    token: Arc<str>,
    base: String,
}

pub struct Destination {
    channel: String,
    messages: Vec<String>,
    sent: Vec<String>,
    interaction: Option<Interaction>,
}
impl Destination {
    pub fn new(channel: String, interaction: Option<Interaction>) -> Self {
        Self {
            channel,
            messages: Vec::new(),
            sent: Vec::new(),
            interaction,
        }
    }
}

impl Discord {
    #[cfg(test)]
    pub fn fixture(base: String) -> Self {
        Self {
            base,
            ..Self::new("fixture-discord-token".into()).unwrap()
        }
    }
    pub fn new(token: String) -> anyhow::Result<Self> {
        ensure!(
            !token.is_empty() && token.len() <= 512 && !token.chars().any(char::is_whitespace),
            "Invalid Discord bot token"
        );
        let mut auth = HeaderValue::from_str(&format!("Bot {token}"))
            .map_err(|_| anyhow!("Invalid Discord bot token"))?;
        auth.set_sensitive(true);
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(data().request_secs))
            .timeout(Duration::from_secs(data().request_secs))
            .user_agent(concat!(
                "Palepale (https://palepale.tech, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ))
            .build()
            .map_err(|_| anyhow!("Cannot create Discord client"))?;
        Ok(Self {
            client,
            auth,
            token: token.into(),
            base: data().api.clone(),
        })
    }

    pub async fn identity(&self) -> anyhow::Result<String> {
        let user = self.request(Method::GET, "/users/@me", None).await?;
        ensure!(user["bot"] == true, "Use a Discord bot token");
        Ok(user["id"]
            .as_str()
            .filter(|id| snowflake(id))
            .context("Discord returned an invalid bot ID")?
            .into())
    }

    pub async fn register(&self, application: &str) -> anyhow::Result<()> {
        // Upsert only our command. Bulk overwrite would delete unrelated commands.
        let command = serde_json::from_str(include_str!("../../data/discord-commands.json"))?;
        self.request(
            Method::POST,
            &format!("/applications/{application}/commands"),
            Some(command),
        )
        .await?;
        Ok(())
    }

    pub async fn ack(
        &self,
        interaction: &Interaction,
        message: Option<&str>,
    ) -> anyhow::Result<()> {
        let body = if let Some(message) = message {
            json!({"type":4,"data":{"content":message,"flags":64,"allowed_mentions":{"parse":[]}}})
        } else {
            json!({"type":5})
        };
        super::clock::timeout(
            Duration::from_secs(2),
            self.request(
                Method::POST,
                &format!(
                    "/interactions/{}/{}/callback",
                    interaction.id, interaction.token
                ),
                Some(body),
            ),
        )
        .await
        .map_err(|_| anyhow!("Discord command acknowledgement timed out"))??;
        Ok(())
    }

    pub async fn say(&self, channel: &str, content: &str) -> anyhow::Result<String> {
        let message = self
            .request(
                Method::POST,
                &format!("/channels/{channel}/messages"),
                Some(message_body(content)),
            )
            .await?;
        Ok(message["id"]
            .as_str()
            .filter(|id| snowflake(id))
            .context("Discord returned an invalid message ID")?
            .into())
    }

    pub async fn channel(&self, channel: &str) -> anyhow::Result<Value> {
        self.request(Method::GET, &format!("/channels/{channel}"), None)
            .await
    }

    pub async fn thread(&self, channel: &str, message: &str) -> anyhow::Result<String> {
        let channel = self
            .request(
                Method::POST,
                &format!("/channels/{channel}/messages/{message}/threads"),
                Some(json!({"name":"Palepale", "auto_archive_duration":1440})),
            )
            .await?;
        Ok(channel["id"]
            .as_str()
            .filter(|id| snowflake(id))
            .context("Discord returned an invalid thread ID")?
            .into())
    }

    /// Edit existing chunks and append new ones. No model-produced mention can ping anyone.
    pub async fn display(&self, target: &mut Destination, text: &str) -> anyhow::Result<()> {
        for (index, content) in crate::discord::chunks(text, data().message_units)
            .iter()
            .enumerate()
        {
            if target.sent.get(index) == Some(content) {
                continue;
            }
            if index == 0
                && let Some(interaction) = &target.interaction
            {
                self.request(
                    Method::PATCH,
                    &format!(
                        "/webhooks/{}/{}/messages/@original",
                        interaction.application, interaction.token
                    ),
                    Some(message_body(content)),
                )
                .await?;
                if target.messages.is_empty() {
                    target.messages.push(String::new());
                }
            } else if let Some(id) = target.messages.get(index) {
                self.request(
                    Method::PATCH,
                    &format!("/channels/{}/messages/{id}", target.channel),
                    Some(message_body(content)),
                )
                .await?;
            } else {
                target
                    .messages
                    .push(self.say(&target.channel, content).await?);
            }
            if let Some(sent) = target.sent.get_mut(index) {
                sent.clone_from(content);
            } else {
                target.sent.push(content.clone());
            }
        }
        Ok(())
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        for attempt in 0..3 {
            let mut request = self
                .client
                .request(method.clone(), format!("{}{path}", self.base))
                .header("Authorization", self.auth.clone());
            if let Some(body) = &body {
                request = request.json(body);
            }
            let mut response = request
                .send()
                .await
                .map_err(|_| anyhow!("Discord request failed; delivery is unconfirmed"))?;
            let status = response.status();
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow!("Discord response was interrupted"))?
            {
                ensure!(
                    bytes.len() + chunk.len() <= data().max_event_bytes,
                    "Discord response exceeded its limit"
                );
                bytes.extend_from_slice(&chunk);
            }
            // Only an explicit 429 is retried. A timeout/5xx may already have delivered a message.
            if status.as_u16() == 429 && attempt < 2 {
                let retry = serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .and_then(|v| v["retry_after"].as_f64());
                if let Some(seconds) =
                    retry.filter(|s| s.is_finite() && *s >= 0.0 && *s <= data().request_secs as f64)
                {
                    super::clock::sleep(Duration::from_secs_f64(seconds.max(0.05))).await;
                    continue;
                }
            }
            ensure!(
                status.is_success(),
                "Discord request failed (HTTP {})",
                status.as_u16()
            );
            return if bytes.is_empty() {
                Ok(Value::Null)
            } else {
                serde_json::from_slice(&bytes).map_err(|_| anyhow!("Discord returned invalid JSON"))
            };
        }
        unreachable!("last attempt returns")
    }

    pub async fn gateway(
        &self,
        events: mpsc::Sender<Incoming>,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut resume = Resume::default();
        let mut backoff = data().reconnect_secs;
        loop {
            let connected = Instant::now();
            let result = tokio::select! { biased; () = cancel.cancelled() => return Ok(()), result = self.connection(&data().gateway, &mut resume, &events) => result };
            match result {
                Err(GatewayError::Fatal(message)) => return Err(anyhow!(message)),
                _ => super::term::err("Discord disconnected. Reconnecting…\n"),
            }
            if connected.elapsed().as_secs() > 60 {
                backoff = data().reconnect_secs;
            }
            tokio::select! { () = cancel.cancelled() => return Ok(()), () = super::clock::sleep(Duration::from_secs(backoff)) => {} }
            backoff = (backoff * 2).min(data().max_reconnect_secs);
        }
    }

    async fn connection(
        &self,
        gateway: &str,
        resume: &mut Resume,
        events: &mpsc::Sender<Incoming>,
    ) -> Result<(), GatewayError> {
        let url = resume.url.as_deref().unwrap_or(gateway);
        let config = WebSocketConfig::default()
            .max_message_size(Some(data().max_event_bytes))
            .max_frame_size(Some(data().max_event_bytes));
        let (mut socket, _) = tokio::time::timeout(
            Duration::from_secs(data().request_secs),
            connect_async_with_config(url, Some(config), false),
        )
        .await
        .map_err(|_| GatewayError::Reconnect)?
        .map_err(|_| GatewayError::Reconnect)?;
        let hello = tokio::time::timeout(Duration::from_secs(data().request_secs), socket.next())
            .await
            .map_err(|_| GatewayError::Reconnect)?
            .ok_or(GatewayError::Reconnect)?
            .map_err(|_| GatewayError::Reconnect)?;
        if let Message::Close(frame) = &hello {
            return Err(close(frame.as_ref().map(|f| u16::from(f.code)), resume));
        }
        let hello: Value =
            serde_json::from_str(hello.to_text().map_err(|_| GatewayError::Reconnect)?)
                .map_err(|_| GatewayError::Reconnect)?;
        let ms = hello["d"]["heartbeat_interval"]
            .as_u64()
            .filter(|ms| *ms > 0 && *ms <= 120_000)
            .ok_or(GatewayError::Reconnect)?;
        if hello["op"] != 10 {
            return Err(GatewayError::Reconnect);
        }
        let identify = if let Some(session) = &resume.session {
            json!({"op":6,"d":{"token":self.token.as_ref(),"session_id":session,"seq":resume.seq}})
        } else {
            json!({"op":2,"d":{"token":self.token.as_ref(),"intents":1|512|4096|32768,"properties":{"os":std::env::consts::OS,"browser":"palepale","device":"palepale"}}})
        };
        socket
            .send(Message::Text(identify.to_string().into()))
            .await
            .map_err(|_| GatewayError::Reconnect)?;
        let mut heartbeat = tokio::time::interval_at(
            Instant::now() + Duration::from_millis(fastrand::u64(0..ms)),
            Duration::from_millis(ms),
        );
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut acknowledged = true;
        loop {
            let heartbeat_due = tokio::select! {
                _ = heartbeat.tick() => { if !acknowledged { return Err(GatewayError::Reconnect); } true },
                message = socket.next() => {
                    let message = message.ok_or(GatewayError::Reconnect)?.map_err(|_| GatewayError::Reconnect)?;
                    match message {
                        Message::Close(frame) => return Err(close(frame.map(|f| u16::from(f.code)), resume)),
                        Message::Ping(bytes) => { socket.send(Message::Pong(bytes)).await.map_err(|_| GatewayError::Reconnect)?; false },
                        Message::Text(text) => {
                            let packet: Value = serde_json::from_str(&text).map_err(|_| GatewayError::Reconnect)?;
                            match packet["op"].as_u64() {
                                Some(0) => {
                                    resume.seq = packet["s"].as_u64().or(resume.seq);
                                    if packet["t"] == "READY" {
                                        resume.session = packet["d"]["session_id"].as_str().map(str::to_owned);
                                        resume.bot = packet["d"]["user"]["id"].as_str().unwrap_or_default().into();
                                        resume.url = packet["d"]["resume_gateway_url"].as_str().and_then(resume_url);
                                        super::term::out("Discord connected.\n");
                                    }
                                    if let Some(event) = incoming(packet["t"].as_str().unwrap_or_default(), &packet["d"], &resume.bot)
                                        && events.try_send(event).is_err() {
                                        super::term::err("Discord event queue is full. Message skipped.\n");
                                    }
                                    false
                                },
                                Some(1) => true,
                                Some(7) => return Err(GatewayError::Reconnect),
                                Some(9) => { if packet["d"] != true { *resume = Resume::default(); } return Err(GatewayError::Reconnect); },
                                Some(11) => { acknowledged = true; false },
                                _ => false,
                            }
                        },
                        _ => false,
                    }
                }
            };
            if heartbeat_due {
                socket
                    .send(Message::Text(
                        json!({"op":1,"d":resume.seq}).to_string().into(),
                    ))
                    .await
                    .map_err(|_| GatewayError::Reconnect)?;
                acknowledged = false;
            }
        }
    }
}

fn message_body(content: &str) -> Value {
    json!({"content":content,"allowed_mentions":{"parse":[],"replied_user":false}})
}
#[derive(Default)]
struct Resume {
    seq: Option<u64>,
    session: Option<String>,
    url: Option<String>,
    bot: String,
}
#[derive(Debug)]
enum GatewayError {
    Reconnect,
    Fatal(&'static str),
}
fn close(code: Option<u16>, resume: &mut Resume) -> GatewayError {
    match code {
        Some(4004) => GatewayError::Fatal("Discord rejected the bot token. Run setup again"),
        Some(4010..=4014) => GatewayError::Fatal(
            "Discord rejected the Gateway settings. Enable Message Content Intent in the Developer Portal",
        ),
        Some(4007 | 4009) => {
            *resume = Resume::default();
            GatewayError::Reconnect
        }
        _ => GatewayError::Reconnect,
    }
}
fn resume_url(value: &str) -> Option<String> {
    let mut url = url::Url::parse(value).ok()?;
    if url.scheme() != "wss"
        || !url.username().is_empty()
        || url.password().is_some()
        || !url
            .host_str()
            .is_some_and(|h| h == "gateway.discord.gg" || h.ends_with(".discord.gg"))
    {
        return None;
    }
    url.set_query(Some("v=10&encoding=json"));
    Some(url.into())
}

#[cfg(test)]
mod tests;

//! MCP configuration, connection state, and the agent's dynamic tool catalog.

mod tool;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::extensions::{Scope, text};
use crate::paths::Paths;
use crate::{io, limits, settings, trust};

use tool::Remote;
pub use tool::Search;

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub env_vars: Vec<String>,
    #[serde(default)]
    pub bearer_token_env_var: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub startup_timeout_secs: Option<u64>,
    #[serde(default)]
    pub tool_timeout_secs: Option<u64>,
    #[serde(default)]
    pub read_only_tools: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            command: None,
            args: vec![],
            url: None,
            enabled: true,
            env_vars: vec![],
            bearer_token_env_var: None,
            client_id: None,
            scopes: vec![],
            startup_timeout_secs: None,
            tool_timeout_secs: None,
            read_only_tools: vec![],
        }
    }
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.command.is_some() == self.url.is_some() {
            bail!("Set either command or url for an MCP server");
        }
        if self.command.as_ref().is_some_and(|s| s.trim().is_empty()) {
            bail!("MCP command is empty");
        }
        if let Some(raw) = &self.url {
            let url = url::Url::parse(raw)?;
            let local = url
                .host_str()
                .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1"));
            if url.scheme() != "https" && !(url.scheme() == "http" && local) {
                bail!("Use HTTPS for a remote MCP server");
            }
            if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
                bail!("MCP URL cannot contain login credentials or a fragment");
            }
        }
        for timeout in [self.startup_timeout_secs, self.tool_timeout_secs]
            .into_iter()
            .flatten()
        {
            if timeout == 0 || timeout > 3600 {
                bail!("MCP timeout must be between 1 and 3600 seconds");
            }
        }
        Ok(())
    }

    pub fn address(&self) -> String {
        if let Some(raw) = &self.url {
            url::Url::parse(raw)
                .map(|mut u| {
                    u.set_query(None);
                    u.to_string()
                })
                .unwrap_or_else(|_| raw.clone())
        } else {
            self.command.clone().unwrap_or_default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    Untrusted,
    Review,
    Ready,
    Connecting,
    Connected,
    SignIn,
    Error,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Untrusted => "untrusted",
            Self::Review => "review",
            Self::Ready => "ready",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::SignIn => "sign in",
            Self::Error => "error",
        }
    }
}

struct Server {
    config: Config,
    scope: Scope,
    state: State,
    message: Option<String>,
    challenge: Option<String>,
    generation: u64,
    connection: Option<Arc<io::mcp::Connection>>,
    tools: Vec<Arc<Remote>>,
    connect_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Clone)]
pub struct View {
    pub name: String,
    pub config: Config,
    pub scope: Scope,
    pub state: State,
    pub message: Option<String>,
    pub tools: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Runtime {
    servers: RwLock<BTreeMap<String, Server>>,
    selected: RwLock<VecDeque<String>>,
    revision: AtomicU64,
}

fn fingerprint(name: &str, config: &Config, project: Option<&std::path::Path>) -> String {
    let value = serde_json::to_vec(&(name, config, project)).expect("configuration serializes");
    crate::extensions::hash(&value)
}

fn reviewed(paths: &Paths) -> anyhow::Result<BTreeSet<String>> {
    match io::fs::read_string(&paths.home.join("mcp/reviewed.json"))? {
        Some(text) => Ok(serde_json::from_str(&text)?),
        None => Ok(BTreeSet::new()),
    }
}

pub fn credential_path(paths: &Paths, config: &Config) -> PathBuf {
    let identity = serde_json::to_vec(&(&config.url, &config.client_id, &config.scopes))
        .expect("identity serializes");
    paths
        .home
        .join("mcp/auth")
        .join(format!("{}.json", crate::extensions::hash(&identity)))
}

impl Runtime {
    pub fn reload(&self, app: &App) -> anyhow::Result<()> {
        let mut definitions: BTreeMap<_, _> =
            settings::load(&app.paths, &settings::Overrides::default())?
                .mcp_servers
                .into_iter()
                .map(|(name, config)| (name, (config, Scope::User)))
                .collect();
        for (file, scope) in [
            (app.paths.user_config(), Scope::User),
            (app.paths.project_config(), Scope::Project),
        ] {
            if scope == Scope::Project && io::fs::same_file(&file, &app.paths.user_config()) {
                continue;
            }
            if let Some(source) = io::fs::read_string(&file)? {
                let document: toml::Table = toml::from_str(&source)?;
                if let Some(table) = document.get("mcp_servers").and_then(toml::Value::as_table) {
                    for (name, value) in table {
                        let config: Config = value
                            .clone()
                            .try_into()
                            .with_context(|| format!("MCP server {name}"))?;
                        definitions.insert(name.clone(), (config, scope));
                    }
                }
            }
        }
        let trusted = trust::is_trusted(&app.paths).unwrap_or(false);
        let reviewed = reviewed(&app.paths)?;
        let mut servers = self.servers.write().unwrap();
        servers.retain(|name, server| {
            let keep = definitions.contains_key(name);
            if !keep && let Some(connection) = &server.connection {
                connection.close();
            }
            keep
        });
        for (name, (config, scope)) in definitions {
            if servers.get(&name).is_some_and(|s| {
                s.config == config
                    && s.scope == scope
                    && !(trusted && s.state == State::Untrusted)
                    && !(!trusted && scope == Scope::Project && s.state != State::Untrusted)
            }) {
                continue;
            }
            if let Some(old) = servers.remove(&name)
                && let Some(connection) = old.connection
            {
                connection.close();
            }
            let invalid = if !crate::skills::valid_name(&name) {
                Some("Invalid MCP server name".into())
            } else {
                config.validate().err().map(|e| e.to_string())
            };
            let state = if invalid.is_some() {
                State::Error
            } else if !config.enabled {
                State::Off
            } else if scope == Scope::Project && !trusted {
                State::Untrusted
            } else if scope == Scope::Project
                && !reviewed.contains(&fingerprint(&name, &config, Some(&app.paths.project)))
            {
                State::Review
            } else {
                State::Ready
            };
            servers.insert(
                name,
                Server {
                    config,
                    scope,
                    state,
                    message: invalid,
                    challenge: None,
                    generation: self.revision.fetch_add(1, Ordering::Relaxed),
                    connection: None,
                    tools: vec![],
                    connect_lock: Arc::default(),
                },
            );
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<View> {
        self.servers
            .read()
            .unwrap()
            .iter()
            .map(|(name, server)| View {
                name: name.clone(),
                config: server.config.clone(),
                scope: server.scope,
                state: if server.state == State::Connected
                    && server.connection.as_ref().is_some_and(|c| c.closed())
                {
                    State::Error
                } else {
                    server.state
                },
                message: server.message.clone(),
                tools: server
                    .tools
                    .iter()
                    .map(|t| (t.remote_name.clone(), t.description.clone()))
                    .collect(),
            })
            .collect()
    }

    pub async fn start_enabled(self: &Arc<Self>, app: &App) {
        if settings::env("PALEPALE_OFFLINE").is_some() {
            return;
        }
        let _ = self.reload(app);
        let names: Vec<_> = self
            .list()
            .into_iter()
            .filter(|s| matches!(s.state, State::Ready | State::Connecting))
            .map(|s| s.name)
            .collect();
        futures_util::future::join_all(names.iter().map(|name| self.connect(app, name, false)))
            .await;
    }

    pub async fn connect(
        self: &Arc<Self>,
        app: &App,
        name: &str,
        review: bool,
    ) -> anyhow::Result<()> {
        if settings::env("PALEPALE_OFFLINE").is_some() {
            bail!("MCP connections are disabled in offline mode");
        }
        let connect_lock = self
            .servers
            .read()
            .unwrap()
            .get(name)
            .context("MCP server was removed")?
            .connect_lock
            .clone();
        let _connect = connect_lock.lock().await;
        let (config, generation) = {
            let mut servers = self.servers.write().unwrap();
            let server = servers.get_mut(name).context("MCP server was removed")?;
            if server.state == State::Connected
                && server.connection.as_ref().is_some_and(|c| !c.closed())
            {
                return Ok(());
            }
            if server.state == State::Untrusted
                || (server.scope == Scope::Project && !trust::is_trusted(&app.paths)?)
            {
                bail!(text().untrusted.clone());
            }
            if !server.config.enabled {
                bail!("MCP server is disabled");
            }
            server.config.validate()?;
            if server.state == State::Review {
                if !review {
                    bail!(text().unreviewed.clone());
                }
                let mut accepted = reviewed(&app.paths)?;
                accepted.insert(fingerprint(name, &server.config, Some(&app.paths.project)));
                io::fs::write_atomic(
                    &app.paths.home.join("mcp/reviewed.json"),
                    &serde_json::to_vec(&accepted)?,
                    io::fs::Access::Private,
                )?;
            }
            server.state = State::Connecting;
            server.message = None;
            (server.config.clone(), server.generation)
        };
        let _pending = Pending {
            runtime: self.clone(),
            name: name.into(),
            generation,
        };
        let result = match io::mcp::Connection::open(
            &config,
            &app.paths.project,
            credential_path(&app.paths, &config),
        )
        .await
        {
            Ok(connection) => {
                let connection = Arc::new(connection);
                let timeout = config
                    .startup_timeout_secs
                    .unwrap_or(limits::get().extensions.connect_timeout_secs.value);
                match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout),
                    connection.tools(),
                )
                .await
                {
                    Ok(Ok(tools)) => Ok((connection, tools)),
                    other => {
                        connection.close();
                        let message = match other {
                            Ok(Err(e)) => format!("{e:#}"),
                            _ => "MCP tool discovery timed out".into(),
                        };
                        Err(io::mcp::ConnectError {
                            message,
                            authorization: false,
                            challenge: None,
                        })
                    }
                }
            }
            Err(error) => Err(error),
        };
        let mut servers = self.servers.write().unwrap();
        let Some(server) = servers
            .get_mut(name)
            .filter(|s| s.generation == generation && s.config.enabled)
        else {
            if let Ok((connection, _)) = result {
                connection.close();
            }
            bail!("MCP server changed while connecting");
        };
        match result {
            Ok((connection, tools)) => {
                let mut errors = Vec::new();
                let mut names = BTreeSet::new();
                server.tools = tools
                    .into_iter()
                    .filter_map(|value| {
                        match Remote::new(name, &config, generation, connection.clone(), value) {
                            Ok(tool) if names.insert(tool.remote_name.clone()) => {
                                Some(Arc::new(tool))
                            }
                            Ok(tool) => {
                                errors.push(format!("Duplicate tool: {}", tool.remote_name));
                                None
                            }
                            Err(e) => {
                                errors.push(e.to_string());
                                None
                            }
                        }
                    })
                    .collect();
                server.connection = Some(connection);
                server.state = State::Connected;
                server.challenge = None;
                server.message = (!errors.is_empty()).then(|| errors.join("\n"));
                Ok(())
            }
            Err(error) => {
                server.state = if error.authorization {
                    State::SignIn
                } else {
                    State::Error
                };
                server.message = Some(if error.authorization {
                    format!(
                        "{}\n{}",
                        redact(&config, &error.message),
                        text().missing_auth
                    )
                } else {
                    redact(&config, &error.message)
                });
                server.challenge = error.challenge;
                bail!(server.message.clone().unwrap_or_default())
            }
        }
    }

    pub fn disconnect(&self, name: &str) {
        if let Some(server) = self.servers.write().unwrap().get_mut(name) {
            if let Some(connection) = server.connection.take() {
                connection.close();
            }
            server.tools.clear();
            server.generation = self.revision.fetch_add(1, Ordering::Relaxed);
            server.state = match server.state {
                State::Untrusted | State::Review | State::Off => server.state,
                _ if !server.config.enabled => State::Off,
                _ => State::Ready,
            };
        }
    }

    pub async fn refresh_changed(self: &Arc<Self>, app: &App) {
        let names: Vec<_> = self
            .servers
            .read()
            .unwrap()
            .iter()
            .filter(|(_, s)| s.connection.as_ref().is_some_and(|c| c.changed()))
            .map(|(n, _)| n.clone())
            .collect();
        for name in names {
            self.disconnect(&name);
            let _ = self.connect(app, &name, false).await;
        }
    }

    pub fn active(&self, server: &str, generation: u64) -> bool {
        self.servers.read().unwrap().get(server).is_some_and(|s| {
            s.generation == generation && s.state == State::Connected && s.config.enabled
        })
    }

    pub fn tools(self: &Arc<Self>) -> Vec<Arc<dyn crate::tools::Tool>> {
        let servers = self.servers.read().unwrap();
        let all: Vec<_> = servers
            .values()
            .filter(|s| s.state == State::Connected)
            .flat_map(|s| s.tools.iter())
            .collect();
        let total: usize = all.iter().map(|t| t.schema_bytes).sum();
        if total <= limits::get().extensions.tool_catalog_bytes.value {
            return all
                .into_iter()
                .cloned()
                .map(|t| t as Arc<dyn crate::tools::Tool>)
                .collect();
        }
        let selected = self.selected.read().unwrap();
        let mut tools: Vec<Arc<dyn crate::tools::Tool>> = all
            .into_iter()
            .filter(|t| selected.contains(&t.alias))
            .cloned()
            .map(|t| t as Arc<dyn crate::tools::Tool>)
            .collect();
        tools.push(Arc::new(Search));
        tools
    }

    pub fn context(&self) -> String {
        let servers = self.servers.read().unwrap();
        if servers.is_empty() {
            return String::new();
        }
        let mut lines = String::new();
        for (name, server) in servers.iter() {
            let instructions = server
                .connection
                .as_ref()
                .and_then(|c| c.instructions())
                .unwrap_or_default();
            let row = format!(
                "- {name}: {} ({} tools)\n{instructions}\n",
                server.state.label(),
                server.tools.len()
            );
            let remaining = limits::get()
                .extensions
                .skill_catalog_chars
                .value
                .saturating_sub(lines.chars().count());
            lines.extend(row.chars().take(remaining));
            if row.chars().count() >= remaining {
                break;
            }
        }
        crate::text::template::fill(&text().mcp_frame, &[("servers", &lines)])
    }

    pub fn search(&self, query: &str) -> Vec<(String, String)> {
        let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
        let servers = self.servers.read().unwrap();
        let mut tools: Vec<_> = servers
            .values()
            .filter(|s| s.state == State::Connected)
            .flat_map(|s| s.tools.iter())
            .filter_map(|tool| {
                let haystack = format!("{} {} {}", tool.server, tool.remote_name, tool.description)
                    .to_lowercase();
                let score = words
                    .iter()
                    .filter(|word| haystack.contains(word.as_str()))
                    .count();
                (score > 0).then_some((score, tool))
            })
            .collect();
        tools.sort_by(|(a, x), (b, y)| b.cmp(a).then_with(|| x.alias.cmp(&y.alias)));
        let results: Vec<_> = tools
            .into_iter()
            .take(limits::get().extensions.search_results.value)
            .map(|(_, t)| (t.alias.clone(), t.description.clone(), t.schema_bytes))
            .collect();
        let mut selected = self.selected.write().unwrap();
        for (alias, _, _) in results.iter().rev() {
            selected.retain(|s| s != alias);
            selected.push_back(alias.clone());
        }
        let sizes: BTreeMap<_, _> = servers
            .values()
            .flat_map(|s| s.tools.iter())
            .map(|t| (t.alias.as_str(), t.schema_bytes))
            .collect();
        selected.retain(|alias| sizes.contains_key(alias.as_str()));
        while selected.len() > 1
            && selected
                .iter()
                .filter_map(|a| sizes.get(a.as_str()))
                .sum::<usize>()
                > limits::get().extensions.tool_catalog_bytes.value
        {
            selected.pop_front();
        }
        results
            .into_iter()
            .filter(|(a, _, _)| selected.contains(a))
            .map(|(a, d, _)| (a, d))
            .collect()
    }

    pub fn challenge(&self, name: &str) -> Option<String> {
        self.servers
            .read()
            .unwrap()
            .get(name)
            .and_then(|s| s.challenge.clone())
    }
}

struct Pending {
    runtime: Arc<Runtime>,
    name: String,
    generation: u64,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(server) = self.runtime.servers.write().unwrap().get_mut(&self.name)
            && server.generation == self.generation
            && server.state == State::Connecting
        {
            server.state = State::Ready;
        }
    }
}

pub fn save(app: &App, name: &str, config: &Config, scope: Scope) -> anyhow::Result<()> {
    if !crate::skills::valid_name(name) {
        bail!("Server names use lowercase letters, digits and single hyphens (1–64 characters)");
    }
    config.validate()?;
    let source = toml::to_string(config)?;
    let definition: toml_edit::DocumentMut = source.parse()?;
    settings::edit(&app.paths, scope == Scope::Project, |doc| {
        doc["mcp_servers"][name] = toml_edit::Item::Table(definition.as_table().clone());
        Ok(())
    })?;
    app.mcp.reload(app)
}

pub fn remove(app: &App, name: &str, scope: Scope) -> anyhow::Result<()> {
    settings::edit(&app.paths, scope == Scope::Project, |doc| {
        if let Some(table) = doc
            .get_mut("mcp_servers")
            .and_then(toml_edit::Item::as_table_mut)
        {
            table.remove(name);
        }
        Ok(())
    })?;
    app.mcp.reload(app)
}

pub fn redact(config: &Config, source: &str) -> String {
    let mut text = source.to_owned();
    for name in config.bearer_token_env_var.iter().chain(&config.env_vars) {
        if let Some(value) = settings::env(name)
            && !value.is_empty()
        {
            text = text.replace(&value, "[redacted]");
        }
    }
    text
}

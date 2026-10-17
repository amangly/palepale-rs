use std::sync::LazyLock;

use anyhow::bail;
use clap::{Parser, Subcommand};
use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::extensions::{Kind, Scope as Placement, text};
use crate::text::template::fill;
use crate::{io, mcp};

const SRC: &str = include_str!("../../data/commands/mcp.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SRC)
        .expect("MCP command copy parses")
        .text
});

#[derive(Deserialize)]
struct Text {
    added: String,
    checked: String,
    removed: String,
    exists: String,
    missing: String,
    invalid_endpoint: String,
    login: String,
    logged_in: String,
    logged_out: String,
    unavailable: String,
    toggled: String,
    discovered: String,
    discovery_empty: String,
    discovery_changed: String,
    imported: String,
}

#[derive(Parser)]
#[command(name = "mcp", disable_help_subcommand = true)]
struct Options {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
// Parsed once per command; boxing individual options would obscure clap's shape.
#[allow(clippy::large_enum_variant)]
enum Action {
    List,
    Discover,
    Import {
        name: String,
        #[arg(long)]
        fingerprint: String,
        #[arg(long, value_parser = ["user", "project"], default_value = "user")]
        scope: String,
    },
    Add {
        name: String,
        #[arg(long, value_parser = ["user", "project"], default_value = "user")]
        scope: String,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        cwd: Option<std::path::PathBuf>,
        #[arg(long = "env")]
        environment: Vec<String>,
        #[arg(long = "header")]
        http_headers: Vec<String>,
        #[arg(long = "header-env")]
        env_http_headers: Vec<String>,
        #[arg(long, value_parser = ["auto", "http", "sse"], default_value = "auto")]
        transport: String,
        #[arg(long)]
        no_oauth: bool,
        #[arg(long)]
        bearer_token_env_var: Option<String>,
        #[arg(long)]
        client_id: Option<String>,
        #[arg(long)]
        client_secret_env_var: Option<String>,
        #[arg(long)]
        oauth_callback_port: Option<u16>,
        #[arg(long = "oauth-scope")]
        scopes: Vec<String>,
        #[arg(long = "env-var")]
        env_vars: Vec<String>,
        #[arg(long)]
        startup_timeout_secs: Option<u64>,
        #[arg(long)]
        tool_timeout_secs: Option<u64>,
        #[arg(last = true, num_args = 1..)]
        command: Vec<String>,
    },
    Check {
        name: String,
    },
    #[command(alias = "login")]
    Auth {
        name: String,
    },
    Logout {
        name: String,
    },
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
    Remove {
        name: String,
        #[arg(long, value_parser = ["user", "project"], default_value = "user")]
        scope: String,
    },
}

pub struct Mcp;

impl Command for Mcp {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            if args.is_empty() && cx.scope == Scope::Session {
                return Ok(Outcome {
                    text: String::new(),
                    effects: vec![Effect::ShowExtensions(Kind::Mcp)],
                });
            }
            let options = match Options::try_parse_from(
                std::iter::once("mcp").chain(args.iter().map(String::as_str)),
            ) {
                Ok(options) => options,
                Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
                    return Ok(error.to_string().into());
                }
                Err(error) => bail!("{error}"),
            };
            let app = cx.app;
            app.mcp.reload(app)?;
            let output = match options.action.unwrap_or(Action::List) {
                Action::Discover => {
                    let rows: Vec<_> = mcp::discover::found(&app.paths.project)
                        .into_iter()
                        .map(|found| {
                            fill(
                                &TEXT.discovered,
                                &[
                                    ("name", &found.name),
                                    ("source", &found.source),
                                    ("scope", found.scope.label()),
                                    ("fingerprint", &found.fingerprint),
                                ],
                            )
                        })
                        .collect();
                    if rows.is_empty() {
                        TEXT.discovery_empty.clone()
                    } else {
                        rows.join("\n")
                    }
                }
                Action::Import {
                    name,
                    fingerprint,
                    scope,
                } => {
                    let found = mcp::discover::found(&app.paths.project)
                        .into_iter()
                        .find(|found| found.name == name && found.fingerprint == fingerprint)
                        .ok_or_else(|| anyhow::anyhow!(TEXT.discovery_changed.clone()))?;
                    let scope = if scope == "project" {
                        Placement::Project
                    } else {
                        Placement::User
                    };
                    if app.mcp.list().iter().any(|server| server.name == name) {
                        bail!(fill(
                            &TEXT.exists,
                            &[
                                ("name", &name),
                                ("file", &app.paths.user_config().display().to_string())
                            ]
                        ));
                    }
                    let mut config = found.config;
                    config.enabled = false;
                    mcp::save(app, &name, &config, scope)?;
                    fill(&TEXT.imported, &[("name", &name)])
                }
                Action::List => {
                    let rows: Vec<_> = app
                        .mcp
                        .list()
                        .into_iter()
                        .map(|server| {
                            format!(
                                "{}  {}  {} tools  {}",
                                server.name,
                                server.state.label(),
                                server.tools.len(),
                                server.scope.label()
                            )
                        })
                        .collect();
                    if rows.is_empty() {
                        text().empty_mcp.clone()
                    } else {
                        rows.join("\n")
                    }
                }
                Action::Add {
                    name,
                    scope,
                    url,
                    cwd,
                    environment,
                    http_headers,
                    env_http_headers,
                    transport,
                    no_oauth,
                    command,
                    bearer_token_env_var,
                    client_id,
                    client_secret_env_var,
                    oauth_callback_port,
                    scopes,
                    env_vars,
                    startup_timeout_secs,
                    tool_timeout_secs,
                } => {
                    if url.is_some() == !command.is_empty() {
                        bail!(TEXT.invalid_endpoint.clone());
                    }
                    let scope = placement(&scope);
                    let file = config_file(cx, scope);
                    if contains(&file, &name)? {
                        bail!(fill(
                            &TEXT.exists,
                            &[("name", &name), ("file", &file.display().to_string())]
                        ));
                    }
                    let mut program = command.into_iter();
                    let config = mcp::Config {
                        url,
                        cwd,
                        environment: pairs(environment)?,
                        http_headers: pairs(http_headers)?,
                        env_http_headers: pairs(env_http_headers)?,
                        transport: match transport.as_str() {
                            "http" => mcp::Transport::Http,
                            "sse" => mcp::Transport::Sse,
                            _ => mcp::Transport::Auto,
                        },
                        oauth: !no_oauth,
                        command: program.next(),
                        args: program.collect(),
                        bearer_token_env_var,
                        client_id,
                        client_secret_env_var,
                        oauth_callback_port,
                        scopes,
                        env_vars,
                        startup_timeout_secs,
                        tool_timeout_secs,
                        ..Default::default()
                    };
                    mcp::save(app, &name, &config, scope)?;
                    fill(
                        &TEXT.added,
                        &[
                            ("name", &name),
                            ("scope", scope.label()),
                            ("file", &file.display().to_string()),
                        ],
                    )
                }
                Action::Check { name } => {
                    // Checking never grants project trust or accepts a changed definition.
                    app.mcp.connect(app, &name, false).await?;
                    let server = app
                        .mcp
                        .list()
                        .into_iter()
                        .find(|s| s.name == name)
                        .expect("connected server exists");
                    let mut result = fill(
                        &TEXT.checked,
                        &[("name", &name), ("count", &server.tools.len().to_string())],
                    );
                    for (tool, _) in server.tools {
                        result.push_str(&format!("\n  {tool}"));
                    }
                    if let Some(message) = server.message {
                        result.push_str(&format!("\n{message}"));
                    }
                    result
                }
                Action::Auth { name } => {
                    let server = find(cx, &name)?;
                    if matches!(
                        server.state,
                        mcp::State::Off | mcp::State::Untrusted | mcp::State::Review
                    ) {
                        bail!(TEXT.unavailable.clone());
                    }
                    if cx.scope == Scope::Session {
                        return Ok(Outcome {
                            text: String::new(),
                            effects: vec![Effect::McpAuth(name)],
                        });
                    }
                    let challenge = app.mcp.challenge(&name);
                    let login = tokio::time::timeout(
                        crate::limits::get().extensions.connect_timeout_secs.secs(),
                        io::mcp_auth::Login::begin(
                            &server.config,
                            mcp::credential_path(&app.paths, &server.config),
                            challenge.as_deref(),
                        ),
                    )
                    .await
                    .map_err(|_| anyhow::anyhow!("OAuth discovery timed out"))??;
                    io::term::out(&format!(
                        "{}\n",
                        fill(&TEXT.login, &[("name", &name), ("url", login.url())])
                    ));
                    if crate::settings::env("PALEPALE_NO_BROWSER").is_none() {
                        let _ = io::proc::open_url(login.url());
                    }
                    let cancel = tokio_util::sync::CancellationToken::new();
                    tokio::select! { result = login.finish(&cancel) => result.map_err(|e|anyhow::anyhow!(mcp::redact(&server.config, &e.to_string())))?, () = io::term::interrupted() => { cancel.cancel(); bail!("Sign-in cancelled"); } }
                    app.mcp.disconnect(&name);
                    fill(&TEXT.logged_in, &[("name", &name)])
                }
                Action::Logout { name } => {
                    let server = find(cx, &name)?;
                    app.mcp.disconnect(&name);
                    io::mcp_auth::Store::new(mcp::credential_path(&app.paths, &server.config))
                        .logout()
                        .await?;
                    fill(&TEXT.logged_out, &[("name", &name)])
                }
                action @ (Action::Enable { .. } | Action::Disable { .. }) => {
                    let (name, enabled) = match action {
                        Action::Enable { name } => (name, true),
                        Action::Disable { name } => (name, false),
                        _ => unreachable!(),
                    };
                    let mut server = find(cx, &name)?;
                    server.config.enabled = enabled;
                    mcp::save(app, &name, &server.config, server.scope)?;
                    fill(
                        &TEXT.toggled,
                        &[
                            ("name", &name),
                            ("state", if enabled { "enabled" } else { "disabled" }),
                        ],
                    )
                }
                Action::Remove { name, scope } => {
                    let scope = placement(&scope);
                    let file = config_file(cx, scope);
                    if !contains(&file, &name)? {
                        bail!(fill(
                            &TEXT.missing,
                            &[("name", &name), ("file", &file.display().to_string())]
                        ));
                    }
                    mcp::remove(app, &name, scope)?;
                    fill(
                        &TEXT.removed,
                        &[
                            ("name", &name),
                            ("scope", scope.label()),
                            ("file", &file.display().to_string()),
                        ],
                    )
                }
            };
            Ok(output.into())
        })
    }
}

fn find(cx: &Ctx<'_>, name: &str) -> anyhow::Result<mcp::View> {
    cx.app
        .mcp
        .list()
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| anyhow::anyhow!(TEXT.unavailable.clone()))
}

fn pairs(values: Vec<String>) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    values
        .into_iter()
        .map(|v| {
            let (name, value) = v.split_once('=').ok_or_else(|| {
                anyhow::anyhow!("Use NAME=value for environment variables and headers")
            })?;
            if name.is_empty() {
                bail!("Environment/header name cannot be empty");
            }
            Ok((name.to_owned(), value.to_owned()))
        })
        .collect()
}

fn placement(scope: &str) -> Placement {
    if scope == "project" {
        Placement::Project
    } else {
        Placement::User
    }
}

fn config_file(cx: &Ctx<'_>, scope: Placement) -> std::path::PathBuf {
    match scope {
        Placement::User => cx.app.paths.user_config(),
        Placement::Project => cx.app.paths.project_config(),
    }
}

fn contains(file: &std::path::Path, name: &str) -> anyhow::Result<bool> {
    let source = io::fs::read_string(file)?.unwrap_or_default();
    let document: toml::Table = toml::from_str(&source)?;
    Ok(document
        .get("mcp_servers")
        .and_then(toml::Value::as_table)
        .is_some_and(|servers| servers.contains_key(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_auth_uses_the_native_card_without_writing_terminal_output() {
        let project = crate::tools::testing::Project::new();
        mcp::save(
            &project.app,
            "docs",
            &mcp::Config {
                url: Some("https://example.com/mcp".into()),
                ..Default::default()
            },
            Placement::User,
        )
        .unwrap();
        let cx = Ctx {
            app: &project.app,
            scope: Scope::Session,
            session: None,
        };
        let outcome = Mcp.run(&cx, &["auth".into(), "docs".into()]).await.unwrap();
        assert!(outcome.text.is_empty());
        assert_eq!(outcome.effects, [Effect::McpAuth("docs".into())]);
    }
}

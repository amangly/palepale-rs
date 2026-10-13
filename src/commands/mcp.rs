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
}

#[derive(Parser)]
#[command(name = "mcp", disable_help_subcommand = true)]
struct Options {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    List,
    Add {
        name: String,
        #[arg(long, value_parser = ["user", "project"], default_value = "user")]
        scope: String,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        bearer_token_env_var: Option<String>,
        #[arg(long)]
        client_id: Option<String>,
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
                    command,
                    bearer_token_env_var,
                    client_id,
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
                        command: program.next(),
                        args: program.collect(),
                        bearer_token_env_var,
                        client_id,
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

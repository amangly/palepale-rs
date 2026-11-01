use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::extensions::{Kind, text};
use clap::{Parser, Subcommand};
use futures_util::future::BoxFuture;
use std::sync::LazyLock;

pub struct Skills;

static SKILLS: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/skills.toml")));

#[derive(Parser)]
#[command(name = "skills", disable_help_subcommand = true)]
struct Options {
    #[command(subcommand)]
    action: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    List,
    Add {
        source: String,
        #[arg(long, default_value="user", value_parser=["user", "project"])]
        scope: String,
    },
    Remove {
        source: String,
        #[arg(long, default_value="user", value_parser=["user", "project"])]
        scope: String,
    },
    Check {
        name: String,
    },
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
    Refresh,
}

impl Command for Skills {
    fn spec(&self) -> &Spec {
        &SKILLS
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
                    effects: vec![Effect::ShowExtensions(Kind::Skills)],
                });
            }
            let options = match Options::try_parse_from(
                std::iter::once("skills").chain(args.iter().map(String::as_str)),
            ) {
                Ok(options) => options,
                Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
                    return Ok(error.to_string().into());
                }
                Err(error) => anyhow::bail!("{error}"),
            };
            let app = cx.app;
            let placement = |scope: &str| {
                if scope == "project" {
                    crate::extensions::Scope::Project
                } else {
                    crate::extensions::Scope::User
                }
            };
            match options.action.unwrap_or(Action::List) {
                Action::Add { source, scope } => {
                    crate::skills::add_source(app, &source, placement(&scope)).await?
                }
                Action::Remove { source, scope } => {
                    crate::skills::remove_source(app, &source, placement(&scope))?
                }
                Action::Refresh => crate::skills::refresh(app).await?,
                Action::Check { name } => {
                    let found = crate::skills::load(app)
                        .await
                        .into_iter()
                        .find(|s| s.name == name && s.state == crate::skills::State::Ready)
                        .ok_or_else(|| {
                            anyhow::anyhow!("Skill is missing, disabled or untrusted: {name}")
                        })?;
                    return Ok(crate::skills::context(&found)?.into());
                }
                action @ (Action::Enable { .. } | Action::Disable { .. }) => {
                    let (name, enabled) = match action {
                        Action::Enable { name } => (name, true),
                        Action::Disable { name } => (name, false),
                        _ => unreachable!(),
                    };
                    let found = crate::skills::load(app)
                        .await
                        .into_iter()
                        .find(|s| {
                            s.name == name
                                && matches!(
                                    s.state,
                                    crate::skills::State::Ready | crate::skills::State::Disabled
                                )
                        })
                        .ok_or_else(|| anyhow::anyhow!("Skill is unavailable: {name}"))?;
                    if (found.state == crate::skills::State::Ready) != enabled {
                        crate::skills::toggle(app, &found)?;
                    }
                }
                Action::List => {}
            }
            let lines: Vec<_> = crate::skills::load(cx.app)
                .await
                .into_iter()
                .map(|s| {
                    format!(
                        "{}  {}  {}  {}",
                        s.name,
                        s.state.label(),
                        s.scope.label(),
                        s.description
                    )
                })
                .collect();
            Ok(if lines.is_empty() {
                text().empty_skills.clone()
            } else {
                lines.join("\n")
            }
            .into())
        })
    }
}

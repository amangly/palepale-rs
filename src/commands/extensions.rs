use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::extensions::{Kind, text};
use futures_util::future::BoxFuture;
use std::sync::LazyLock;

pub struct Mcp;
pub struct Skills;

static MCP: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/mcp.toml")));
static SKILLS: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/skills.toml")));

fn run<'a>(
    kind: Kind,
    spec: &'a Spec,
    cx: &'a Ctx<'a>,
    args: &'a [String],
) -> BoxFuture<'a, anyhow::Result<Outcome>> {
    Box::pin(async move {
        if !args.is_empty() && args != ["list"] {
            return Err(cx.usage(spec));
        }
        if cx.scope == Scope::Session {
            return Ok(Outcome {
                text: String::new(),
                effects: vec![Effect::ShowExtensions(kind)],
            });
        }
        let lines: Vec<String> = match kind {
            Kind::Mcp => {
                cx.app.mcp.reload(cx.app)?;
                cx.app
                    .mcp
                    .list()
                    .into_iter()
                    .map(|s| {
                        format!(
                            "{}  {}  {} tools  {}",
                            s.name,
                            s.state.label(),
                            s.tools.len(),
                            s.scope.label()
                        )
                    })
                    .collect()
            }
            Kind::Skills => crate::skills::discover(cx.app)
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
                .collect(),
        };
        Ok(if lines.is_empty() {
            match kind {
                Kind::Mcp => text().empty_mcp.clone(),
                Kind::Skills => text().empty_skills.clone(),
            }
        } else {
            lines.join("\n")
        }
        .into())
    })
}

impl Command for Mcp {
    fn spec(&self) -> &Spec {
        &MCP
    }
    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        run(Kind::Mcp, &MCP, cx, args)
    }
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
        run(Kind::Skills, &SKILLS, cx, args)
    }
}

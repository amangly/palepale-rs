use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::extensions::{Kind, text};
use futures_util::future::BoxFuture;
use std::sync::LazyLock;

pub struct Skills;

static SKILLS: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/skills.toml")));

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
            if !args.is_empty() && args != ["list"] {
                return Err(cx.usage(&SKILLS));
            }
            if cx.scope == Scope::Session {
                return Ok(Outcome {
                    text: String::new(),
                    effects: vec![Effect::ShowExtensions(Kind::Skills)],
                });
            }
            let lines: Vec<_> = crate::skills::discover(cx.app)
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

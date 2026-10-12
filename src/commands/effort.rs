use std::sync::LazyLock;

use futures_util::future::BoxFuture;

use super::{Command, Ctx, Effect, Outcome, Spec};
use crate::effort;

const SRC: &str = include_str!("../../data/commands/effort.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));

pub struct Effort;

impl Command for Effort {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            match args {
                [] => Ok(Outcome {
                    effects: vec![Effect::PickEffort],
                    ..Outcome::default()
                }),
                [level] => {
                    let level = if level == "default" {
                        None
                    } else {
                        effort::validate(level)?;
                        Some(level.clone())
                    };
                    Ok(Outcome {
                        text: String::new(),
                        effects: vec![Effect::SetEffort(level)],
                    })
                }
                _ => Err(cx.usage(&SPEC)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_bare_command_opens_the_picker_and_default_clears_the_override() {
        let dir = tempfile::tempdir().unwrap();
        let app = crate::app::App::for_tests(dir.path());
        let cx = Ctx {
            app: &app,
            scope: super::super::Scope::Session,
            session: None,
        };
        assert_eq!(
            Effort.run(&cx, &[]).await.unwrap().effects,
            [Effect::PickEffort]
        );
        assert_eq!(
            Effort.run(&cx, &["default".into()]).await.unwrap().effects,
            [Effect::SetEffort(None)]
        );
    }
}

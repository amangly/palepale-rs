use super::{Command, Ctx, Effect, Outcome, Spec};
use futures_util::future::BoxFuture;
use std::sync::LazyLock;

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/status.toml")));
pub struct Status;

impl Command for Status {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            cx.no_args(&SPEC, args)?;
            cx.session.as_ref().ok_or_else(|| cx.usage(&SPEC))?;
            Ok(Outcome {
                effects: vec![Effect::ShowStatus],
                ..Outcome::default()
            })
        })
    }
}

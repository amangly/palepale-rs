use super::{Command, Ctx, Effect, Outcome, Spec};
use futures_util::future::BoxFuture;

pub struct Workflow {
    source: &'static str,
    spec: std::sync::OnceLock<Spec>,
}
impl Workflow {
    const fn new(source: &'static str) -> Self {
        Self {
            source,
            spec: std::sync::OnceLock::new(),
        }
    }
}
pub static UNDO: Workflow = Workflow::new(include_str!("../../data/commands/undo.toml"));
pub static RETRY: Workflow = Workflow::new(include_str!("../../data/commands/retry.toml"));
pub static COMPACT: Workflow = Workflow::new(include_str!("../../data/commands/compact.toml"));
impl Command for Workflow {
    fn spec(&self) -> &Spec {
        self.spec.get_or_init(|| Spec::parse(self.source))
    }
    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let effect = match self.spec().name.as_str() {
                "compact" => Effect::Compact(args.join(" ")),
                name => {
                    cx.no_args(self.spec(), args)?;
                    Effect::Undo {
                        retry: name == "retry",
                    }
                }
            };
            Ok(Outcome {
                text: String::new(),
                effects: vec![effect],
            })
        })
    }
}

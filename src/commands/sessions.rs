use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use futures_util::future::BoxFuture;
use std::sync::LazyLock;

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/sessions.toml")));
pub struct Sessions;

impl Command for Sessions {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            if cx.scope == Scope::Session {
                return match args {
                    [] => Ok(Outcome {
                        text: String::new(),
                        effects: vec![Effect::PickSession],
                    }),
                    [id] => Ok(Outcome {
                        text: String::new(),
                        effects: vec![Effect::ResumeSession(id.clone())],
                    }),
                    _ => Err(cx.usage(&SPEC)),
                };
            }
            cx.no_args(&SPEC, args)?;
            let sessions = crate::session::store::list(&cx.app.paths)?;
            if sessions.is_empty() {
                return Ok(crate::session::text::get().empty.clone().into());
            }
            Ok(sessions
                .iter()
                .map(|item| {
                    crate::text::controls::for_terminal(&format!(
                        "{}  {}  {}  {}",
                        item.id, item.activity, item.model, item.title
                    ))
                    .into_owned()
                })
                .collect::<Vec<_>>()
                .join("\n")
                .into())
        })
    }
}

use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Outcome, Session, Spec};
use crate::text::template::fill;

const SRC: &str = include_str!("../../data/commands/status.toml");

static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SRC)
        .expect("data/commands/status.toml is checked by tests")
        .text
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    body: String,
    effort: String,
    context: String,
    no_context: String,
}

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
            let session = cx.session.as_ref().ok_or_else(|| cx.usage(&SPEC))?;
            let project = cx.app.paths.project.display().to_string();
            let mut text = render(session, cx.app.api.is_configured(), &project);
            if cx.app.api.is_configured() {
                let account = if cx.app.api.has_key() {
                    match crate::api::account::me(&cx.app.api).await {
                        Ok(me) => crate::commands::login::account_status(&me),
                        Err(error) => error.to_string(),
                    }
                } else {
                    crate::commands::login::status(cx)?
                };
                text.push_str(&format!("\n- {account}"));
            }
            Ok(text.into())
        })
    }
}

fn render(session: &Session, configured: bool, project: &str) -> String {
    let backend = crate::backend::text();
    let connection = if configured {
        &backend.connected
    } else {
        &backend.disconnected
    };
    let model = match &session.effort {
        Some(effort) => fill(
            &TEXT.effort,
            &[("model", &session.model), ("effort", effort)],
        ),
        None => session.model.clone(),
    };
    let context = match session.context_pct {
        Some(pct) => fill(&TEXT.context, &[("pct", &pct.to_string())]),
        None => TEXT.no_context.clone(),
    };
    fill(
        &TEXT.body,
        &[
            ("backend", connection),
            ("model", &model),
            ("mode", &session.mode),
            ("context", &context),
            ("file", &session.file),
            ("project", project),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session {
            model: "demo-2".into(),
            effort: Some("high".into()),
            mode: "MANUAL".into(),
            context_pct: Some(12),
            file: "/home/u/.palepale/sessions/s.jsonl".into(),
        }
    }

    #[test]
    fn it_reports_local_session_state_without_accounts_or_billing() {
        let text = render(&session(), true, "/p");
        for want in [
            "Backend: backend configured",
            "Model: demo-2, effort high",
            "Mode: MANUAL",
            "Context: 12% of the window",
            "`/home/u/.palepale/sessions/s.jsonl`",
        ] {
            assert!(text.contains(want), "{want} missing from:\n{text}");
        }
        let quiet = Session {
            effort: None,
            context_pct: None,
            ..session()
        };
        let text = render(&quiet, false, "/p");
        assert!(text.contains("Backend: backend not configured\n"), "{text}");
        assert!(!text.contains("Account:") && !text.contains("Balance") && !text.contains('$'));
        assert!(
            text.contains("Model: demo-2\n") && text.contains("nothing sent yet"),
            "{text}"
        );
    }
}

//! Delegated jobs use the same agent loop and permission gate as their parent.
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse};
use crate::agent::{
    event::{AgentEvent, Outcome},
    message::{Message, Part},
    run::{self, RunCtx},
};
use crate::session::workflow::text;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::LazyLock;

#[derive(Deserialize)]
struct Kind {
    id: String,
    summary: String,
    tools: Vec<String>,
    #[serde(default)]
    except: Vec<String>,
    prompt: String,
}
static KINDS: LazyLock<Vec<Kind>> = LazyLock::new(|| {
    [
        include_str!("../../data/agents/explore.toml"),
        include_str!("../../data/agents/general.toml"),
    ]
    .iter()
    .map(|s| toml::from_str(s).expect("agent kind"))
    .collect()
});
static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    let mut spec = Spec::load(
        "task",
        include_str!("../../data/tools/task.md"),
        include_str!("../../data/tools/task.schema.json"),
    );
    spec.description = std::borrow::Cow::Owned(crate::text::template::fill(
        &spec.description,
        &[(
            "agents",
            &KINDS
                .iter()
                .map(|k| format!("- {}: {}", k.id, k.summary))
                .collect::<Vec<_>>()
                .join("\n"),
        )],
    ));
    spec.parameters["properties"]["subagent_type"]["enum"] =
        json!(KINDS.iter().map(|k| &k.id).collect::<Vec<_>>());
    spec
});
#[derive(Deserialize)]
struct Args {
    description: String,
    prompt: String,
    subagent_type: String,
}
pub struct Task;
impl Tool for Task {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn joins_on_cancel(&self) -> bool {
        true
    }
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        if !KINDS.iter().any(|k| k.id == args.subagent_type)
            || args.prompt.trim().is_empty()
            || args.description.trim().is_empty()
        {
            return Err(Refusal::InvalidArgs(text().task_invalid.clone()));
        }
        if cx.shared.depth >= crate::limits::get().subagent.max_depth.value {
            return Err(Refusal::Refused(text().task_depth.clone()));
        }
        Ok(Check::new(vec![crate::permission::gate::Ask::new(
            crate::permission::catalog::TASK.name,
            args.subagent_type,
        )]))
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            match child(args, cx).await {
                Ok(output) => output,
                Err(error) => Output::error(error.to_string()),
            }
        })
    }
}
async fn child(args: Value, cx: &ToolCx<'_>) -> anyhow::Result<Output> {
    let args: Args = serde_json::from_value(args)?;
    let parent = cx
        .run
        .ok_or_else(|| anyhow::anyhow!(text().task_no_run.clone()))?;
    let kind = KINDS
        .iter()
        .find(|k| k.id == args.subagent_type)
        .ok_or_else(|| anyhow::anyhow!(text().task_unknown.clone()))?;
    let _slot = tokio::select! {
        permit = cx.shared.slots.acquire() => permit?,
        () = cx.cancel.cancelled() => return Ok(Output::error(text().task_cancelled.clone())),
    };
    let shared = cx.shared.child(cx.cancel.clone());
    shared.set_cwd(cx.cwd.clone());
    let parent_file = cx.shared.session.lock().unwrap().clone();
    let store =
        crate::session::store::Store::child(&cx.app.paths, &parent_file, cx.call_id, &kind.id)?;
    let mut transcript = crate::session::transcript::Transcript::new(Some(store));
    let mut except = kind.except.clone();
    if shared.depth >= crate::limits::get().subagent.max_depth.value {
        except.push("task".into());
    }
    let tools = parent.tools.subset(&kind.tools, &except);
    let system = format!("{}\n\n{}", parent.system, kind.prompt);
    let emit = |event| {
        (cx.emit)(AgentEvent::Child {
            call_id: cx.call_id.to_owned(),
            event: Box::new(event),
        })
    };
    let child = RunCtx {
        app: parent.app,
        model: parent.model,
        effort: parent.effort.clone(),
        system: &system,
        tools: &tools,
        plan_file: parent.plan_file.clone(),
        emit: &emit,
    };
    let outcome = Box::pin(run::run(
        &child,
        &mut transcript,
        &shared,
        vec![Part::Text { text: args.prompt }],
    ))
    .await;
    let answer = transcript.messages.iter().rev().find_map(|m| {
        if let Message::Assistant(a) = m {
            Some(a)
        } else {
            None
        }
    });
    let content = answer
        .map(|a| {
            a.parts
                .iter()
                .filter_map(|p| {
                    if let Part::Text { text } = p {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            crate::text::template::fill(&text().task_ended, &[("outcome", &format!("{outcome:?}"))])
        });
    let mut output = if outcome == Outcome::Done {
        Output::ok(content)
    } else {
        Output::error(content)
    };
    let changed_files = transcript.messages.iter().any(
        |message| matches!(message, Message::Tool(result) if crate::agent::verify::changed(result)),
    );
    output.details = Some(
        json!({"agent": kind.id, "description": args.description, "session": transcript.file(), "outcome": outcome, "usage": transcript.usage(), "changed_files": changed_files}),
    );
    Ok(output)
}

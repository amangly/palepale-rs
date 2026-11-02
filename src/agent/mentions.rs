//! Explicit references use the same checked, recorded tool pipeline as model calls.
use std::collections::BTreeSet;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::agent::{
    batch,
    event::{AgentEvent, Outcome},
    message::{Assistant, Finish, Message, Part, ToolCall},
    run::{RunCtx, record},
    state::Shared,
};
use crate::session::transcript::Transcript;
use crate::text::mentions;

pub async fn expand(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    input: &[Part],
    cancel: &CancellationToken,
) -> Result<bool, Outcome> {
    // Child prompts contain already-resolved user references and source text.
    if shared.depth != 0 {
        return Ok(false);
    }
    let prompt = input
        .iter()
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let references = mentions::parse(&prompt);
    let agents: Vec<_> = references
        .iter()
        .filter(|m| m.closed && crate::tools::task::catalog().any(|(id, _)| id == m.path))
        .collect();
    if agents.len() > 1 {
        return fail(cx, transcript, &mentions::text().multiple_agents);
    }
    let mut seen = BTreeSet::new();
    let paths: Vec<_> = references
        .iter()
        .filter(|m| m.closed && !m.path.is_empty() && !agents.contains(m))
        .filter(|m| {
            let canonical = crate::tools::target::resolve(&cx.app.paths, &m.path)
                .map(|p| p.path)
                .unwrap_or_else(|_| m.path.clone().into());
            seen.insert(canonical)
        })
        .collect();
    if paths.len() > crate::limits::get().mentions.files.value {
        return fail(cx, transcript, &mentions::text().too_many);
    }
    let mut context = String::new();
    for path in paths {
        // Reserve one bounded read plus its path and truncation notice before executing it.
        let limits = crate::limits::get();
        let reserve =
            limits.tools.max_bytes.value + limits.tools.max_line_chars.value + path.path.len();
        if context.len().saturating_add(reserve) > limits.mentions.context_bytes.value {
            return fail(cx, transcript, &mentions::text().too_many);
        }
        let call = call(transcript, "read", json!({"path": path.path}));
        let results = execute(cx, transcript, shared, call, cancel).await?;
        context.push_str(&format!("\n{}:\n{}\n", path.path, results.content));
        if context.len() > crate::limits::get().mentions.context_bytes.value {
            return fail(cx, transcript, &mentions::text().too_many);
        }
    }
    if let Some(agent) = agents.first() {
        let mut task = prompt.clone();
        task.replace_range(agent.span.clone(), "");
        task = mentions::unescape(&task);
        task.push_str(&context);
        task.push_str(&crate::agent::instructions::snapshot(
            &cx.app.paths,
            &transcript.messages,
        ));
        let call = call(
            transcript,
            "task",
            json!({"subagent_type": agent.path, "description": mentions::text().task_description, "prompt": task}),
        );
        let result = execute(cx, transcript, shared, call, cancel).await?;
        return Ok(crate::agent::verify::changed(&result));
    }
    Ok(false)
}

fn call(transcript: &Transcript, name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: format!("mention_{}_{}", transcript.messages.len(), name),
        name: name.into(),
        arguments: args.to_string(),
    }
}

async fn execute(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    call: ToolCall,
    cancel: &CancellationToken,
) -> Result<crate::agent::message::ToolResult, Outcome> {
    if cancel.is_cancelled() {
        return Err(Outcome::Aborted);
    }
    let message = Assistant {
        parts: vec![Part::ToolCall(call.clone())],
        model: cx.model.id.clone(),
        effort: None,
        usage: None,
        finish: Finish::ToolCalls,
        error: None,
        request_id: None,
    };
    record(cx, transcript, Message::Assistant(message.clone()));
    (cx.emit)(AgentEvent::MessageEnd { message });
    let batch = batch::run(
        cx,
        &transcript.messages[..transcript.messages.len() - 1],
        &[call],
        shared,
        cancel,
    )
    .await;
    let result = batch.results.into_iter().next().expect("one mention call");
    record(cx, transcript, Message::Tool(result.clone()));
    if cancel.is_cancelled() {
        return Err(Outcome::Aborted);
    }
    if batch.stop {
        return Err(Outcome::Rejected);
    }
    if (result.is_error && result.name != "task") || result.refused.is_some() {
        return Err(Outcome::Failed);
    }
    Ok(result)
}

fn fail(cx: &RunCtx<'_>, transcript: &mut Transcript, text: &str) -> Result<bool, Outcome> {
    let message = Assistant {
        parts: vec![Part::Text { text: text.into() }],
        model: cx.model.id.clone(),
        effort: None,
        usage: None,
        finish: Finish::Done,
        error: None,
        request_id: None,
    };
    (cx.emit)(AgentEvent::Delta {
        kind: crate::agent::event::DeltaKind::Text,
        text: text.into(),
        index: None,
        name: None,
    });
    record(cx, transcript, Message::Assistant(message.clone()));
    (cx.emit)(AgentEvent::MessageEnd { message });
    Err(Outcome::Failed)
}

//! Summaries are additive session records; the original transcript remains intact.
use crate::agent::{
    event::{AgentEvent, Outcome},
    message::{Finish, Message, Part},
    run::RunCtx,
    state::Shared,
};
use crate::api::{
    chat::{self, Turn},
    preflight,
};
use crate::session::{transcript::Transcript, workflow};
use tokio_util::sync::CancellationToken;

pub fn can_compact(transcript: &Transcript) -> bool {
    cut(transcript).is_some()
}

fn cut(transcript: &Transcript) -> Option<usize> {
    let from = transcript
        .compactions
        .last()
        .and_then(|c| transcript.ids.iter().position(|id| id == &c.kept))
        .unwrap_or(0);
    let last = workflow::last_prompt(transcript)?;
    if last <= from {
        return None;
    }
    let budget = crate::limits::get().compaction.keep_tokens.value
        * preflight::config().bytes_per_token as usize;
    let mut bytes = 0;
    let mut candidate = last;
    for i in (from..transcript.messages.len()).rev() {
        bytes += serde_json::to_vec(&transcript.messages[i]).map_or(0, |v| v.len());
        if matches!(&transcript.messages[i], Message::User { parts } if parts.iter().any(|p| matches!(p, Part::Text { .. } | Part::Attachment(_))))
            && i > from
        {
            candidate = i;
            if bytes >= budget {
                break;
            }
        }
    }
    Some(candidate)
}

pub fn needed(turn: &Turn<'_>) -> bool {
    preflight::estimated_input(turn)
        .saturating_add(turn.model.max_output_tokens)
        .saturating_mul(100)
        >= turn
            .model
            .context_window
            .saturating_mul(crate::limits::get().compaction.trigger_percent.value)
}

pub async fn run(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    focus: &str,
) -> Outcome {
    let cancel = shared.begin_run();
    let outcome = match compact(cx, transcript, shared, &cancel, focus, false).await {
        Ok(_) => Outcome::Done,
        Err(_) if cancel.is_cancelled() => Outcome::Aborted,
        Err(_) => Outcome::Failed,
    };
    (cx.emit)(AgentEvent::RunEnd { outcome });
    outcome
}

pub async fn compact(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    cancel: &CancellationToken,
    focus: &str,
    auto: bool,
) -> Result<bool, String> {
    (cx.emit)(AgentEvent::CompactionStart { auto });
    let result = summarize(cx, transcript, cancel, focus).await;
    let (changed, usage, error) = match result {
        Ok(Some((cut, summary, usage))) => match transcript.compact(cut, summary, usage) {
            Ok(()) => {
                shared.resumed();
                (true, usage, None)
            }
            Err(error) => (false, usage, Some(error.error)),
        },
        Ok(None) => (false, None, None),
        Err(error) => (false, None, Some(error)),
    };
    (cx.emit)(AgentEvent::CompactionEnd {
        changed,
        usage,
        error: error.clone(),
    });
    error.map_or(Ok(changed), Err)
}

async fn summarize(
    cx: &RunCtx<'_>,
    transcript: &Transcript,
    cancel: &CancellationToken,
    focus: &str,
) -> Result<Option<(usize, String, Option<crate::agent::message::Usage>)>, String> {
    let Some(cut) = cut(transcript) else {
        return Ok(None);
    };
    let mut model = cx.model.clone();
    model.max_output_tokens = model
        .max_output_tokens
        .min(crate::limits::get().compaction.summary_tokens.value);
    let from = transcript
        .compactions
        .last()
        .and_then(|c| transcript.ids.iter().position(|id| id == &c.kept))
        .unwrap_or(0);
    let mut messages = Vec::new();
    if let Some(summary) = transcript.compactions.last() {
        messages.push(Message::User {
            parts: vec![Part::Text {
                text: summary.summary.clone(),
            }],
        });
    }
    messages.extend_from_slice(&transcript.messages[from..cut]);
    let mut messages = crate::agent::context::build(&messages);
    let instruction = crate::text::template::fill(
        include_str!("../../data/prompts/compact.md"),
        &[("focus", focus)],
    );
    messages.push(Message::User {
        parts: vec![Part::Text { text: instruction }],
    });
    let profile = transcript.state.agent_profile.as_deref();
    let turn = Turn {
        model: &model,
        system: cx.system,
        messages: &messages,
        tools: &[],
        agent_profile: profile,
        effort: cx.effort.as_deref(),
    };
    preflight::check(&turn).map_err(|error| error.to_string())?;
    let reply = chat::stream(&cx.app.api, &turn, cancel, &mut |_| {}).await;
    if let Some(error) = reply.error {
        return Err(error.to_string());
    }
    if reply.finish != Finish::Done || reply.tool_calls().next().is_some() {
        return Err(workflow::text().summary_empty.clone());
    }
    let summary = reply
        .parts
        .iter()
        .filter_map(|p| {
            if let Part::Text { text } = p {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if summary.trim().is_empty() {
        return Err(workflow::text().summary_empty.clone());
    }
    Ok(Some((cut, summary, reply.usage)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn summaries_keep_the_latest_prompt_and_never_summarize_the_same_prefix_twice() {
        let mut transcript = Transcript::new(None);
        assert!(transcript.push(Message::user_text("first")).is_ok());
        assert!(cut(&transcript).is_none());
        assert!(transcript.push(Message::user_text("second")).is_ok());
        assert_eq!(cut(&transcript), Some(1));
        assert!(
            transcript
                .compact(1, "first summarized".into(), None)
                .is_ok()
        );
        assert!(cut(&transcript).is_none());
        assert!(transcript.push(Message::user_text("third")).is_ok());
        assert_eq!(cut(&transcript), Some(2));
        assert!(transcript.rewind(1).is_ok());
        assert!(transcript.compactions.is_empty());
        assert!(cut(&transcript).is_none());
    }
}

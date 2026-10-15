//! One reply's tool calls: cleared one by one in the order the model made them (prompts stay
//! in order), then run together, with results kept in call order.

use std::sync::Arc;

use futures_util::future::join_all;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::agent::doom_loop;
use crate::agent::event::AgentEvent;
use crate::agent::message::{Message, ToolCall, ToolResult};
use crate::agent::model_text;
use crate::agent::prompt::{self, Prompt, Reply};
use crate::agent::run::RunCtx;
use crate::agent::state::Shared;
use crate::limits;
use crate::mode::DoomLoop;
use crate::permission::gate::{self, Ask, Decision, Policy};
use crate::text::template::fill;
use crate::tools::{Output, Refusal, Tool, ToolCx, permissions};

pub struct Batch {
    pub results: Vec<ToolResult>,
    /// The user declined without a word: stop the run.
    pub stop: bool,
}

/// A call that may run, and what it was cleared for.
struct Cleared {
    tool: Arc<dyn Tool>,
    args: Value,
    asks: Vec<Ask>,
}

/// Why a call does not run: what the model is told, and whether the run stops there.
struct Refused {
    why: String,
    /// Recorded with the result; see `ToolResult::refused`.
    kind: &'static str,
    /// What it needed permission for, when it got as far as asking.
    asks: Vec<Ask>,
    stop: bool,
}

impl Refused {
    fn new(kind: &'static str, why: String) -> Self {
        Self {
            why,
            kind,
            asks: Vec::new(),
            stop: false,
        }
    }

    fn asked(self, asks: Vec<Ask>) -> Self {
        Self { asks, ..self }
    }
}

type Slot = Result<Cleared, Refused>;

pub async fn run<'a>(
    cx: &'a RunCtx<'a>,
    history: &[Message],
    calls: &'a [ToolCall],
    shared: &'a Shared,
    cancel: &'a CancellationToken,
) -> Batch {
    let mut cleared = Vec::with_capacity(calls.len());
    let mut stop = false;
    for (i, call) in calls.iter().enumerate() {
        let slot = if stop {
            Err(Refused::new("rejected", model_text::get().rejected.clone()))
        } else {
            let repeats = doom_loop::repeats(history, calls, i);
            clear(
                cx,
                shared,
                cancel,
                repeats,
                uncertain_repeat(history, call),
                call,
            )
            .await
        };
        if let Err(refused) = &slot {
            stop |= refused.stop;
        }
        cleared.push(slot);
    }
    Batch {
        results: settle(cx, calls, cleared, shared, cancel).await,
        stop,
    }
}

/// Refused calls still emit their lifecycle events and record why they did not run.
pub async fn refuse_all(
    cx: &RunCtx<'_>,
    calls: &[ToolCall],
    shared: &Shared,
    cancel: &CancellationToken,
    kind: &'static str,
    why: &str,
) -> Batch {
    let slots = calls
        .iter()
        .map(|_| Err(Refused::new(kind, why.to_owned())))
        .collect();
    Batch {
        results: settle(cx, calls, slots, shared, cancel).await,
        stop: false,
    }
}

async fn settle(
    cx: &RunCtx<'_>,
    calls: &[ToolCall],
    slots: Vec<Slot>,
    shared: &Shared,
    cancel: &CancellationToken,
) -> Vec<ToolResult> {
    // Polled in order, so tools start, and results come back, in the order they were called.
    // A refused call gets its start and end too, so a frontend shows every call's fate.
    join_all(slots.into_iter().zip(calls).map(|(slot, call)| async move {
        (cx.emit)(AgentEvent::ToolStart {
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });
        let done = match slot {
            Err(refused) => ToolResult {
                asks: refused.asks,
                refused: Some(refused.kind.to_owned()),
                ..result(call, Output::error(refused.why))
            },
            Ok(cleared) => {
                let tcx = tool_cx(cx, shared, cancel, &call.id);
                let out = tokio::select! {
                    out = cleared.tool.run(cleared.args, &tcx) => out,
                    () = cancel.cancelled() => Output::error(model_text::get().cancelled.clone()),
                };
                ToolResult {
                    asks: cleared.asks,
                    ..result(call, out)
                }
            }
        };
        (cx.emit)(AgentEvent::ToolEnd {
            result: done.clone(),
        });
        done
    }))
    .await
}

/// Everything a call must pass before it runs, in order: not cancelled, a known tool, valid
/// JSON, not a loop, the tool's own check, then the permission policy (asking if it says so).
/// Read the mode per call so a change at one prompt governs the remaining calls.
async fn clear<'a>(
    cx: &'a RunCtx<'a>,
    shared: &'a Shared,
    cancel: &'a CancellationToken,
    repeats: usize,
    uncertain: bool,
    call: &'a ToolCall,
) -> Result<Cleared, Refused> {
    let mode = shared.mode();
    let text = model_text::get();
    if cancel.is_cancelled() {
        return Err(Refused::new("cancelled", text.cancelled.clone()));
    }
    let tool = cx.tools.get(&call.name).ok_or_else(|| {
        Refused::new(
            "unknown_tool",
            fill(
                &text.unknown_tool,
                &[("name", &call.name), ("available", &cx.tools.names())],
            ),
        )
    })?;
    let args: Value = serde_json::from_str(&call.arguments).map_err(|e| {
        Refused::new(
            "bad_json",
            fill(&text.bad_json, &[("error", &e.to_string())]),
        )
    })?;

    let mut asks = Vec::new();
    if repeats >= limits::get().agent.doom_loop_repeats.value {
        match mode.verdicts.doom_loop {
            DoomLoop::TellModel => {
                return Err(Refused::new(
                    "doom_loop",
                    fill(&text.doom_loop, &[("count", &repeats.to_string())]),
                ));
            }
            DoomLoop::Ask => asks.push(Ask::new(permissions::DOOM_LOOP.name, call.name.clone())),
        }
    }
    let mut check = tool
        .check(&args, &tool_cx(cx, shared, cancel, &call.id))
        .map_err(|refusal| match refusal {
            Refusal::InvalidArgs(error) => Refused::new(
                "invalid_arguments",
                fill(
                    &text.invalid_arguments,
                    &[("name", &call.name), ("error", &error)],
                ),
            ),
            Refusal::Refused(why) => Refused::new("check", why),
        })?;
    if uncertain {
        check.confirm = true;
        check.preview = Some(crate::session::text::get().interrupted.clone());
    }
    asks.extend(check.asks);

    let policy = Policy {
        rules: &cx.app.settings.permission,
        mode,
        plan_file: &cx.plan_file,
    };
    let cleared = |asks| Cleared {
        tool: tool.clone(),
        args,
        asks,
    };
    let decision = match policy.decide(&asks) {
        Decision::Allow if check.confirm => Decision::Ask,
        decision => decision,
    };
    match decision {
        Decision::Allow => Ok(cleared(asks)),
        Decision::Deny(what) => {
            Err(Refused::new("denied", fill(&text.denied, &[("what", &what)])).asked(asks))
        }
        Decision::Ask => {
            let prompt = Prompt::Permission {
                tool: call.name.clone(),
                asks: asks.clone(),
                preview: check.preview,
                confirm: check.confirm,
            };
            let reply = prompt::ask(shared, cx.emit, &call.id, prompt, cancel).await;
            let refused = match reply {
                Some(Reply::Yes) => return Ok(cleared(asks)),
                Some(Reply::No { feedback: Some(f) } | Reply::Text(f)) => Refused::new(
                    "rejected_with_feedback",
                    fill(&text.rejected_with_feedback, &[("feedback", &f)]),
                ),
                Some(Reply::No { feedback: None } | Reply::Choice(_) | Reply::Answers(_)) => {
                    Refused {
                        stop: true,
                        ..Refused::new("rejected", text.rejected.clone())
                    }
                }
                Some(Reply::Unavailable) => {
                    let (kind, why) = if check.confirm {
                        ("unconfirmed", &text.unconfirmed)
                    } else {
                        ("non_interactive", &text.non_interactive)
                    };
                    Refused::new(kind, fill(why, &[("what", &gate::describe(&asks))]))
                }
                None => Refused::new("cancelled", text.cancelled.clone()),
            };
            Err(refused.asked(asks))
        }
    }
}

/// A resumed action with no recorded outcome needs an explicit answer even in AUTO.
fn uncertain_repeat(history: &[Message], proposed: &ToolCall) -> bool {
    history.iter().any(|message| {
        let Message::Tool(result) = message else {
            return false;
        };
        if result.refused.as_deref() != Some("interrupted") {
            return false;
        }
        history
            .iter()
            .filter_map(|message| match message {
                Message::Assistant(reply) => Some(reply),
                _ => None,
            })
            .flat_map(|reply| reply.tool_calls())
            .any(|call| {
                call.id == result.call_id
                    && call.name == proposed.name
                    && match (
                        serde_json::from_str::<Value>(&call.arguments),
                        serde_json::from_str::<Value>(&proposed.arguments),
                    ) {
                        (Ok(original), Ok(next)) => original == next,
                        _ => call.arguments == proposed.arguments,
                    }
            })
    })
}

fn tool_cx<'a>(
    cx: &'a RunCtx<'a>,
    shared: &'a Shared,
    cancel: &CancellationToken,
    call_id: &'a str,
) -> ToolCx<'a> {
    ToolCx {
        app: cx.app,
        shared,
        emit: cx.emit,
        call_id,
        plan_file: &cx.plan_file,
        cancel: cancel.clone(),
    }
}

fn result(call: &ToolCall, out: Output) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        name: call.name.clone(),
        content: out.content,
        attachments: out.attachments,
        is_error: out.is_error,
        details: out.details,
        asks: Vec::new(),
        refused: None,
    }
}

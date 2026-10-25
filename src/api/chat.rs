//! One paid model turn: dispatch once, then stream the reply.
//! Never fails: every outcome, including errors and cancellation, is an `Assistant` message.

use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::agent::message::{Assistant, Finish, Message, Usage};
use crate::api::assemble::{Assembler, Delta};
use crate::api::error::{ApiError, ErrorInfo};
use crate::api::models::Model;
use crate::api::sse;
use crate::api::wire::{self, Replay, StreamOptions, WireMessage, WireUsage};
use crate::api::{Api, Call};
use crate::limits;

pub struct Turn<'a> {
    pub model: &'a Model,
    pub agent_profile: Option<&'a str>,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [serde_json::Value],
}

pub enum Event<'a> {
    Delta(Delta<'a>),
}

pub async fn stream(
    api: &Api,
    turn: &Turn<'_>,
    cancel: &CancellationToken,
    on: &mut (dyn FnMut(Event<'_>) + Send),
) -> Assistant {
    if let Err(error) = crate::api::preflight::check(turn) {
        return message(turn, Vec::new(), Finish::Error, None, Some(error), None);
    }
    let body = serde_json::to_value(request(turn)).expect("chat requests serialize");
    // Missing output does not prove that the provider rejected the paid request.
    attempt_once(api, turn, &body, cancel, on).await
}

async fn attempt_once(
    api: &Api,
    turn: &Turn<'_>,
    body: &serde_json::Value,
    cancel: &CancellationToken,
    on: &mut (dyn FnMut(Event<'_>) + Send),
) -> Assistant {
    let limits = &limits::get().chat;
    let operation = crate::session::entry::new_id();
    let headers = [("Idempotency-Key", operation.as_str())];
    let call = Call {
        body: Some(body),
        headers: &headers,
        ..Call::default()
    };
    let opened = tokio::select! {
        r = api.open("chat", call, limits.headers_timeout_secs.secs()) => r,
        () = cancel.cancelled() => return message(turn, Vec::new(), Finish::Aborted, None, None, None),
    };
    let resp = match opened {
        Ok(resp) => resp,
        Err(ApiError { info, .. }) => {
            let request_id = info.request_id.clone();
            return message(
                turn,
                Vec::new(),
                Finish::Error,
                None,
                Some(info),
                request_id,
            );
        }
    };

    let events = sse::events(
        resp.body,
        limits.idle_timeout_secs.secs(),
        limits.stall_timeout_secs.secs(),
        limits.max_event_bytes.value,
    );
    let mut events = std::pin::pin!(events);
    let mut asm = Assembler::default();
    let mut cut = None;
    let mut aborted = false;
    loop {
        let next = tokio::select! {
            next = events.next() => next,
            () = cancel.cancelled() => { aborted = true; break; }
        };
        match next {
            None => break,
            Some(Err(error)) => {
                cut = Some(error);
                break;
            }
            Some(Ok(data)) => match serde_json::from_str(&data) {
                Ok(chunk) => asm.apply(chunk, &mut |d| on(Event::Delta(d))),
                Err(e) => {
                    cut = Some(ErrorInfo::local("bad_stream", e.to_string()));
                    break;
                }
            },
        }
    }

    let request_id = resp.head.request_id.clone();
    let done = asm.finish(cut);
    let (finish, error) = if aborted {
        (Finish::Aborted, None)
    } else if !turn.model.capabilities.tool_calling
        && done
            .parts
            .iter()
            .any(|part| matches!(part, crate::agent::message::Part::ToolCall(_)))
    {
        (
            Finish::Error,
            Some(with_request_id(
                ErrorInfo::local(
                    "unsupported_tool_call",
                    crate::agent::model_text::get()
                        .unsupported_tool_call
                        .clone(),
                ),
                &request_id,
            )),
        )
    } else {
        (
            done.finish,
            done.error.map(|e| with_request_id(e, &request_id)),
        )
    };
    let usage = done.usage.map(|u| cost(turn.model, &u));
    message(turn, done.parts, finish, usage, error, request_id)
}

fn request(turn: &Turn<'_>) -> wire::Request {
    let replay = Replay {
        model: &turn.model.id,
        reasoning: turn.model.capabilities.reasoning,
        tools: turn.model.capabilities.tool_calling,
    };
    let mut messages = vec![WireMessage::System {
        content: turn.system.to_owned(),
    }];
    messages.extend(wire::messages(turn.messages, &replay));
    wire::Request {
        model: turn.model.id.clone(),
        messages,
        tools: if replay.tools {
            turn.tools.to_vec()
        } else {
            Vec::new()
        },
        stream: true,
        stream_options: StreamOptions {
            include_usage: true,
        },
        agent_profile: turn.agent_profile.map(str::to_owned),
        max_completion_tokens: crate::api::preflight::output_limit(turn.model),
    }
}

/// Tokens times USD-per-million is exactly micro-dollars.
fn cost(model: &Model, usage: &WireUsage) -> Usage {
    let cached = usage.cached().min(usage.prompt_tokens);
    let p = &model.pricing;
    let micros = (usage.prompt_tokens - cached) as f64 * p.input_per_1m_usd
        + cached as f64 * p.cached_input_per_1m_usd
        + usage.completion_tokens as f64 * p.output_per_1m_usd;
    Usage {
        input: usage.prompt_tokens,
        cached,
        output: usage.completion_tokens,
        reasoning: usage.reasoning(),
        cost_micros: micros.round() as u64,
    }
}

fn with_request_id(mut error: ErrorInfo, request_id: &Option<String>) -> ErrorInfo {
    if error.request_id.is_none() {
        error.request_id.clone_from(request_id);
    }
    error
}

fn message(
    turn: &Turn<'_>,
    parts: Vec<crate::agent::message::Part>,
    finish: Finish,
    usage: Option<Usage>,
    error: Option<ErrorInfo>,
    request_id: Option<String>,
) -> Assistant {
    Assistant {
        parts,
        model: turn.model.id.clone(),
        effort: None,
        usage,
        finish,
        error,
        request_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Capabilities, Pricing};

    fn model(reasoning: bool) -> Model {
        Model {
            id: "m".into(),
            display_name: String::new(),
            description: String::new(),
            context_window: 1000,
            max_output_tokens: 100,
            default_max_output_tokens: None,
            available: true,
            pricing: Pricing {
                input_per_1m_usd: 6.0,
                cached_input_per_1m_usd: 0.6,
                output_per_1m_usd: 10.0,
            },
            capabilities: Capabilities {
                reasoning,
                ..Capabilities::default()
            },
            input_modalities: None,
            capability_source: None,
            reasoning_efforts: None,
            default_reasoning_effort: None,
        }
    }

    #[test]
    fn cost_splits_cached_and_uncached_input() {
        let usage: WireUsage = serde_json::from_str(
            r#"{"prompt_tokens":1000,"completion_tokens":100,"prompt_tokens_details":{"cached_tokens":400}}"#,
        )
        .unwrap();
        let u = cost(&model(true), &usage);
        // 600 * 6 + 400 * 0.6 + 100 * 10 micro-dollars
        assert_eq!(u.cost_micros, 3600 + 240 + 1000);
        assert_eq!(u.cached, 400);
    }

    #[test]
    fn reasoning_uses_the_server_default() {
        let m = model(false);
        let turn = Turn {
            model: &m,
            agent_profile: None,
            system: "sys",
            messages: &[],
            tools: &[],
        };
        let recorded =
            |turn: &Turn<'_>| message(turn, vec![], Finish::Done, None, None, None).effort;
        assert!(
            serde_json::to_value(request(&turn))
                .unwrap()
                .get("reasoning_effort")
                .is_none()
        );
        assert_eq!(recorded(&turn), None);
        let m = model(true);
        let turn = Turn { model: &m, ..turn };
        assert!(
            serde_json::to_value(request(&turn))
                .unwrap()
                .get("reasoning_effort")
                .is_none()
        );
        assert_eq!(recorded(&turn), None);
    }

    #[test]
    fn request_sends_the_selected_server_profile() {
        let m = model(false);
        let turn = Turn {
            model: &m,
            agent_profile: Some("jb-2"),
            system: "sys",
            messages: &[],
            tools: &[],
        };
        assert_eq!(request(&turn).agent_profile.as_deref(), Some("jb-2"));
        let off = Turn {
            agent_profile: None,
            ..turn
        };
        assert_eq!(request(&off).agent_profile, None);
    }
}

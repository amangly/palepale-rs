//! The agent loop (after pi's agent-loop): ask the model, run what it calls, feed the results
//! back, until it stops calling tools. Messages queued meanwhile are delivered at safe points.

use crate::agent::batch::{self, Batch};
use crate::agent::event::{AgentEvent, Outcome};
use crate::agent::message::{Finish, Message, Part, ToolCall};
use crate::agent::model_text;
use crate::agent::state::{Reminder, Shared};
use crate::agent::system_prompt;
use crate::api::chat::{self, Event, Turn};
use crate::api::models::Model;
use crate::app::App;
use crate::limits;
use crate::session::transcript::Transcript;
use crate::text::template::fill;
use crate::tools::Registry;

pub struct RunCtx<'a> {
    pub app: &'a App,
    pub model: &'a Model,
    pub effort: Option<String>,
    pub system: &'a str,
    pub tools: &'a Registry,
    /// Where PLAN mode writes its plan; the only file it may change.
    pub plan_file: String,
    pub emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
}

pub async fn run(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shared: &Shared,
    prompt: Vec<Part>,
) -> Outcome {
    let cancel = shared.begin_run();
    *shared.session.lock().unwrap() = transcript
        .stored_file()
        .map(ToOwned::to_owned)
        .unwrap_or_default();
    let shadow = if shared.depth == 0 && transcript.stored_file().is_some() {
        Some(crate::io::snapshot::Shadow::open(&cx.app.paths).await)
    } else {
        None
    };
    let before = match &shadow {
        Some(shadow) => shadow.take().await,
        None => None,
    };
    let state = crate::session::store::SavedState {
        model: cx.model.id.clone(),
        mode: shared.mode().id.clone(),
        effort: cx.effort.clone(),
        agent_profile: shared.agent_profile(),
        system_prompt: transcript.state.system_prompt.clone(),
    };
    if let Err(error) = transcript.save_state(state) {
        (cx.emit)(AgentEvent::Unsaved {
            path: error.path.display().to_string(),
            error: error.error,
        });
    }
    let mut parts = with_reminder(cx, shared, prompt);
    if shared.depth == 0 && shared.orient_once() {
        let context = tokio::select! {
            context = crate::agent::orient::snapshot(&cx.app.paths) => context,
            () = cancel.cancelled() => String::new(),
        };
        if !context.is_empty() {
            parts.insert(0, Part::Reminder { text: context });
        }
    }
    if !crate::commands::discovered::expand_prompts(cx, shared, &cancel, &mut parts).await {
        record(cx, transcript, Message::User { parts });
        (cx.emit)(AgentEvent::RunEnd {
            outcome: Outcome::Failed,
        });
        return Outcome::Failed;
    }
    crate::skills::expand(
        cx.app,
        shared,
        cx.emit,
        &cx.plan_file,
        &cancel,
        &transcript.messages,
        &mut parts,
    )
    .await;
    record(cx, transcript, Message::User { parts });
    let prompt_id = transcript.ids.last().cloned().unwrap_or_default();
    if let Some(before) = &before
        && let Err(error) = transcript.snapshot(prompt_id.clone(), before.clone(), None)
    {
        (cx.emit)(AgentEvent::Unsaved {
            path: error.path.display().to_string(),
            error: error.error,
        });
    }
    let max_turns = if shared.depth == 0 {
        limits::get().agent.max_turns.value
    } else {
        limits::get().subagent.max_turns.value
    };
    let mut turns = 0;
    let mut summary_failed = false;
    let mut overflow_retried = false;
    let mut told_written = false;
    let mut changed = false;
    let mut verified = 0;

    let outcome = 'run: loop {
        loop {
            if turns >= max_turns {
                break 'run Outcome::TurnLimit;
            }
            turns += 1;

            tokio::select! {
                () = async {
                    cx.app.mcp.start_enabled(cx.app).await;
                    cx.app.mcp.refresh_changed(cx.app).await;
                } => {},
                () = cancel.cancelled() => break 'run Outcome::Aborted,
            }
            let skills = crate::skills::load(cx.app).await;
            let mut skill_context =
                crate::skills::visible_catalog(&skills, cx.app, shared, &cx.plan_file);
            let previous_skills =
                crate::skills::last_catalog(&transcript.messages, "<available_skills>");
            if skill_context.is_empty() && previous_skills.is_some() {
                skill_context = crate::text::template::fill(
                    &crate::extensions::text().skills_frame,
                    &[("skills", "")],
                );
            }
            if !skill_context.is_empty() && previous_skills != Some(skill_context.as_str()) {
                record(
                    cx,
                    transcript,
                    Message::User {
                        parts: vec![Part::Reminder {
                            text: skill_context,
                        }],
                    },
                );
            }
            let mut mcp_context = cx.app.mcp.context();
            let previous = crate::skills::last_catalog(&transcript.messages, "<mcp_servers>");
            if mcp_context.is_empty() && previous.is_some() {
                mcp_context = crate::text::template::fill(
                    &crate::extensions::text().mcp_frame,
                    &[("servers", "")],
                );
            }
            if !mcp_context.is_empty() && previous != Some(mcp_context.as_str()) {
                record(
                    cx,
                    transcript,
                    Message::User {
                        parts: vec![Part::Reminder { text: mcp_context }],
                    },
                );
            }
            let registry = cx.tools.snapshot();
            let tools = registry.wire();

            if let Some(instructions) =
                crate::agent::instructions::current(&cx.app.paths, &transcript.messages)
                && crate::skills::last_catalog(&transcript.messages, "<active_instructions>")
                    != Some(&instructions)
            {
                record(
                    cx,
                    transcript,
                    Message::User {
                        parts: vec![Part::Reminder { text: instructions }],
                    },
                );
            }

            let mut messages = transcript.context();
            let agent_profile = shared.agent_profile();
            if !summary_failed
                && crate::agent::compact::can_compact(transcript)
                && crate::agent::compact::needed(&Turn {
                    model: cx.model,
                    agent_profile: agent_profile.as_deref(),
                    effort: cx.effort.as_deref(),
                    system: cx.system,
                    messages: &messages,
                    tools: &tools,
                })
            {
                summary_failed = !matches!(
                    crate::agent::compact::compact(cx, transcript, shared, &cancel, "", true).await,
                    Ok(true)
                );
                messages = transcript.context();
            }
            let turn = Turn {
                model: cx.model,
                agent_profile: agent_profile.as_deref(),
                effort: cx.effort.as_deref(),
                system: cx.system,
                messages: &messages,
                tools: &tools,
            };
            let reply = chat::stream(&cx.app.api, &turn, &cancel, &mut |event| {
                (cx.emit)(match event {
                    Event::Delta(delta) => AgentEvent::from(delta),
                });
            })
            .await;
            record(cx, transcript, Message::Assistant(reply.clone()));
            let finish = reply.finish;
            let overflow = reply.error.as_ref().is_some_and(|e| {
                [
                    "context_budget",
                    "context_length_exceeded",
                    "context_window_exceeded",
                ]
                .contains(&e.kind.as_str())
            });
            let calls: Vec<ToolCall> = reply.tool_calls().cloned().collect();
            let written = calls.is_empty() && reply.parts.iter().any(|part| matches!(part, Part::Text { text } if crate::agent::written::wrote_calls(text)));
            (cx.emit)(AgentEvent::MessageEnd { message: reply });
            if overflow && !overflow_retried && !summary_failed {
                overflow_retried = true;
                if matches!(
                    crate::agent::compact::compact(cx, transcript, shared, &cancel, "", true).await,
                    Ok(true)
                ) {
                    continue;
                }
                summary_failed = true;
            }
            match finish {
                Finish::Aborted => break 'run Outcome::Aborted,
                Finish::Error => break 'run Outcome::Failed,
                _ => {}
            }

            if !calls.is_empty() {
                let round = RunCtx {
                    app: cx.app,
                    model: cx.model,
                    effort: cx.effort.clone(),
                    system: cx.system,
                    tools: &registry,
                    plan_file: cx.plan_file.clone(),
                    emit: cx.emit,
                };
                let batch = if finish == Finish::Length {
                    // A reply cut off mid-call may carry truncated arguments; never run those.
                    batch::refuse_all(
                        &round,
                        &calls,
                        shared,
                        &cancel,
                        "truncated_tool_call",
                        &model_text::get().truncated_tool_call,
                    )
                    .await
                } else {
                    let history = &transcript.messages[..transcript.messages.len() - 1];
                    batch::run(&round, history, &calls, shared, &cancel).await
                };
                let Batch { results, stop } = batch;
                for result in results {
                    changed |= crate::agent::verify::changed(&result);
                    record(cx, transcript, Message::Tool(result));
                }
                if cancel.is_cancelled() {
                    break 'run Outcome::Aborted;
                }
                if stop {
                    break 'run Outcome::Rejected;
                }
                if finish == Finish::Length {
                    // Record balanced refusals, then require the user to request another paid turn.
                    break 'run Outcome::Failed;
                }
            }

            let steered = shared.drain_steer();
            if !steered.is_empty() {
                for parts in steered {
                    deliver(cx, shared, transcript, parts, &cancel, shadow.as_ref()).await;
                }
                continue;
            }
            if calls.is_empty() {
                if written && !told_written && !tools.is_empty() {
                    told_written = true;
                    record(
                        cx,
                        transcript,
                        Message::User {
                            parts: vec![Part::Reminder {
                                text: model_text::get().written_calls.clone(),
                            }],
                        },
                    );
                    continue;
                }
                break;
            }
        }
        let queued = shared.drain_follow();
        if queued.is_empty() {
            let verify_changed = if cx.app.settings.verify.is_some() && shared.depth == 0 {
                match (&shadow, &before) {
                    (Some(shadow), Some(before)) => shadow
                        .take()
                        .await
                        .map_or(changed, |after| after != *before),
                    _ => changed,
                }
            } else {
                changed
            };
            if let Some(call) = crate::agent::verify::call(
                &cx.app.settings,
                verify_changed,
                shared.depth == 0,
                verified,
            ) {
                verified += 1;
                let command = cx.app.settings.verify.as_deref().unwrap_or_default();
                let assistant = crate::agent::message::Assistant {
                    parts: vec![Part::ToolCall(call.clone())],
                    model: cx.model.id.clone(),
                    effort: None,
                    usage: None,
                    finish: Finish::ToolCalls,
                    error: None,
                    request_id: None,
                };
                record(cx, transcript, Message::Assistant(assistant.clone()));
                (cx.emit)(AgentEvent::MessageEnd { message: assistant });
                let verify_state = shared.child(cancel.clone());
                verify_state.set_cwd(cx.app.paths.project.clone());
                let checked = batch::run(
                    cx,
                    &transcript.messages[..transcript.messages.len() - 1],
                    &[call],
                    &verify_state,
                    &cancel,
                )
                .await;
                let follow = checked
                    .results
                    .first()
                    .and_then(|result| crate::agent::verify::failure(command, result));
                for result in checked.results {
                    record(cx, transcript, Message::Tool(result));
                }
                if cancel.is_cancelled() {
                    break Outcome::Aborted;
                }
                if checked.stop {
                    break Outcome::Rejected;
                }
                if let Some(parts) = follow {
                    record(cx, transcript, Message::User { parts });
                    continue;
                }
            }
            break Outcome::Done;
        }
        for parts in queued {
            deliver(cx, shared, transcript, parts, &cancel, shadow.as_ref()).await;
        }
    };
    if let Some(shadow) = &shadow {
        finish_checkpoint(cx, transcript, shadow).await;
    }
    (cx.emit)(AgentEvent::RunEnd { outcome });
    outcome
}

async fn finish_checkpoint(
    cx: &RunCtx<'_>,
    transcript: &mut Transcript,
    shadow: &crate::io::snapshot::Shadow,
) -> Option<String> {
    let after = shadow.take().await;
    if let Some(at) = crate::session::workflow::last_prompt(transcript) {
        let id = transcript.ids[at].clone();
        if let Some(snapshot) = transcript.snapshots.get(&id).cloned()
            && let Err(error) = transcript.snapshot(id, snapshot.before, after.clone())
        {
            (cx.emit)(AgentEvent::Unsaved {
                path: error.path.display().to_string(),
                error: error.error,
            });
        }
    }
    after
}

/// Keeps `message`; a session file that can no longer be written is reported once, and the
/// run goes on without it.
fn record(cx: &RunCtx<'_>, transcript: &mut Transcript, message: Message) {
    if let Err(unsaved) = transcript.push(message) {
        (cx.emit)(AgentEvent::Unsaved {
            path: unsaved.path.display().to_string(),
            error: unsaved.error,
        });
    }
}

/// Hands a message typed mid-run to the model, and tells the frontend it landed.
async fn deliver(
    cx: &RunCtx<'_>,
    shared: &Shared,
    transcript: &mut Transcript,
    parts: Vec<Part>,
    cancel: &tokio_util::sync::CancellationToken,
    shadow: Option<&crate::io::snapshot::Shadow>,
) {
    let before = match shadow {
        Some(shadow) => finish_checkpoint(cx, transcript, shadow).await,
        None => None,
    };
    let text = parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (cx.emit)(AgentEvent::Delivered { text });
    let mut parts = with_reminder(cx, shared, parts);
    crate::skills::expand(
        cx.app,
        shared,
        cx.emit,
        &cx.plan_file,
        cancel,
        &transcript.messages,
        &mut parts,
    )
    .await;
    record(cx, transcript, Message::User { parts });
    if let Some(before) = before
        && let Some(id) = transcript.ids.last().cloned()
        && let Err(error) = transcript.snapshot(id, before, None)
    {
        (cx.emit)(AgentEvent::Unsaved {
            path: error.path.display().to_string(),
            error: error.error,
        });
    }
}

/// Mode changes reach the model as a reminder on the next user message, so the system prompt
/// (and the vendor's cache of it) never changes mid-session.
fn with_reminder(cx: &RunCtx<'_>, shared: &Shared, mut parts: Vec<Part>) -> Vec<Part> {
    let text = match shared.take_reminder() {
        Some(Reminder::Entered(mode)) => mode
            .reminder
            .as_deref()
            .and_then(system_prompt::reminder)
            .map(|text| fill(text, &[("plan_file", &cx.plan_file)])),
        Some(Reminder::Left { now }) => {
            Some(fill(&model_text::get().mode_left, &[("mode", &now.label)]))
        }
        Some(Reminder::Restored { now }) => Some(fill(
            &model_text::get().mode_restored,
            &[("mode", &now.label)],
        )),
        None => None,
    };
    if let Some(text) = text {
        parts.insert(0, Part::Reminder { text });
    }
    parts
}

#[cfg(test)]
mod tests {
    //! The loop against a local fake chat endpoint that replays scripted replies.

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::routing::post;
    use serde_json::{Value, json};

    use super::*;
    use crate::agent::prompt::{Answer, Reply};
    use crate::api::Api;
    use crate::api::models::{Capabilities, Pricing};
    use crate::io::http::Http;
    use crate::mode;

    type Script = Arc<Mutex<(VecDeque<String>, Vec<Value>)>>;

    /// One scripted reply; `calls` are (id, tool, arguments).
    fn reply(text: &str, calls: &[(&str, &str, Value)], finish: &str) -> String {
        let mut frames = Vec::new();
        if !text.is_empty() {
            frames.push(json!({"choices": [{"index": 0, "delta": {"content": text}}]}));
        }
        for (i, (id, name, args)) in calls.iter().enumerate() {
            frames.push(json!({"choices": [{"index": 0, "delta": {"tool_calls": [
                {"index": i, "id": id, "type": "function",
                 "function": {"name": name, "arguments": args.to_string()}}]}}]}));
        }
        frames.push(json!({"choices": [{"index": 0, "delta": {}, "finish_reason": finish}]}));
        frames.push(json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 2}}));
        let mut body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
        body.push_str("data: [DONE]\n\n");
        body
    }

    fn text(t: &str) -> String {
        reply(t, &[], "stop")
    }

    struct Harness {
        _dir: tempfile::TempDir,
        app: App,
        script: Script,
        model: Model,
    }

    impl Harness {
        async fn new(replies: Vec<String>) -> Self {
            let script: Script = Arc::new(Mutex::new((replies.into(), Vec::new())));
            let state = script.clone();
            let router = Router::new().route(
                "/v1/chat/completions",
                post(move |body: String| {
                    let state = state.clone();
                    async move {
                        let mut s = state.lock().unwrap();
                        s.1.push(serde_json::from_str(&body).unwrap());
                        let next = s.0.pop_front().unwrap_or_else(|| text("(unscripted)"));
                        ([("content-type", "text/event-stream")], next)
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

            let dir = tempfile::tempdir().unwrap();
            let mut app = App::for_tests(dir.path());
            let http = Http::new(std::time::Duration::from_secs(5)).unwrap();
            app.api = Api::redirected(http, "k", &base);
            let model = Model {
                id: "m".into(),
                display_name: String::new(),
                description: String::new(),
                context_window: 100_000,
                max_output_tokens: 1000,
                default_max_output_tokens: None,
                available: true,
                pricing: Pricing {
                    input_per_1m_usd: 1.0,
                    cached_input_per_1m_usd: 0.1,
                    output_per_1m_usd: 2.0,
                },
                capabilities: Capabilities {
                    streaming: true,
                    tool_calling: true,
                    ..Capabilities::default()
                },
                input_modalities: None,
                capability_source: None,
                reasoning_efforts: None,
                default_reasoning_effort: None,
            };
            Self {
                _dir: dir,
                app,
                script,
                model,
            }
        }

        fn write(&self, path: &str, text: &str) {
            crate::io::fs::write_atomic(
                &self.app.paths.project.join(path),
                text.as_bytes(),
                crate::io::fs::Access::Shared,
            )
            .unwrap();
        }

        /// Runs one prompt; `on` sees every event and may drive `shared` like a frontend does.
        async fn run(
            &self,
            shared: &Shared,
            on: impl Fn(&AgentEvent, &Shared) + Send + Sync,
        ) -> (Outcome, Transcript, Vec<String>) {
            let registry = Registry::builtin(&self.app);
            let events = Mutex::new(Vec::new());
            let emit = |event: AgentEvent| {
                on(&event, shared);
                let kind = serde_json::to_value(&event).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                events.lock().unwrap().push(kind);
            };
            let cx = RunCtx {
                app: &self.app,
                model: &self.model,
                effort: None,
                system: "sys",
                tools: &registry,
                plan_file: self.plan_file(),
                emit: &emit,
            };
            let mut transcript = Transcript::new(None);
            let prompt = vec![Part::Text { text: "go".into() }];
            let outcome = run(&cx, &mut transcript, shared, prompt).await;
            (outcome, transcript, events.into_inner().unwrap())
        }

        fn plan_file(&self) -> String {
            self.app
                .paths
                .home
                .join("plan.md")
                .to_string_lossy()
                .replace('\\', "/")
        }

        fn requests(&self) -> Vec<Value> {
            self.script.lock().unwrap().1.clone()
        }
    }

    fn shared(mode: &str) -> Shared {
        Shared::new(mode::get(mode).unwrap())
    }

    #[tokio::test]
    async fn written_calls_are_corrected_only_once_and_opening_context_is_grounded() {
        let written = r#"I will read it. {"tool_calls":[{"name":"read"}]}"#;
        let h = Harness::new(vec![text(written), text(written)]).await;
        h.write("actual.txt", "contents");
        let (outcome, _, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(outcome, Outcome::Done);
        let requests = h.requests();
        assert_eq!(requests.len(), 2);
        let first = requests[0].to_string();
        assert!(first.contains("<workspace>"));
        assert!(first.contains("actual.txt"));
        assert!(
            last_message(&requests[1])
                .to_string()
                .contains("wrote tool calls into its text")
        );
    }

    #[tokio::test]
    async fn a_configured_failing_check_returns_to_the_model_and_records_balanced_calls() {
        let mut h = Harness::new(vec![
            reply(
                "",
                &[(
                    "write-1",
                    "write",
                    json!({"path":"work.txt", "content":"work"}),
                )],
                "tool_calls",
            ),
            text("Done"),
            reply(
                "",
                &[(
                    "write-2",
                    "write",
                    json!({"path":"ok.flag", "content":"ok"}),
                )],
                "tool_calls",
            ),
            text("Fixed"),
        ])
        .await;
        h.app.settings.verify = Some("test -f ok.flag".into());
        let (outcome, transcript, _) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(h.requests().len(), 4);
        assert!(
            h.requests()[2]
                .to_string()
                .contains("failed after your changes")
        );
        let calls = transcript
            .messages
            .iter()
            .filter_map(|message| {
                if let Message::Assistant(reply) = message {
                    Some(reply.tool_calls().count())
                } else {
                    None
                }
            })
            .sum::<usize>();
        let results = transcript
            .messages
            .iter()
            .filter(|message| matches!(message, Message::Tool(_)))
            .count();
        assert_eq!(calls, results);
        assert!(transcript.messages.iter().any(|message| matches!(message, Message::Tool(result) if result.name == "bash" && !result.is_error)));
    }

    #[tokio::test]
    async fn a_manual_verification_check_requires_permission() {
        let mut h = Harness::new(vec![
            reply(
                "",
                &[(
                    "write-1",
                    "write",
                    json!({"path":"work.txt", "content":"work"}),
                )],
                "tool_calls",
            ),
            text("Done"),
        ])
        .await;
        h.app.settings.verify = Some("printf checked > verified.txt".into());
        let mut edit = crate::permission::rule::Rule {
            permission: "edit".into(),
            pattern: "*".into(),
            action: crate::permission::rule::Action::Allow,
        };
        h.app.settings.permission.push(edit.clone());
        edit.permission = "external_directory".into();
        h.app.settings.permission.push(edit);
        let (_, transcript, _) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    shared.answer(*id, crate::agent::prompt::Reply::Unavailable);
                }
            })
            .await;
        assert!(transcript.messages.iter().any(|message| matches!(message, Message::Tool(result) if result.name == "bash" && result.refused.is_some())));
        assert!(matches!(
            crate::io::fs::kind(&h.app.paths.project.join("verified.txt")),
            Ok(crate::io::fs::Kind::Missing)
        ));
    }

    #[tokio::test]
    async fn manual_compaction_uses_no_tools_and_keeps_original_messages() {
        let harness = Harness::new(vec![text("Earlier work summarized")]).await;
        let registry = Registry::builtin(&harness.app);
        let state = shared("auto");
        let mut transcript = Transcript::new(None);
        assert!(transcript.push(Message::user_text("older request")).is_ok());
        assert!(
            transcript
                .push(Message::user_text("latest request"))
                .is_ok()
        );
        let cx = RunCtx {
            app: &harness.app,
            model: &harness.model,
            effort: None,
            system: "sys",
            tools: &registry,
            plan_file: harness.plan_file(),
            emit: &|_| {},
        };
        let outcome =
            crate::agent::compact::run(&cx, &mut transcript, &state, "retain tests").await;
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.compactions.len(), 1);
        let requests = harness.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .get("tools")
                .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty))
        );
        let context = serde_json::to_string(&transcript.context()).unwrap();
        assert!(context.contains("Earlier work summarized"));
        assert!(context.contains("latest request"));
        assert!(!context.contains("older request"));
    }

    fn last_message(request: &Value) -> Value {
        request["messages"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn profile_change_reaches_the_next_model_request_only() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("r", "read", json!({"path":"note.txt"}))],
                "tool_calls",
            ),
            text("done"),
            text("again"),
        ])
        .await;
        h.write("note.txt", "hello");
        let shared = shared("auto");
        shared.set_agent_profile(Some("direct".into()));
        let (outcome, _, _) = h
            .run(&shared, |event, shared| {
                if matches!(event, AgentEvent::MessageEnd { .. }) {
                    shared.set_agent_profile(Some("deep".into()));
                }
            })
            .await;
        assert_eq!(outcome, Outcome::Done);
        shared.set_agent_profile(None);
        h.run(&shared, |_, _| {}).await;
        let requests = h.requests();
        assert_eq!(requests[0]["server_profile"], "direct");
        assert_eq!(requests[1]["server_profile"], "deep");
        assert!(requests[2].get("server_profile").is_none());
        assert_eq!(shared.mode().id, "auto");
        shared.set_agent_profile(Some("review".into()));
        shared.start_over();
        assert!(shared.agent_profile().is_none());
        assert_eq!(shared.mode().id, "auto");
    }

    #[tokio::test]
    async fn parallel_results_come_back_in_call_order() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "read", json!({"path": "a.txt"})),
                    ("c2", "read", json!({"path": "b.txt"})),
                ],
                "tool_calls",
            ),
            text("done"),
        ])
        .await;
        h.write("a.txt", "AAA");
        h.write("b.txt", "BBB");
        let (outcome, transcript, _) = h.run(&shared("manual"), |_, _| {}).await;
        assert_eq!(outcome, Outcome::Done);
        let results: Vec<&str> = transcript
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Tool(r) => Some(r.call_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(results, vec!["c1", "c2"]);
        let roles: Vec<String> = h.requests()[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(roles, vec!["system", "user", "assistant", "tool", "tool"]);
    }

    #[tokio::test]
    async fn steering_lands_after_the_tools_and_before_the_next_turn() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("c1", "read", json!({"path": "a.txt"}))],
                "tool_calls",
            ),
            text("done"),
        ])
        .await;
        h.write("a.txt", "x");
        h.run(&shared("manual"), |event, shared| {
            if matches!(event, AgentEvent::ToolStart { .. }) {
                shared.steer(vec![Part::Text {
                    text: "also look at b".into(),
                }]);
            }
        })
        .await;
        let last = last_message(&h.requests()[1]);
        assert_eq!(last["role"], "user");
        assert_eq!(last["content"], "also look at b");
    }

    #[tokio::test]
    async fn follow_ups_wait_until_the_model_is_done() {
        let h = Harness::new(vec![text("first"), text("second")]).await;
        let state = shared("manual");
        state.follow_up(vec![Part::Text {
            text: "and then?".into(),
        }]);
        let (outcome, _, _) = h.run(&state, |_, _| {}).await;
        assert_eq!(outcome, Outcome::Done);
        let requests = h.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(last_message(&requests[1])["content"], "and then?");
    }

    #[tokio::test]
    async fn a_plain_rejection_stops_the_run() {
        let h = Harness::new(vec![reply(
            "",
            &[("c1", "bash", json!({"command": "printf hi"}))],
            "tool_calls",
        )])
        .await;
        let (outcome, transcript, _) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    shared.answer(*id, Reply::No { feedback: None });
                }
            })
            .await;
        assert_eq!(outcome, Outcome::Rejected);
        assert_eq!(h.requests().len(), 1);
        assert!(matches!(transcript.messages.last(), Some(Message::Tool(r)) if r.is_error));
    }

    #[tokio::test]
    async fn feedback_on_a_rejection_reaches_the_model() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("c1", "bash", json!({"command": "rm x"}))],
                "tool_calls",
            ),
            text("ok"),
        ])
        .await;
        h.run(&shared("manual"), |event, shared| {
            if let AgentEvent::Prompt { id, .. } = event {
                shared.answer(
                    *id,
                    Reply::No {
                        feedback: Some("use trash instead".into()),
                    },
                );
            }
        })
        .await;
        let last = last_message(&h.requests()[1]);
        assert!(
            last["content"]
                .as_str()
                .unwrap()
                .contains("use trash instead")
        );
    }

    #[tokio::test]
    async fn the_question_tool_carries_the_answer_back() {
        let h = Harness::new(vec![
            reply(
                "",
                &[(
                    "c1",
                    "question",
                    json!({"questions": [
                        {"question": "Which DB?", "options": ["sqlite", "postgres"]},
                        {"question": "Which checks?", "options": ["lint", "test"], "multiple": true}
                    ]}),
                )],
                "tool_calls",
            ),
            text("ok"),
        ])
        .await;
        let (_, _, events) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    let answers = vec![
                        Answer {
                            picked: vec![1],
                            text: None,
                        },
                        Answer {
                            picked: vec![0, 1],
                            text: None,
                        },
                    ];
                    shared.answer(*id, Reply::Answers(answers));
                }
            })
            .await;
        assert_eq!(events.iter().filter(|e| *e == "prompt").count(), 1);
        let result = last_message(&h.requests()[1]);
        let content = result["content"].as_str().unwrap();
        assert!(content.contains("Which DB?\n  postgres"), "{content}");
        assert!(content.contains("Which checks?\n  lint, test"), "{content}");
    }

    #[tokio::test]
    async fn an_approved_plan_switches_mode() {
        let h = Harness::new(vec![
            reply("", &[("c1", "plan_exit", json!({}))], "tool_calls"),
            text("implementing"),
        ])
        .await;
        crate::io::fs::write_atomic(
            std::path::Path::new(&h.plan_file()),
            b"1. do it",
            crate::io::fs::Access::Shared,
        )
        .unwrap();
        let state = shared("plan");
        h.run(&state, |event, shared| {
            if let AgentEvent::Prompt { id, .. } = event {
                shared.answer(*id, Reply::Choice(0));
            }
        })
        .await;
        assert_eq!(state.mode().id, "manual");
        let result = last_message(&h.requests()[1]);
        assert!(result["content"].as_str().unwrap().contains("MANUAL"));
    }

    #[tokio::test]
    async fn switching_to_auto_approves_a_waiting_prompt() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("c1", "bash", json!({"command": "printf hi"}))],
                "tool_calls",
            ),
            text("done"),
        ])
        .await;
        let (outcome, transcript, _) = h
            .run(&shared("manual"), |event, shared| {
                if matches!(event, AgentEvent::Prompt { .. }) {
                    shared.set_mode(mode::get("auto").unwrap());
                }
            })
            .await;
        assert_eq!(outcome, Outcome::Done);
        let ran = transcript
            .messages
            .iter()
            .any(|m| matches!(m, Message::Tool(r) if r.content.contains("hi")));
        assert!(ran);
    }

    #[tokio::test]
    async fn mode_changes_at_a_prompt_apply_to_later_calls_in_the_same_batch() {
        for next in ["auto", "plan"] {
            let h = Harness::new(vec![
                reply(
                    "",
                    &[
                        ("c1", "bash", json!({"command": "printf hi"})),
                        ("c2", "write", json!({"path": "next.txt", "content": "x"})),
                    ],
                    "tool_calls",
                ),
                text("done"),
            ])
            .await;
            let (outcome, transcript, events) = h
                .run(&shared("manual"), |event, shared| {
                    if let AgentEvent::Prompt { id, .. } = event {
                        shared.set_mode(mode::get(next).unwrap());
                        shared.answer(*id, Reply::Yes);
                    }
                })
                .await;
            assert_eq!(outcome, Outcome::Done);
            assert_eq!(
                events.iter().filter(|e| *e == "prompt").count(),
                1,
                "{next}"
            );
            let result = transcript.messages.iter().find_map(|m| match m {
                Message::Tool(r) if r.call_id == "c2" => Some(r),
                _ => None,
            });
            let result = result.unwrap();
            assert_eq!(
                result.refused.as_deref(),
                (next == "plan").then_some("denied")
            );
            let path = h.app.paths.project.join("next.txt");
            assert_eq!(
                crate::io::fs::read_string(&path).unwrap().as_deref(),
                (next == "auto").then_some("x"),
                "{next}"
            );
        }
    }

    #[tokio::test]
    async fn mandatory_confirmation_stays_pending_when_switching_to_auto() {
        use crate::forums::{Forum, store};

        for mode in ["manual", "auto"] {
            let h = Harness::new(vec![
                reply(
                    "",
                    &[("post", "forum_post", json!({
                        "forum": "fixture", "section": "1", "subject": "Draft", "body": "Review me"
                    }))],
                    "tool_calls",
                ),
                text("done"),
            ])
            .await;
            store::add(
                &h.app.paths,
                Forum {
                    id: "fixture".into(),
                    name: "Fixture".into(),
                    base_url: "http://127.0.0.1:1".into(),
                    kind: "mobiquo".into(),
                    ..Forum::default()
                },
            )
            .unwrap();
            let (_, transcript, events) = h
                .run(&shared(mode), |event, shared| {
                    if let AgentEvent::Prompt { id, prompt, .. } = event {
                        let crate::agent::prompt::Prompt::Permission {
                            confirm, preview, ..
                        } = prompt
                        else {
                            panic!("{prompt:?}");
                        };
                        assert!(confirm);
                        assert!(preview.as_ref().unwrap().contains("Review me"));
                        shared.set_mode(crate::mode::get("auto").unwrap());
                        assert!(shared.waiting(*id));
                        assert!(shared.answer(*id, Reply::Unavailable));
                    }
                })
                .await;
            assert_eq!(events.iter().filter(|e| *e == "prompt").count(), 1);
            let result = transcript
                .messages
                .iter()
                .find_map(|m| match m {
                    Message::Tool(r) => Some(r),
                    _ => None,
                })
                .unwrap();
            assert_eq!(result.refused.as_deref(), Some("unconfirmed"));
            assert_eq!(
                result.asks,
                [crate::permission::gate::Ask::new("forum_post", "127.0.0.1")]
            );
        }
    }

    #[tokio::test]
    async fn calls_in_a_cut_off_reply_are_never_run() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "write", json!({"path": "new.txt", "content": "x"})),
                    ("c2", "write", json!({"path": "other.txt", "content": "y"})),
                ],
                "length",
            ),
            text("ok"),
        ])
        .await;
        let (outcome, transcript, events) = h.run(&shared("auto"), |_, _| {}).await;
        assert_eq!(outcome, Outcome::Failed);
        for path in ["new.txt", "other.txt"] {
            assert_eq!(
                crate::io::fs::kind(&h.app.paths.project.join(path)).unwrap(),
                crate::io::fs::Kind::Missing
            );
        }
        let results: Vec<_> = transcript
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Tool(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 2);
        for result in results {
            assert_eq!(result.refused.as_deref(), Some("truncated_tool_call"));
            assert!(result.is_error);
            assert!(result.asks.is_empty());
        }
        for event in ["tool_start", "tool_end"] {
            assert_eq!(events.iter().filter(|e| *e == event).count(), 2, "{event}");
        }
        assert_eq!(h.requests().len(), 1);
    }

    #[tokio::test]
    async fn results_record_what_was_asked_and_why_a_call_was_refused() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "read", json!({"path": "a.txt"})),
                    ("c2", "nope", json!({})),
                    ("c3", "bash", json!({"command": "printf hi"})),
                ],
                "tool_calls",
            ),
            text("ok"),
        ])
        .await;
        h.write("a.txt", "x");
        let (_, transcript, _) = h
            .run(&shared("manual"), |event, shared| {
                if let AgentEvent::Prompt { id, .. } = event {
                    shared.answer(*id, Reply::Unavailable);
                }
            })
            .await;
        let results: Vec<_> = transcript
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Tool(r) => Some((r.asks.clone(), r.refused.as_deref())),
                _ => None,
            })
            .collect();
        let read = crate::permission::gate::Ask::new("read", "a.txt");
        let bash = crate::permission::gate::Ask::new("bash", "printf hi");
        assert_eq!(
            results,
            [
                (vec![read], None),
                (vec![], Some("unknown_tool")),
                (vec![bash], Some("non_interactive")),
            ]
        );
    }

    #[tokio::test]
    async fn repeated_identical_calls_are_stopped_in_auto() {
        let call = |id| reply("", &[(id, "read", json!({"path": "a.txt"}))], "tool_calls");
        let h = Harness::new(vec![call("c1"), call("c2"), call("c3"), text("done")]).await;
        h.write("a.txt", "x");
        h.run(&shared("auto"), |_, _| {}).await;
        let third = last_message(&h.requests()[3]);
        assert!(
            third["content"].as_str().unwrap().contains("3 times"),
            "{third}"
        );
    }

    #[tokio::test]
    async fn plan_mode_reminds_the_model_and_refuses_edits() {
        let h = Harness::new(vec![
            reply(
                "",
                &[("c1", "write", json!({"path": "x.txt", "content": "x"}))],
                "tool_calls",
            ),
            text("planned"),
        ])
        .await;
        let (outcome, _, _) = h.run(&shared("plan"), |_, _| {}).await;
        assert_eq!(outcome, Outcome::Done);
        let first = last_message(&h.requests()[0]);
        assert!(first["content"].as_str().unwrap().contains("PLAN mode"));
        let result = last_message(&h.requests()[1]);
        assert!(result["content"].as_str().unwrap().contains("Not allowed"));
    }

    #[tokio::test]
    async fn a_refusal_reaches_the_model_as_what_it_is() {
        let h = Harness::new(vec![
            reply(
                "",
                &[
                    ("c1", "write", json!({"path": "d", "content": "x"})),
                    ("c2", "write", json!({"path": "e.txt"})),
                ],
                "tool_calls",
            ),
            text("ok"),
        ])
        .await;
        h.write("d/inner.txt", "x");
        h.run(&shared("auto"), |_, _| {}).await;
        let messages = h.requests()[1]["messages"].as_array().unwrap().clone();
        let results: Vec<&str> = messages[messages.len() - 2..]
            .iter()
            .map(|m| m["content"].as_str().unwrap())
            .collect();
        assert_eq!(results[0], "d is a directory.");
        assert_eq!(
            results[1]
                .to_lowercase()
                .matches("invalid arguments")
                .count(),
            1,
            "{}",
            results[1]
        );
    }

    #[tokio::test]
    async fn leaving_plan_mode_is_told_to_the_model_once() {
        let h = Harness::new(Vec::new()).await;
        let told = |i: usize| {
            let content = last_message(&h.requests()[i])["content"].to_string();
            content.contains("no longer apply")
        };
        let session = shared("plan");
        h.run(&session, |_, _| {}).await;
        session.set_mode(mode::get("manual").unwrap());
        h.run(&session, |_, _| {}).await;
        h.run(&session, |_, _| {}).await;
        assert_eq!([told(0), told(1), told(2)], [false, true, false]);

        let session = shared("manual");
        h.run(&session, |_, _| {}).await;
        session.set_mode(mode::get("auto").unwrap());
        h.run(&session, |_, _| {}).await;
        assert_eq!([told(3), told(4)], [false, false]);
    }
}

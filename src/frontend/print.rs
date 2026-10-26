//! Headless mode: one prompt in, the agent runs to completion, the reply (or a JSON event
//! stream) out. Permission prompts cannot be answered here, so they are declined.

use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::anyhow;

use crate::agent::event::{AgentEvent, DeltaKind, Outcome};
use crate::agent::message::{Assistant, Finish, Part, ToolResult};
use crate::agent::prompt::{Prompt, Reply};
use crate::agent::run::{self, RunCtx};
use crate::agent::state::Shared;
use crate::agent::system_prompt;
use crate::api::models;
use crate::app::App;
use crate::io::term;
use crate::limits;
use crate::permission::gate;
use crate::session::transcript::Transcript;
use crate::text::shorten;
use crate::tools::{self, Registry};

/// How much of an argument or error a one-line note shows.
const NOTE_CHARS: usize = 100;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

pub async fn run(
    app: &App,
    prompt: String,
    format: Format,
    start: &crate::session::Start,
) -> anyhow::Result<ExitCode> {
    if !app.api.is_configured() {
        anyhow::bail!(crate::backend::text().unavailable.clone());
    }
    let catalog = models::load(app, true).await;
    let mut transcript = Transcript::open(&app.paths, &start.selection)?;
    // An explicit model must match the configured backend catalog.
    let model = catalog.selected(start.model(app, &transcript.state))?;
    let mode = start.mode(app, &transcript.state)?;
    let selected_prompt = start.prompt(app, &transcript.state).to_owned();
    let system = system_prompt::headless(system_prompt::selected(app, model, &selected_prompt)?);
    let registry = Registry::builtin(app);
    let shared = Shared::new(mode);
    shared.set_agent_profile(transcript.state.agent_profile.clone());
    if !matches!(start.selection, crate::session::store::Selection::New) {
        shared.resumed();
    }
    transcript.state.system_prompt = selected_prompt;
    let plan_file = tools::plan_file(&app.paths, &transcript.id());

    let out = Printer {
        format,
        shared: &shared,
        output_limit: crate::api::preflight::output_limit(model),
        tools_enabled: model.capabilities.tool_calling,
        at_line_start: AtomicBool::new(true),
        last: Mutex::new(None),
        declined: Mutex::new(Vec::new()),
    };
    let emit = |event: AgentEvent| out.event(event);
    let cx =
        RunCtx {
            app,
            model,
            effort: transcript.state.effort.clone().filter(|effort| {
                transcript.state.model == model.id && model.supports_effort(effort)
            }),
            system: &system,
            tools: &registry,
            plan_file,
            emit: &emit,
        };

    let interrupt = async {
        term::interrupted().await;
        shared.interrupt();
        std::future::pending::<()>().await;
    };
    let prompt = vec![Part::Text { text: prompt }];
    let mut proposed = transcript.context();
    proposed.push(crate::agent::message::Message::User {
        parts: prompt.clone(),
    });
    if let Some(text) = crate::agent::instructions::current(&app.paths, &proposed) {
        proposed.push(crate::agent::message::Message::User {
            parts: vec![Part::Reminder { text }],
        });
    }
    let proposed = crate::agent::context::build(&proposed);
    crate::api::preflight::check(&crate::api::chat::Turn {
        model,
        system: &system,
        messages: &proposed,
        tools: &registry.wire(),
        agent_profile: None,
        effort: cx.effort.as_deref(),
    })
    .or_else(|error| {
        if error.kind == "context_budget" && crate::agent::compact::can_compact(&transcript) {
            Ok(())
        } else {
            Err(error)
        }
    })
    .map_err(|error| anyhow!(error.to_string()))?;
    let outcome = tokio::select! {
        outcome = run::run(&cx, &mut transcript, &shared, prompt) => outcome,
        () = interrupt => unreachable!("the interrupt future never finishes"),
    };
    Ok(out.finish(outcome))
}

struct Printer<'a> {
    format: Format,
    shared: &'a Shared,
    output_limit: u64,
    tools_enabled: bool,
    /// Whether stdout ends at a line start, so stderr notes do not land mid-sentence.
    at_line_start: AtomicBool,
    last: Mutex<Option<Assistant>>,
    /// Calls already reported at their declined prompt; their start and end say nothing new.
    declined: Mutex<Vec<String>>,
}

impl Printer<'_> {
    fn event(&self, event: AgentEvent) {
        if let AgentEvent::Child { event: child, .. } = &event
            && let AgentEvent::Prompt { id, .. } = child.as_ref()
        {
            self.shared.answer(*id, Reply::Unavailable);
        }
        // Nobody can answer a prompt here; decline it so the model hears why and carries on.
        if let AgentEvent::Prompt { id, .. } = &event {
            self.shared.answer(*id, Reply::Unavailable);
        }
        if let AgentEvent::MessageEnd { message } = &event {
            *self.last.lock().unwrap() = Some(message.clone());
        }
        match self.format {
            Format::Json => {
                if let Ok(line) = serde_json::to_string(&event) {
                    term::out(&format!("{line}\n"));
                }
            }
            Format::Text => self.text(&event),
        }
    }

    fn text(&self, event: &AgentEvent) {
        match event {
            AgentEvent::Delta {
                kind: DeltaKind::Text,
                text,
                ..
            } => {
                term::out(text);
                self.at_line_start
                    .store(text.ends_with('\n'), Ordering::Relaxed);
            }
            AgentEvent::ToolStart { call_id, .. }
            | AgentEvent::ToolEnd {
                result: ToolResult { call_id, .. },
            } if self.declined.lock().unwrap().contains(call_id) => {}
            AgentEvent::ToolStart {
                name, arguments, ..
            } => self.note(&format!("› {name} {}", summary(arguments))),
            AgentEvent::ToolEnd { result } if result.is_error => {
                let first = result.content.lines().next().unwrap_or_default();
                self.note(&format!("  ✗ {}", shorten::line(first, NOTE_CHARS)));
            }
            AgentEvent::Prompt {
                prompt:
                    Prompt::Permission {
                        tool,
                        asks,
                        confirm,
                        ..
                    },
                call_id,
                ..
            } => {
                self.declined.lock().unwrap().push(call_id.clone());
                let what = shorten::line(&gate::describe(asks), NOTE_CHARS);
                if *confirm {
                    self.note(&crate::text::template::fill(
                        &crate::session::text::get().confirmation_required,
                        &[("tool", tool), ("what", &what)],
                    ));
                } else {
                    self.note(&format!(
                        "  ✗ {tool} needs permission ({what}); rerun with --mode auto to allow it"
                    ));
                }
            }
            AgentEvent::Retry {
                delay_ms, error, ..
            } => self.note(&format!(
                "{}; retrying in {:.1}s",
                error.entry().title,
                *delay_ms as f64 / 1000.0
            )),
            AgentEvent::Unsaved { path, error } => {
                self.note(&format!(
                    "palepale: no longer saving this session to {path}: {error}"
                ));
            }
            _ => {}
        }
    }

    fn note(&self, line: &str) {
        if !self.at_line_start.swap(true, Ordering::Relaxed) {
            term::out("\n");
        }
        term::err(&format!("{line}\n"));
    }

    fn finish(&self, outcome: Outcome) -> ExitCode {
        if self.format == Format::Text && !self.at_line_start.load(Ordering::Relaxed) {
            term::out("\n");
        }
        let last = self.last.lock().unwrap().take();
        if last.as_ref().is_some_and(|m| m.finish == Finish::Length) {
            if self.tools_enabled {
                term::err(&format!(
                    "palepale: the reply reached the {}-token output limit; an incomplete tool call was not run\n",
                    self.output_limit
                ));
            } else {
                term::err(&format!(
                    "palepale: the reply reached the {}-token output limit\n",
                    self.output_limit
                ));
            }
        }
        match outcome {
            Outcome::Aborted => {
                term::err("palepale: cancelled\n");
                ExitCode::from(130)
            }
            Outcome::Failed => {
                if let Some(error) = last.and_then(|m| m.error) {
                    if error.partial {
                        term::err(&format!("palepale: {error}\n"));
                        return ExitCode::from(1);
                    }
                    let entry = error.entry();
                    term::err(&format!("palepale: {}: {error}\n", entry.title));
                    if let Some(hint) = &entry.hint {
                        term::err(&format!("  {hint}\n"));
                    }
                }
                ExitCode::FAILURE
            }
            Outcome::TurnLimit => {
                let limit = limits::get().agent.max_turns.value;
                term::err(&format!("palepale: stopped after {limit} turns\n"));
                ExitCode::FAILURE
            }
            Outcome::Rejected => ExitCode::FAILURE,
            Outcome::Done => ExitCode::SUCCESS,
        }
    }
}

/// The argument a person recognises a call by: its path, command, pattern, URL or query.
fn summary(arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    let found = ["path", "command", "pattern", "url", "query"]
        .iter()
        .find_map(|key| args[key].as_str())
        .unwrap_or_default();
    shorten::line(found.lines().next().unwrap_or_default(), NOTE_CHARS)
}

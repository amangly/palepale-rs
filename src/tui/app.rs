//! The interactive session: one loop over terminal input, agent events, the turn in flight and a
//! frame tick. The loop owns the transcript and lends it to each turn, which hands it back, so a
//! turn runs alongside input without an engine thread.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::time::Instant;

use anyhow::anyhow;
use futures_util::future::BoxFuture;
use futures_util::stream::FuturesUnordered;
use futures_util::{FutureExt, StreamExt};
use tokio::sync::mpsc;

use crate::agent::event::{AgentEvent, Outcome};
use crate::agent::message::{Part, Usage};
use crate::agent::run::{self, RunCtx};
use crate::agent::state::Shared;
use crate::agent::system_prompt;
use crate::api::account;
use crate::api::models::{self, Catalog, Model};
use crate::app::App;
use crate::commands::{self, Effect, Outcome as CommandOutcome, Scope, Session};
use crate::io::term::{self, Button, Input, KeyCode};
use crate::io::{clipboard, clock};
use crate::limits;
use crate::mode;
use crate::session::store::Store;
use crate::session::transcript::Transcript;
use crate::text::styled::{Line, Style, TAB};
use crate::text::template;
use crate::text::theme;
use crate::tools::{self, Registry};
use crate::tui::card::{self, Card};
use crate::tui::command::{self, Work};
use crate::tui::dashboard::{self, Dashboard};
use crate::tui::editor::Editor;
use crate::tui::effort as effort_picker;
use crate::tui::feed::{Block, Feed};
use crate::tui::forums::{self, Forums};
use crate::tui::keymap::{self, Action};
use crate::tui::login::{self, Login};
use crate::tui::palette::{self, Palette};
use crate::tui::paste::Burst;
use crate::tui::picker::{self, Picker};
use crate::tui::queue::{self, Kind};
use crate::tui::screen::Screen;
use crate::tui::status::{self, Progress};
use crate::tui::toast::Toast;
use crate::tui::view::View;
use crate::tui::{frame, layout, ui, welcome};
use crate::{settings, update};

type Turn<'a> = Pin<Box<dyn Future<Output = (Transcript, Outcome)> + 'a>>;

/// What every turn shares. The model and system prompt are not here: the session can switch
/// them between turns.
struct Turns<'a> {
    tools: &'a Registry,
    emit: &'a (dyn Fn(AgentEvent) + Send + Sync),
}

/// What the loop should do after an input.
enum Step<'a> {
    Continue,
    Send(String),
    Quit,
    /// Start network work for the card on screen.
    Job(BoxFuture<'a, Done>),
    /// Put the conversation away and start a fresh one.
    NewConversation,
    /// Run beside input: a session command, or what one of them asked for.
    Work(BoxFuture<'a, Work>),
}

/// What a card's network work came to, for the card that started it.
enum Done {
    Login(login::Done),
    Forums(forums::Done),
}

pub async fn run(app: &App, initial: Option<String>) -> anyhow::Result<ExitCode> {
    let catalog = models::load(app, false).await;
    // An explicit model must match the configured backend catalog.
    let model = if app.api.is_configured() && !catalog.models.is_empty() {
        Some(catalog.selected(&app.settings.model)?.clone())
    } else {
        None
    };
    let mode = mode::get(&app.settings.mode).ok_or_else(|| anyhow!("mode was validated"))?;
    let registry = Registry::builtin(app);
    let store = Store::create(&app.paths);
    let session_file = store.path.display().to_string();
    let transcript = Transcript::new(Some(store));
    let shared = Shared::new(mode);
    let (tx, rx) = mpsc::unbounded_channel();
    let emit = move |event: AgentEvent| {
        let _ = tx.send(event);
    };
    let turns = Turns {
        tools: &registry,
        emit: &emit,
    };

    let raw = term::Raw::fullscreen()?;
    let mut tui = Tui::new(app, catalog, model, &shared, initial, session_file);
    // Startup already finished updating; consume its saved notice once.
    if let Ok(exe) = crate::io::proc::current_exe() {
        crate::io::fs::sweep_replaced(&exe);
    }
    if let Some(change) = update::take_news(&app.paths) {
        tui.feed.notice(&change.opened(), "done");
    }
    let (_spent, transcript) = tui.run(turns, transcript, rx).await;
    drop(raw);

    let text = &ui::get().text;
    // Only a file that still holds the whole conversation counts as saved; a turn still
    // holding the transcript at exit has already written its prompt there.
    let last = match &transcript {
        Some(transcript) => transcript.path().map(|p| p.display().to_string()),
        None => Some(tui.session_file.clone()),
    };
    for path in tui.earlier.iter().chain(&last) {
        term::out(&format!(
            "{}\n",
            template::fill(&text.saved, &[("path", path)])
        ));
    }
    Ok(ExitCode::SUCCESS)
}

struct Tui<'a> {
    app: &'a App,
    catalog: Catalog,
    /// The model for the next turn, and the system prompt written for it.
    model: Option<Model>,
    /// The effort asked for: the setting until `/effort` changes it.
    effort: Option<String>,
    system: String,
    shared: &'a Shared,
    screen: Screen,
    feed: Feed,
    view: View,
    /// The transcript block on each screen row of the last frame, for clicks.
    blocks: Vec<Option<usize>>,
    editor: Editor,
    palette: Palette,
    burst: Burst,
    queue: Vec<(Kind, String)>,
    /// Prompts waiting on the user; the first is on screen in place of the editor.
    cards: VecDeque<Box<dyn Card>>,
    /// The `/login` card, on screen in place of the editor while open.
    login: Option<Login>,
    /// The `/models` card, likewise.
    picker: Option<Picker>,
    effort_picker: Option<effort_picker::Picker>,
    dashboard: Option<Dashboard>,
    dashboard_id: u64,
    session_usage: Usage,
    session_requests: u64,
    /// The `/forums` card, likewise.
    forums: Option<Forums>,
    /// The turn in flight, for the working line.
    progress: Option<Progress>,
    last_verb: Option<usize>,
    rng: fastrand::Rng,
    braille: bool,
    last_ctrl_c: Option<Instant>,
    toast: Option<Toast>,
    cost_micros: u64,
    /// The current conversation's session file, and those of conversations `/new` put away.
    session_file: String,
    earlier: Vec<String>,
    /// Input tokens of the last request; the bar divides by the current model's window, so a
    /// switch of model shows its own share at once.
    context_tokens: Option<u64>,
}

impl<'a> Tui<'a> {
    fn new(
        app: &'a App,
        catalog: Catalog,
        model: Option<Model>,
        shared: &'a Shared,
        initial: Option<String>,
        session_file: String,
    ) -> Self {
        let limits = &limits::get().tui;
        let (width, height) = term::size();
        let mut editor = Editor::new(limits.history_max.value);
        if let Some(text) = initial {
            editor.set_text(&text);
        }
        let effort = app.settings.effort.clone().filter(|id| {
            model
                .as_ref()
                .is_some_and(|model| model.supports_effort(id))
        });
        let mut tui = Self {
            app,
            catalog,
            system: model
                .as_ref()
                .map(|model| system_prompt::build(&app.paths, model))
                .unwrap_or_default(),
            model,
            effort,
            shared,
            screen: Screen::new(width, height, theme::get(), term::color_depth()),
            feed: Feed::new(),
            view: View::default(),
            blocks: Vec::new(),
            editor,
            palette: Palette::default(),
            burst: Burst::new(limits.paste_gap_ms.ms(), limits.paste_min_keys.value),
            queue: Vec::new(),
            cards: VecDeque::new(),
            login: None,
            picker: None,
            effort_picker: None,
            dashboard: None,
            dashboard_id: 0,
            session_usage: Usage::default(),
            session_requests: 0,
            forums: None,
            progress: None,
            last_verb: None,
            rng: fastrand::Rng::new(),
            braille: term::braille(),
            last_ctrl_c: None,
            toast: None,
            cost_micros: 0,
            session_file,
            earlier: Vec::new(),
            context_tokens: None,
        };
        let welcome = tui.welcome();
        tui.feed.push(Block::Welcome(welcome));
        tui
    }

    /// Share of the current model's window the last request used.
    fn context_pct(&self) -> Option<u64> {
        self.context_tokens
            .zip(self.model.as_ref())
            .map(|(tokens, model)| tokens * 100 / model.context_window.max(1))
    }

    /// What this session reports about itself to a command.
    fn session(&self) -> Session {
        Session {
            model: self
                .model
                .as_ref()
                .map(|m| m.id.clone())
                .unwrap_or_else(|| {
                    let backend = crate::backend::text();
                    if self.app.api.is_configured() {
                        backend.connected.clone()
                    } else {
                        backend.disconnected.clone()
                    }
                }),
            effort: self
                .model
                .as_ref()
                .and_then(|m| m.effort(self.effort.as_deref()))
                .map(str::to_owned),
            mode: self.shared.mode().label.clone(),
            context_pct: self.context_pct(),
            file: self.session_file.clone(),
        }
    }

    fn welcome(&self) -> welcome::Info {
        let mode = self.shared.mode();
        welcome::Info {
            version: crate::release::tag().unwrap_or(concat!("v", env!("CARGO_PKG_VERSION"))),
            cwd: self.app.paths.project.display().to_string(),
            model: self
                .model
                .as_ref()
                .map(|m| m.id.clone())
                .unwrap_or_else(|| crate::backend::text().disconnected.clone()),
            effort: self
                .model
                .as_ref()
                .and_then(|m| m.effort(self.effort.as_deref()))
                .map(str::to_owned),
            mode_label: mode.label.clone(),
            mode_color: mode.color.clone(),
            backend_configured: self.app.api.is_configured(),
            tools_supported: self
                .model
                .as_ref()
                .is_some_and(|m| m.capabilities.tool_calling),
            braille: term::braille(),
        }
    }

    /// A new conversation: a new session file, a clean screen, nothing the model was told.
    fn fresh(&mut self, old: Transcript) -> Transcript {
        let store = Store::create(&self.app.paths);
        let put_away = std::mem::replace(&mut self.session_file, store.path.display().to_string());
        self.shared.start_over();
        // AGENTS.md may have changed (/init writes it), and the prompt is read at a start.
        self.system = self
            .model
            .as_ref()
            .map(|model| system_prompt::build(&self.app.paths, model))
            .unwrap_or_default();
        self.feed = Feed::new();
        let welcome = self.welcome();
        self.feed.push(Block::Welcome(welcome));
        self.view = View::default();
        self.queue.clear();
        self.context_tokens = None;
        self.session_usage = Usage::default();
        self.session_requests = 0;
        let text = &ui::get().text;
        match old.path() {
            Some(_) => {
                self.feed.notice(
                    &template::fill(&text.new_saved, &[("path", &put_away)]),
                    "muted",
                );
                self.earlier.push(put_away);
            }
            None => self.feed.notice(&text.new_conversation, "muted"),
        }
        Transcript::new(Some(store))
    }

    /// Runs until the user quits; returns what the session spent, in micro-dollars, and the
    /// transcript unless a turn still held it.
    async fn run(
        &mut self,
        turns: Turns<'a>,
        transcript: Transcript,
        mut events: mpsc::UnboundedReceiver<AgentEvent>,
    ) -> (u64, Option<Transcript>) {
        let mut transcript = Some(transcript);
        let mut turn: Option<Turn<'a>> = None;
        let mut job: Option<BoxFuture<'a, Done>> = None;
        let mut work: FuturesUnordered<BoxFuture<'a, Work>> = FuturesUnordered::new();
        if self.app.api.is_configured() && settings::env("PALEPALE_NO_AUTO_LOGIN").is_none() {
            if self.app.api.has_key() {
                let app = self.app;
                let revision = app.api.key_revision();
                work.push(Box::pin(async move {
                    Work::Account {
                        revision,
                        result: account::me(&app.api).await,
                    }
                }));
            } else {
                self.login = Some(Login::starting());
                job = Some(
                    login::run(self.app, login::Job::Start)
                        .map(Done::Login)
                        .boxed(),
                );
            }
        }
        let mut inputs = Box::pin(term::inputs());
        let mut tick = tokio::time::interval(limits::get().tui.frame_ms.ms());
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let shared = self.shared;
        loop {
            self.draw(turn.is_some());
            let step = tokio::select! {
                input = inputs.next() => {
                    let Some(input) = input else { break };
                    self.input(input, turn.is_some())
                }
                Some(event) = events.recv() => {
                    self.agent(event);
                    while let Ok(event) = events.try_recv() {
                        self.agent(event);
                    }
                    Step::Continue
                }
                (owned, _) = async { turn.as_mut().expect("guarded by the branch condition").await }, if turn.is_some() => {
                    turn = None;
                    transcript = Some(owned);
                    self.progress = None;
                    self.queue.clear();
                    while let Ok(event) = events.try_recv() {
                        self.agent(event);
                    }
                    Step::Continue
                }
                done = async { job.as_mut().expect("guarded by the branch condition").await }, if job.is_some() => {
                    job = None;
                    self.job_done(done)
                }
                Some(done) = work.next(), if !work.is_empty() => self.finish(done),
                // Frames tick while something moves: a turn, or a toast that has to go away.
                _ = tick.tick(), if turn.is_some() || self.toast.is_some() => Step::Continue,
            };
            match step {
                Step::Continue => {}
                Step::Quit => break,
                Step::Job(run) => job = Some(run),
                Step::Work(run) => work.push(run),
                Step::Send(text) => {
                    let Some(model) = self.model.clone() else {
                        self.feed.notice(&models_note(&self.catalog), "warn");
                        continue;
                    };
                    // Typed text queues while a turn runs; only a command's message gets here.
                    let Some(mut owned) = transcript.take() else {
                        self.feed.notice(&ui::get().text.busy, "warn");
                        continue;
                    };
                    let progress = Progress::start(clock::instant(), self.last_verb, &mut self.rng);
                    self.last_verb = Some(progress.verb());
                    self.progress = Some(progress);
                    let parts = vec![Part::Text { text }];
                    let (app, tools, emit) = (self.app, turns.tools, turns.emit);
                    let plan_file = tools::plan_file(&app.paths, &owned.id());
                    let system = self.system.clone();
                    let effort = self.effort.clone();
                    turn = Some(Box::pin(async move {
                        let cx = RunCtx {
                            app,
                            model: &model,
                            effort: effort.as_deref(),
                            system: &system,
                            tools,
                            plan_file,
                            emit,
                        };
                        let outcome = run::run(&cx, &mut owned, shared, parts).await;
                        (owned, outcome)
                    }));
                }
                // A turn holds the transcript while it runs.
                Step::NewConversation => match transcript.take() {
                    Some(old) => transcript = Some(self.fresh(old)),
                    None => self.feed.notice(&ui::get().text.busy, "warn"),
                },
            }
            // Closing a card drops whatever it was waiting on.
            if self.login.is_none() && self.forums.is_none() {
                job = None;
            }
        }
        (self.cost_micros, transcript)
    }

    fn input(&mut self, input: Input, running: bool) -> Step<'a> {
        // Inside a burst of keys (a paste where the terminal sends one as keystrokes), Enter
        // and Tab are text for whatever field has the keys, never commands.
        let input = match input {
            Input::Key(key) => match (self.burst.is_paste(clock::instant()), key.code) {
                (true, KeyCode::Enter) => Input::Paste("\n".into()),
                (true, KeyCode::Tab) => Input::Paste(TAB.into()),
                _ => Input::Key(key),
            },
            other => other,
        };
        match input {
            Input::Resize(width, height) => {
                self.screen.resize(width, height);
                Step::Continue
            }
            Input::Scroll(notches) => {
                let lines = limits::get().tui.scroll_lines.value as isize;
                self.view.scroll(notches * lines);
                Step::Continue
            }
            Input::Click {
                row,
                button: Button::Left,
            } => {
                if let Some(Some(block)) = self.blocks.get(row) {
                    self.feed.toggle(*block);
                }
                Step::Continue
            }
            // The login card's code wherever it is clicked, else the block under the pointer.
            Input::Click {
                row,
                button: Button::Right,
            } => {
                let text = self
                    .login
                    .as_ref()
                    .and_then(Login::code)
                    .map(str::to_owned)
                    .or_else(|| {
                        let block = self.blocks.get(row).copied().flatten()?;
                        self.feed.copy_text(block)
                    });
                if let Some(text) = text {
                    self.copy(&text);
                }
                Step::Continue
            }
            Input::Key(key)
                if matches!(
                    keymap::get().action(&key),
                    Some(Action::PageUp | Action::PageDown)
                ) =>
            {
                let page = self.view.page();
                let up = keymap::get().action(&key) == Some(Action::PageUp);
                self.view.scroll(if up { -page } else { page });
                Step::Continue
            }
            Input::Paste(_)
                if self.picker.is_some()
                    || self.effort_picker.is_some()
                    || self.dashboard.is_some() =>
            {
                Step::Continue
            }
            Input::Paste(text) => {
                if let Some(card) = &mut self.login {
                    card.insert(&text);
                } else if let Some(card) = &mut self.forums {
                    card.insert(&text);
                } else if let Some(card) = self.cards.front_mut() {
                    card.insert(&text);
                } else {
                    self.editor.insert(&text);
                }
                Step::Continue
            }
            Input::Key(key) if self.login.is_some() => self.login_key(&key),
            Input::Key(key) if self.forums.is_some() => self.forums_key(&key),
            Input::Key(key) if self.picker.is_some() => {
                self.picker_key(&key);
                Step::Continue
            }
            Input::Key(key) if self.effort_picker.is_some() => {
                self.effort_key(&key);
                Step::Continue
            }
            Input::Key(key) if self.dashboard.is_some() => self.dashboard_key(&key),
            Input::Key(key) if !self.cards.is_empty() => {
                self.card_key(&key, running);
                Step::Continue
            }
            Input::Key(key) => {
                match keymap::get().action(&key) {
                    Some(action) => {
                        // The command popup takes its keys first; it is closed during a run,
                        // where Tab steers.
                        if !running {
                            match self.palette.key(action, &self.editor.text()) {
                                palette::Step::Ignored => {}
                                palette::Step::Handled => return Step::Continue,
                                palette::Step::Complete(text) => {
                                    self.editor.set_text(&text);
                                    return Step::Continue;
                                }
                                palette::Step::Run(name) => {
                                    self.editor.set_text(&Scope::Session.invoke(&name));
                                    return self.submit(running);
                                }
                            }
                        }
                        self.action(action, running)
                    }
                    None => {
                        self.editor.type_key(&key);
                        Step::Continue
                    }
                }
            }
        }
    }

    /// Keys while a prompt card is up: the card answers most, the session keeps a few.
    fn card_key(&mut self, key: &crate::io::term::KeyEvent, running: bool) {
        let action = keymap::get().action(key);
        match action {
            Some(Action::CycleMode) => return self.cycle_mode(),
            Some(Action::CtrlC) if running => return self.interrupt(),
            _ => {}
        }
        let Some(card) = self.cards.front_mut() else {
            return;
        };
        if let card::Step::Answer(reply) = card.key(action, key) {
            self.shared.answer(card.id(), reply);
            self.cards.pop_front();
        }
    }

    fn login_key(&mut self, key: &crate::io::term::KeyEvent) -> Step<'a> {
        let action = keymap::get().action(key);
        if action == Some(Action::CycleMode) {
            self.cycle_mode();
            return Step::Continue;
        }
        let next = match &mut self.login {
            Some(card) => card.key(action, key),
            None => return Step::Continue,
        };
        self.login_next(next)
            .map_or(Step::Continue, |job| self.login_job(job))
    }

    fn login_job(&self, job: login::Job) -> Step<'a> {
        Step::Job(login::run(self.app, job).map(Done::Login).boxed())
    }

    fn forums_key(&mut self, key: &crate::io::term::KeyEvent) -> Step<'a> {
        let action = keymap::get().action(key);
        if action == Some(Action::CycleMode) {
            self.cycle_mode();
            return Step::Continue;
        }
        match &mut self.forums {
            Some(card) => {
                let next = card.key(action, key);
                self.forums_next(next)
            }
            None => Step::Continue,
        }
    }

    fn forums_next(&mut self, next: forums::Next) -> Step<'a> {
        match next {
            forums::Next::Stay => {}
            forums::Next::Run(job) => {
                return Step::Job(forums::run(self.app, job).map(Done::Forums).boxed());
            }
            forums::Next::Note(text) => self.feed.notice(&text, "done"),
            forums::Next::Error(text) => self.feed.notice(&text, "error"),
            forums::Next::Close => self.forums = None,
        }
        Step::Continue
    }

    /// Hands a card's finished work back to it; what it asks next may be more work.
    fn job_done(&mut self, done: Done) -> Step<'a> {
        match done {
            Done::Login(done) => {
                let logged_in = matches!(&done, login::Done::LoggedIn(_));
                let next = self.login.as_mut().map(|card| card.finished(done));
                if let Some(job) = next.and_then(|next| self.login_next(next)) {
                    self.login_job(job)
                } else if logged_in && self.model.is_none() {
                    Step::Work(command::refresh_catalog(self.app))
                } else {
                    Step::Continue
                }
            }
            Done::Forums(done) => {
                // The prompt names the forums to browse; an added or removed one changes it.
                if matches!(done, forums::Done::Changed { .. }) {
                    self.system = self
                        .model
                        .as_ref()
                        .map(|model| system_prompt::build(&self.app.paths, model))
                        .unwrap_or_default();
                }
                match self.forums.as_mut().map(|card| card.finished(done)) {
                    Some(next) => self.forums_next(next),
                    None => Step::Continue,
                }
            }
        }
    }

    fn picker_key(&mut self, key: &crate::io::term::KeyEvent) {
        let action = keymap::get().action(key);
        if action == Some(Action::CycleMode) {
            return self.cycle_mode();
        }
        let Some(picker) = &mut self.picker else {
            return;
        };
        match picker.key(action) {
            picker::Step::Stay => {}
            picker::Step::Close => self.picker = None,
            picker::Step::Pick(id) => {
                self.picker = None;
                self.switch_model(&id);
                self.open_effort_picker();
            }
        }
    }

    /// Puts `text` on the clipboard and says so at the top right.
    fn copy(&mut self, text: &str) {
        clipboard::copy(text);
        self.toast = Some(Toast::new(&ui::get().text.copied, clock::instant()));
    }

    fn effort_key(&mut self, key: &crate::io::term::KeyEvent) {
        let action = keymap::get().action(key);
        if action == Some(Action::CycleMode) {
            return self.cycle_mode();
        }
        let Some(picker) = &mut self.effort_picker else {
            return;
        };
        match picker.key(action) {
            effort_picker::Step::Stay => {}
            effort_picker::Step::Close => self.effort_picker = None,
            effort_picker::Step::Pick(id) => {
                self.effort_picker = None;
                self.set_effort(id);
            }
        }
    }

    fn refresh_status(&mut self) -> Step<'a> {
        self.dashboard_id += 1;
        let loading = self.app.api.is_configured() && self.app.api.has_key();
        let revision = self.app.api.key_revision();
        if let Some(card) = &mut self.dashboard {
            card.refreshing(self.dashboard_id, revision, loading);
        } else {
            self.dashboard = Some(Dashboard::new(self.dashboard_id, revision, loading));
        }
        if loading {
            Step::Work(command::refresh_status(self.app, self.dashboard_id))
        } else {
            Step::Continue
        }
    }

    fn dashboard_key(&mut self, key: &crate::io::term::KeyEvent) -> Step<'a> {
        let action = keymap::get().action(key);
        if action == Some(Action::Quit) {
            return Step::Quit;
        }
        if action == Some(Action::CycleMode) {
            self.cycle_mode();
            return Step::Continue;
        }
        match self.dashboard.as_mut().map(|card| card.key(action, key)) {
            Some(dashboard::Step::Close) => self.dashboard = None,
            Some(dashboard::Step::Refresh) => return self.refresh_status(),
            _ => {}
        }
        Step::Continue
    }

    /// Asks for `effort` from the next turn on, for this session only.
    fn set_effort(&mut self, effort: Option<String>) {
        let text = &ui::get().text;
        if let Some(id) = effort.as_deref() {
            let Some(model) = &self.model else {
                self.feed.notice(&models_note(&self.catalog), "warn");
                return;
            };
            if !model.supports_effort(id) {
                let levels = model.effort_levels();
                let note = if levels.is_empty() {
                    template::fill(&text.effort_unavailable, &[("model", &model.id)])
                } else {
                    template::fill(
                        &text.effort_unsupported,
                        &[
                            ("model", &model.id),
                            ("effort", id),
                            ("levels", &levels.join(", ")),
                        ],
                    )
                };
                self.feed.notice(&note, "warn");
                return;
            }
        }
        self.effort = effort;
        let resolved = self
            .model
            .as_ref()
            .and_then(|model| model.effort(self.effort.as_deref()));
        let note = template::fill(
            &text.effort_set,
            &[("effort", resolved.unwrap_or(&text.effort_default))],
        );
        self.feed.notice(&note, "done");
    }

    fn open_effort_picker(&mut self) {
        self.effort_picker = self.model.as_ref().and_then(|model| {
            (!model.effort_levels().is_empty())
                .then(|| effort_picker::Picker::new(model, self.effort.as_deref()))
        });
    }

    fn replace_model(&mut self, model: Model) {
        if self
            .effort
            .as_deref()
            .is_some_and(|id| !model.supports_effort(id))
        {
            self.effort = None;
        }
        self.system = system_prompt::build(&self.app.paths, &model);
        self.model = Some(model);
    }

    /// Uses `id` from the next turn on, for this session only.
    fn switch_model(&mut self, id: &str) {
        let Some(model) = self.catalog.get(id).cloned() else {
            return;
        };
        self.replace_model(model);
        let text = template::fill(&ui::get().picker.switched, &[("model", id)]);
        self.feed.notice(&text, "done");
        if self
            .model
            .as_ref()
            .is_some_and(|m| !m.capabilities.tool_calling)
        {
            self.feed.notice(&ui::get().text.chat_only, "muted");
        }
    }

    fn login_next(&mut self, next: login::Next) -> Option<login::Job> {
        match next {
            login::Next::Stay => None,
            login::Next::Run(job) => Some(job),
            login::Next::Error(error) => {
                self.feed.notice(&error, "error");
                None
            }
            login::Next::Close(message) => {
                self.login = None;
                if let Some(message) = message {
                    self.feed.notice(&message, "done");
                }
                None
            }
        }
    }

    fn action(&mut self, action: Action, running: bool) -> Step<'a> {
        match action {
            Action::Submit => return self.submit(running),
            // A command is never steering: it would reach the model as text.
            Action::Tab if running && self.is_command() => return self.submit(running),
            Action::Tab if running => self.enqueue(Kind::Steering),
            Action::Tab => self.complete(),
            Action::CycleMode => self.cycle_mode(),
            Action::Escape if running => self.interrupt(),
            Action::Escape => {}
            Action::CtrlC => {
                if !self.editor.is_empty() {
                    self.editor.clear();
                } else if running {
                    self.interrupt();
                } else {
                    let now = clock::instant();
                    let window = limits::get().tui.quit_window_ms.ms();
                    if self
                        .last_ctrl_c
                        .is_some_and(|t| now.duration_since(t) <= window)
                    {
                        return Step::Quit;
                    }
                    self.last_ctrl_c = Some(now);
                }
            }
            Action::Quit if self.editor.is_empty() => return Step::Quit,
            Action::Quit => {}
            Action::ToggleReasoning => self.feed.toggle_all(),
            editing => {
                self.editor.apply(editing);
            }
        }
        Step::Continue
    }

    fn submit(&mut self, running: bool) -> Step<'a> {
        let text = self.editor.text();
        // A trailing backslash asks for a new line, for terminals that cannot send Shift+Enter.
        if let Some(stripped) = text.strip_suffix('\\') {
            self.editor.set_text(&format!("{stripped}\n"));
            return Step::Continue;
        }
        if text.trim().is_empty() {
            return Step::Continue;
        }
        let prefix = Scope::Session.prefix();
        let escape = prefix.repeat(2);
        // A doubled prefix sends the line as written, less one `/`: `//etc/hosts is odd`.
        if text.trim_start().starts_with(&escape) {
            self.editor.set_text(&text.replacen(&escape, prefix, 1));
        } else if let Some(line) = text.trim().strip_prefix(prefix) {
            // Checked before a run gets the text: a command queued as a message would send
            // its arguments (a `--key`) to the model.
            let line = line.to_owned();
            let shown = Scope::Session.invoke(&commands::redact_line(&line));
            self.editor.take_remembering(shown);
            return self.command(&line);
        }
        if running {
            self.enqueue(Kind::Queued);
            return Step::Continue;
        }
        let text = self.editor.take();
        self.feed.user(&text);
        Step::Send(text)
    }

    /// Hands typed text to the run: steering lands after the current tools, a queued message
    /// when the run would otherwise stop.
    fn enqueue(&mut self, kind: Kind) {
        let text = self.editor.take();
        if text.trim().is_empty() {
            return;
        }
        let parts = vec![Part::Text { text: text.clone() }];
        match kind {
            Kind::Steering => self.shared.steer(parts),
            Kind::Queued => self.shared.follow_up(parts),
        }
        self.queue.push((kind, text));
    }

    /// Stops the run and puts anything still queued back in the editor.
    fn interrupt(&mut self) {
        self.shared.interrupt();
        let mut pulled: Vec<String> = self
            .shared
            .drain_steer()
            .into_iter()
            .chain(self.shared.drain_follow())
            .map(|parts| text_of(&parts))
            .collect();
        self.queue.clear();
        if !pulled.is_empty() {
            if !self.editor.is_empty() {
                pulled.push(self.editor.text());
            }
            self.editor.set_text(&pulled.join("\n"));
        }
    }

    fn cycle_mode(&mut self) {
        let modes = mode::all();
        let current = self.shared.mode();
        let at = modes.iter().position(|m| m.id == current.id).unwrap_or(0);
        self.shared.set_mode(&modes[(at + 1) % modes.len()]);
    }

    /// Completes a slash command name when only one fits.
    fn complete(&mut self) {
        let text = self.editor.text();
        if let [only] = palette::matches(&text).as_slice() {
            self.editor
                .set_text(&Scope::Session.invoke(&format!("{} ", only.name)));
        }
    }

    fn is_command(&self) -> bool {
        self.editor
            .text()
            .trim_start()
            .starts_with(Scope::Session.prefix())
    }

    fn command(&mut self, line: &str) -> Step<'a> {
        match command::start(self.app, line, self.session()) {
            Ok(started) => {
                self.feed.user(&started.echo);
                Step::Work(started.run)
            }
            Err(note) => {
                if !note.is_empty() {
                    self.feed.notice(&note, "warn");
                }
                Step::Continue
            }
        }
    }

    /// Applies what a command finished with.
    fn finish(&mut self, done: Work) -> Step<'a> {
        let outcome: CommandOutcome = match done {
            Work::Status {
                id,
                revision,
                me,
                usage,
            } => {
                if revision == self.app.api.key_revision()
                    && let Some(card) = &mut self.dashboard
                    && card.id == id
                {
                    card.finished(me, usage);
                }
                return Step::Continue;
            }
            Work::Account { revision, .. } if revision != self.app.api.key_revision() => {
                return Step::Continue;
            }
            Work::Account { result: Ok(me), .. } => {
                self.feed
                    .notice(&crate::commands::login::account_status(&me), "done");
                return Step::Continue;
            }
            Work::Account {
                result: Err(error), ..
            } => {
                self.feed.notice(&error.to_string(), "warn");
                if error.info.status == Some(401)
                    && crate::auth::from_env().is_none()
                    && self.app.api.has_key()
                    && self.login.is_none()
                {
                    self.app.api.set_key(None);
                    self.login = Some(Login::starting());
                    return self.login_job(login::Job::Start);
                }
                return Step::Continue;
            }
            Work::Catalog(catalog) => {
                self.catalog = catalog;
                self.effort_picker = None;
                if self.catalog.models.is_empty() {
                    self.feed.notice(&models_note(&self.catalog), "warn");
                    return Step::Continue;
                }
                if self.model.is_none() {
                    match self.catalog.selected(&self.app.settings.model) {
                        Ok(model) => {
                            let id = model.id.clone();
                            self.switch_model(&id);
                        }
                        Err(error) => self.feed.notice(&error.to_string(), "warn"),
                    }
                    return Step::Continue;
                }
                if let Some(model) = self
                    .model
                    .as_ref()
                    .and_then(|model| self.catalog.get(&model.id))
                    .cloned()
                {
                    self.replace_model(model);
                }
                self.picker = Some(Picker::new(
                    &self.catalog,
                    self.model.as_ref().map_or("", |m| m.id.as_str()),
                ));
                return Step::Continue;
            }
            Work::Command(Err(e)) => {
                self.feed.notice(&format!("{e:#}"), "error");
                return Step::Continue;
            }
            Work::Command(Ok(outcome)) => outcome,
        };
        if !outcome.text.is_empty() {
            self.feed.markdown(&outcome.text);
        }
        for effect in outcome.effects {
            match effect {
                Effect::Quit => return Step::Quit,
                Effect::CycleMode => self.cycle_mode(),
                Effect::SetMode(id) => {
                    if let Some(mode) = mode::get(&id) {
                        self.shared.set_mode(mode);
                    }
                }
                Effect::Login { number: None } => self.login = Some(Login::new()),
                Effect::Login {
                    number: Some(number),
                } => {
                    self.login = Some(Login::checking());
                    return self.login_job(login::Job::Check(number));
                }
                Effect::PickModel => {
                    return Step::Work(command::refresh_catalog(self.app));
                }
                Effect::ShowStatus => return self.refresh_status(),
                Effect::Forums { login } => match crate::forums::store::load(&self.app.paths) {
                    Ok(list) => self.forums = Some(Forums::new(list, login.as_deref())),
                    Err(e) => self.feed.notice(&format!("{e:#}"), "error"),
                },
                Effect::NewConversation => return Step::NewConversation,
                Effect::SetEffort(id) => self.set_effort(id),
                Effect::PickEffort => match &self.model {
                    Some(model) if !model.effort_levels().is_empty() => {
                        self.open_effort_picker();
                    }
                    Some(model) => self.feed.notice(
                        &template::fill(
                            &ui::get().text.effort_unavailable,
                            &[("model", &model.id)],
                        ),
                        "muted",
                    ),
                    None => self.feed.notice(&models_note(&self.catalog), "warn"),
                },
                Effect::Send(text) => return Step::Send(text),
                Effect::CopyLastReply => match self.feed.last_reply() {
                    Some(text) => self.copy(&text),
                    None => self.feed.notice(&ui::get().text.nothing_to_copy, "muted"),
                },
            }
        }
        Step::Continue
    }

    fn agent(&mut self, event: AgentEvent) {
        let now = clock::instant();
        match &event {
            AgentEvent::MessageEnd { message } => {
                if let Some(usage) = message.usage {
                    self.cost_micros += usage.cost_micros;
                    self.context_tokens = Some(usage.input);
                    self.session_requests += 1;
                    self.session_usage.input = self.session_usage.input.saturating_add(usage.input);
                    self.session_usage.output =
                        self.session_usage.output.saturating_add(usage.output);
                    self.session_usage.cached =
                        self.session_usage.cached.saturating_add(usage.cached);
                    self.session_usage.reasoning =
                        self.session_usage.reasoning.saturating_add(usage.reasoning);
                    self.session_usage.cost_micros = self
                        .session_usage
                        .cost_micros
                        .saturating_add(usage.cost_micros);
                }
            }
            AgentEvent::Delivered { text } => {
                if let Some(i) = self.queue.iter().position(|(_, t)| t == text) {
                    self.queue.remove(i);
                }
            }
            AgentEvent::Prompt { id, prompt, .. } => {
                self.cards
                    .push_back(card::from_prompt(*id, prompt, self.draw_width()));
            }
            _ => {}
        }
        if let Some(progress) = &mut self.progress {
            progress.event(&event);
        }
        self.feed.event(&event, now);
    }

    fn draw(&mut self, running: bool) {
        let now = clock::instant();
        let (pad, width) = layout::margins(self.screen.width());
        let height = self.screen.height();
        let header_rows = usize::from(height > 2) * 2;
        let body_height = height.saturating_sub(header_rows);
        let mode = self.shared.mode();
        // The bar, pinned to the bottom of the screen.
        let mut live = Vec::new();
        if running {
            live.push(self.status(now, width));
        }
        live.extend(queue::render(&self.queue, width));
        if let Some(at) = self.last_ctrl_c
            && now.duration_since(at) <= limits::get().tui.quit_window_ms.ms()
            && !running
        {
            live.push(Line::styled(
                &ui::get().text.ctrl_c_again,
                Style::fg("muted"),
            ));
        }
        // Prompts answered elsewhere (AUTO approved them, the run was interrupted) go away.
        let shared = self.shared;
        self.cards.retain(|card| shared.waiting(card.id()));
        // The command popup floats over the conversation instead of growing the bar.
        let mut floating = Vec::new();
        let session_info = self.session();
        let card = if let Some(login) = &self.login {
            Some(login.render(width))
        } else if let Some(forums) = &self.forums {
            Some(forums.render(width))
        } else if let Some(picker) = &self.picker {
            Some(picker.render(width))
        } else if let Some(picker) = &self.effort_picker {
            Some(picker.render(width))
        } else if let Some(card) = &mut self.dashboard {
            Some(card.render(
                &dashboard::Info {
                    session: &session_info,
                    usage: self.session_usage,
                    requests: self.session_requests,
                    project: &self.app.paths.project.display().to_string(),
                    configured: self.app.api.is_configured(),
                    logged_in: self.app.api.has_key()
                        && card.revision == self.app.api.key_revision(),
                },
                width,
                height.saturating_sub(live.len() + 3).max(5),
            ))
        } else {
            self.cards.front().map(|card| card.render(width))
        };
        match card {
            Some(lines) => live.extend(lines),
            None => {
                live.push(frame::top(mode, width));
                if !running {
                    let found = self.palette.open(&self.editor.text());
                    if !found.is_empty() {
                        floating = self.palette.render(&found, width);
                    }
                }
                let ui = ui::get();
                let prompt = Line::styled(&ui.prompt, Style::fg(&mode.color).bold());
                let rows = (height * limits::get().tui.editor_max_rows_pct.value / 100).max(3);
                live.extend(self.editor.render(width, rows, &prompt, &ui.placeholder));
            }
        }
        live.push(frame::bottom(
            mode,
            &frame::Info {
                model: self
                    .model
                    .as_ref()
                    .map_or(crate::backend::text().disconnected.as_str(), |m| {
                        m.id.as_str()
                    }),
                effort: self
                    .model
                    .as_ref()
                    .and_then(|m| m.effort(self.effort.as_deref())),
                context_pct: self.context_pct(),
                cost_micros: self.cost_micros,
            },
            width,
        ));
        let mut frame = layout::compose(
            &mut self.feed,
            &mut self.view,
            &live,
            &floating,
            (width, body_height),
            now,
        );
        if header_rows > 0 {
            frame.rows.insert(0, Line::default());
            frame.rows.insert(
                0,
                crate::tui::header::render(&self.app.paths.project, width),
            );
            frame.blocks.insert(0, None);
            frame.blocks.insert(0, None);
        }
        self.blocks = frame.blocks;
        if self.toast.as_ref().is_some_and(|t| !t.shown(now)) {
            self.toast = None;
        }
        if let Some(toast) = &self.toast {
            toast.overlay(&mut frame.rows, width);
        }
        layout::inset(&mut frame.rows, pad);
        let bytes = self.screen.frame(&frame.rows);
        if !bytes.is_empty() {
            term::out(&bytes);
        }
    }

    /// The columns a frame is drawn in, inside the side margins.
    fn draw_width(&self) -> usize {
        layout::margins(self.screen.width()).1
    }

    fn status(&self, now: Instant, width: usize) -> Line {
        if let Some((secs, why)) = self.feed.retry(now) {
            return status::retrying(secs, why, width);
        }
        let color = &self.shared.mode().color;
        self.progress
            .as_ref()
            .map(|p| p.render(now, color, self.braille, width))
            .unwrap_or_default()
    }
}

fn models_note(catalog: &Catalog) -> String {
    crate::commands::models::source_note(catalog)
}

fn text_of(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod account_tests {
    use super::*;
    use crate::api::error::ErrorInfo;

    #[test]
    fn late_dashboard_results_cannot_replace_a_newer_card_or_account() {
        let home = tempfile::tempdir().unwrap();
        let app = App::for_tests(home.path());
        app.api.set_key(Some("synthetic-old-key".into()));
        let revision = app.api.key_revision();
        let shared = Shared::new(mode::get(&app.settings.mode).unwrap());
        let catalog = Catalog {
            models: Vec::new(),
            source: models::Source::Unavailable,
            fetched_at: None,
            note: None,
        };
        let mut tui = Tui::new(&app, catalog, None, &shared, None, String::new());
        tui.dashboard = Some(Dashboard::new(2, revision, true));
        let done = |id| Work::Status {
            id,
            revision,
            me: Ok(account::Me {
                account_hint: "old-account".into(),
                ..Default::default()
            }),
            usage: Ok(account::Overview::default()),
        };
        tui.finish(done(1));
        app.api.set_key(Some("synthetic-new-key".into()));
        tui.finish(done(2));
        let session = tui.session();
        let card = tui.dashboard.as_mut().unwrap();
        let rows = card.render(
            &dashboard::Info {
                session: &session,
                usage: Usage::default(),
                requests: 0,
                project: "/project",
                configured: true,
                logged_in: true,
            },
            100,
            30,
        );
        assert!(!rows.iter().any(|line| line.text().contains("old-account")));
    }

    #[test]
    fn late_account_checks_cannot_replace_a_newer_login_or_logout() {
        let home = tempfile::tempdir().unwrap();
        let app = App::for_tests(home.path());
        app.api.set_key(Some("synthetic-old-key".into()));
        let revision = app.api.key_revision();
        let shared = Shared::new(mode::get(&app.settings.mode).unwrap());
        let catalog = Catalog {
            models: Vec::new(),
            source: models::Source::Unavailable,
            fetched_at: None,
            note: None,
        };
        let mut tui = Tui::new(&app, catalog, None, &shared, None, String::new());
        app.api.clone().set_key(Some("synthetic-new-key".into()));
        let expired = ErrorInfo {
            status: Some(401),
            ..ErrorInfo::local("invalid_key", "expired old credential")
        };
        assert!(matches!(
            tui.finish(Work::Account {
                revision,
                result: Err(expired.into())
            }),
            Step::Continue
        ));
        assert!(app.api.has_key() && tui.login.is_none());
        assert!(tui.feed.copy_text(1).is_none());
        let current = app.api.key_revision();
        let me = || account::Me {
            account_hint: "2222".into(),
            ..Default::default()
        };
        tui.finish(Work::Account {
            revision,
            result: Ok(me()),
        });
        assert!(tui.feed.copy_text(1).is_none());
        tui.finish(Work::Account {
            revision: current,
            result: Ok(me()),
        });
        assert!(tui.feed.copy_text(1).unwrap().contains("2222"));
        app.api.set_key(None);
        tui.finish(Work::Account {
            revision: current,
            result: Ok(me()),
        });
        assert!(!app.api.has_key() && tui.login.is_none());
        assert!(tui.feed.copy_text(2).is_none());
    }
}

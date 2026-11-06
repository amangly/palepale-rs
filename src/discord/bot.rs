//! Isolated Discord conversations using the native paid chat path, without local tools.
use super::{Config, Incoming, chunks, data, strip_mention};
use crate::{
    agent::message::{Assistant, Finish, Message, Part, Usage},
    api::{
        Api,
        assemble::Delta,
        chat::{self, Event, Turn},
        models::{Catalog, Model},
    },
    io::{
        self,
        discord::{Destination, Discord},
    },
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{Mutex, mpsc, watch};
use tokio_util::sync::CancellationToken;

pub struct Bot {
    api: Api,
    discord: Discord,
    config: Config,
    catalog: Catalog,
    bot_id: String,
    state: Mutex<State>,
    cancel: CancellationToken,
}
#[derive(Default)]
struct State {
    sessions: HashMap<String, Session>,
    seen: HashSet<String>,
    recent: VecDeque<String>,
    touch: u64,
}
struct Session {
    model: String,
    messages: Vec<Message>,
    usage: Option<Usage>,
    active: Option<CancellationToken>,
    touched: u64,
}

impl State {
    fn make_room(&mut self, key: &str, limit: usize) -> bool {
        if self.sessions.contains_key(key) || self.sessions.len() < limit {
            return true;
        }
        let oldest = self
            .sessions
            .iter()
            .filter(|(_, s)| s.active.is_none())
            .min_by_key(|(_, s)| s.touched)
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            self.sessions.remove(&key);
            true
        } else {
            false
        }
    }
    fn touch(&mut self) -> u64 {
        self.touch = self.touch.saturating_add(1);
        self.touch
    }
}

impl Bot {
    pub fn new(
        api: Api,
        discord: Discord,
        config: Config,
        catalog: Catalog,
        bot_id: String,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            api,
            discord,
            config,
            catalog,
            bot_id,
            state: Mutex::new(State::default()),
            cancel,
        }
    }

    pub async fn serve(self: Arc<Self>, mut events: mpsc::Receiver<Incoming>) {
        let mut jobs = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                biased;
                () = self.cancel.cancelled() => break,
                result = jobs.join_next(), if !jobs.is_empty() => {
                    if result.is_some_and(|r| r.is_err()) { io::term::err("Discord task failed.\n"); }
                }
                event = events.recv(), if jobs.len() < data().event_queue => {
                    let Some(event) = event else { break; };
                    let bot = self.clone();
                    jobs.spawn(async move {
                        if bot.handle(event).await.is_err() { io::term::err(&format!("{}\n", data().text.delivery_failed)); }
                    });
                }
            }
        }
        jobs.abort_all();
        while jobs.join_next().await.is_some() {}
    }

    pub async fn handle(&self, mut event: Incoming) -> anyhow::Result<()> {
        if !self.config.authorized(&event) {
            if let Some(interaction) = &event.interaction {
                self.discord
                    .ack(interaction, Some(&data().text.denied))
                    .await?;
            }
            return Ok(());
        }
        {
            let mut state = self.state.lock().await;
            if !state.seen.insert(event.id.clone()) {
                return Ok(());
            }
            state.recent.push_back(event.id.clone());
            if state.recent.len() > data().dedup_events
                && let Some(id) = state.recent.pop_front()
            {
                state.seen.remove(&id);
            }
        }
        // Only authorized events reach channel lookup. Bot-owned threads accept follow-ups.
        let mut channel = None;
        if event.interaction.is_none() && event.guild.is_some() {
            let direct =
                event.mentioned || self.config.free_response_channels.contains(&event.channel);
            if self.config.auto_thread || !direct {
                channel = Some(self.discord.channel(&event.channel).await?);
            }
            let own_thread = channel.as_ref().is_some_and(|ch| {
                matches!(ch["type"].as_u64(), Some(10..=12)) && ch["owner_id"] == self.bot_id
            });
            if !direct && !own_thread {
                return Ok(());
            }
        }
        let content = strip_mention(&event.content, &self.bot_id);
        let command = event.command.clone().or_else(|| {
            let (name, arg) = content
                .split_once(char::is_whitespace)
                .unwrap_or((&content, ""));
            ["help", "model", "reset", "stop", "usage"]
                .iter()
                .find(|cmd| name == format!("/{cmd}"))
                .map(|cmd| ((*cmd).to_owned(), arg.trim().to_owned()))
        });
        if let Some((command, argument)) = command
            && command != "ask"
        {
            let reply = self.control(&event.key(), &command, &argument).await;
            return self.respond(&event, &reply).await;
        }
        if content.is_empty() || event.attachments || content.len() > self.config.history_bytes {
            return self.respond(&event, &data().text.empty).await;
        }
        if event.interaction.is_none()
            && self.config.auto_thread
            && channel
                .as_ref()
                .is_some_and(|ch| matches!(ch["type"].as_u64(), Some(0 | 5)))
        {
            // If thread creation is ambiguous, do not fall back and duplicate the request.
            event.channel = self.discord.thread(&event.channel, &event.id).await?;
        }
        let key = event.key();
        let (model, mut history, cancel) = match self.begin(&key).await {
            Ok(turn) => turn,
            Err(message) => return self.respond(&event, message).await,
        };
        history.push(Message::User {
            parts: vec![Part::Text { text: content }],
        });
        let result = self.reply(&mut event, &model, &history, &cancel).await;
        let mut state = self.state.lock().await;
        if let Some(session) = state.sessions.get_mut(&key) {
            session.active = None;
            if let Ok(reply) = &result {
                if let Some(usage) = reply.usage {
                    let total = session.usage.get_or_insert_default();
                    total.input = total.input.saturating_add(usage.input);
                    total.output = total.output.saturating_add(usage.output);
                    total.cached = total.cached.saturating_add(usage.cached);
                    total.reasoning = total.reasoning.saturating_add(usage.reasoning);
                }
                if reply.finish == Finish::Done && reply.tool_calls().next().is_none() {
                    history.push(Message::Assistant(reply.clone()));
                    prune(
                        &mut history,
                        self.config.history_turns,
                        self.config.history_bytes,
                    );
                    session.messages = history;
                }
            }
        }
        result.map(|_| ())
    }

    async fn begin(
        &self,
        key: &str,
    ) -> Result<(Model, Vec<Message>, CancellationToken), &'static str> {
        let mut state = self.state.lock().await;
        if state.sessions.get(key).is_some_and(|s| s.active.is_some()) {
            return Err(&data().text.busy);
        }
        if state
            .sessions
            .values()
            .filter(|s| s.active.is_some())
            .count()
            >= self.config.max_concurrent
        {
            return Err(&data().text.capacity);
        }
        if !state.make_room(key, self.config.max_sessions) {
            return Err(&data().text.capacity);
        }
        let touched = state.touch();
        let session = state.sessions.entry(key.into()).or_insert_with(|| Session {
            model: self.config.model.clone(),
            messages: Vec::new(),
            usage: None,
            active: None,
            touched,
        });
        session.touched = touched;
        let model = self
            .catalog
            .get(&session.model)
            .ok_or(data().text.failed.as_str())?
            .clone();
        let cancel = self.cancel.child_token();
        session.active = Some(cancel.clone());
        Ok((model, session.messages.clone(), cancel))
    }

    async fn control(&self, key: &str, command: &str, argument: &str) -> String {
        let mut state = self.state.lock().await;
        match command {
            "reset" => {
                if state.sessions.get(key).is_some_and(|s| s.active.is_some()) {
                    return data().text.busy.clone();
                }
                state.sessions.remove(key);
                data().text.reset.clone()
            }
            "stop" => {
                if let Some(cancel) = state.sessions.get(key).and_then(|s| s.active.as_ref()) {
                    cancel.cancel();
                    data().text.stopping.clone()
                } else {
                    data().text.idle.clone()
                }
            }
            "usage" => {
                if let Some(usage) = state.sessions.get(key).and_then(|s| s.usage) {
                    format!(
                        "Session tokens: {} input · {} output · {} cached. Check Palepale Usage for the final charge.",
                        usage.input, usage.output, usage.cached
                    )
                } else {
                    data().text.no_usage.clone()
                }
            }
            "model" => {
                if argument.is_empty() {
                    let current = state
                        .sessions
                        .get(key)
                        .map_or(&self.config.model, |s| &s.model);
                    let models = self
                        .catalog
                        .models
                        .iter()
                        .filter(|m| m.available && m.capabilities.streaming)
                        .map(|m| m.id.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    format!("Model: {current}\n\n{models}")
                } else if let Some(model) = self
                    .catalog
                    .get(argument)
                    .filter(|m| m.capabilities.streaming)
                {
                    if state.sessions.get(key).is_some_and(|s| s.active.is_some()) {
                        return data().text.busy.clone();
                    }
                    if !state.make_room(key, self.config.max_sessions) {
                        return data().text.capacity.clone();
                    }
                    let touched = state.touch();
                    state.sessions.insert(
                        key.into(),
                        Session {
                            model: model.id.clone(),
                            messages: Vec::new(),
                            usage: None,
                            active: None,
                            touched,
                        },
                    );
                    format!("Model: {}. New conversation started.", model.name())
                } else {
                    "Unknown model. Use /palepale model to list models.".into()
                }
            }
            _ => data().text.help.clone(),
        }
    }

    async fn respond(&self, event: &Incoming, text: &str) -> anyhow::Result<()> {
        let text = chunks(text, data().message_units)
            .into_iter()
            .next()
            .unwrap_or_default();
        if let Some(interaction) = &event.interaction {
            self.discord.ack(interaction, Some(&text)).await
        } else {
            self.discord.say(&event.channel, &text).await.map(|_| ())
        }
    }

    async fn reply(
        &self,
        event: &mut Incoming,
        model: &Model,
        history: &[Message],
        cancel: &CancellationToken,
    ) -> anyhow::Result<Assistant> {
        if let Some(interaction) = &event.interaction {
            self.discord.ack(interaction, None).await?;
        }
        let mut destination = Destination::new(event.channel.clone(), event.interaction.take());
        self.discord
            .display(&mut destination, &data().text.thinking)
            .await?;
        // Reuse native validation, single dispatch, run budgets, reasoning replay and SSE errors.
        let (updates, mut changes) = watch::channel(String::new());
        let turn = Turn {
            model,
            agent_profile: None,
            effort: None,
            system: &data().system,
            messages: history,
            tools: &[],
        };
        let mut bytes = 0usize;
        let mut visible = String::new();
        let mut display_units = 0usize;
        let mut clipped = false;
        let mut on_delta = |event: Event<'_>| {
            let Event::Delta(delta) = event;
            let text = match delta {
                Delta::Text(text) | Delta::Reasoning(text) => text,
                Delta::ToolCall { arguments, .. } => arguments,
            };
            bytes = bytes.saturating_add(text.len());
            if bytes > data().max_reply_bytes {
                clipped = true;
                cancel.cancel();
                return;
            }
            if let Delta::Text(text) = delta {
                for ch in text.chars() {
                    if display_units + ch.len_utf16() > data().max_display_units {
                        clipped = true;
                        cancel.cancel();
                        break;
                    }
                    visible.push(ch);
                    display_units += ch.len_utf16();
                }
                updates.send_replace(visible.clone());
            }
        };
        let output_limit =
            (self.config.max_output_tokens > 0).then_some(self.config.max_output_tokens);
        let mut inference = Box::pin(chat::stream_with_limit(
            &self.api,
            &turn,
            cancel,
            &mut on_delta,
            output_limit,
        ));
        let mut next_update = Box::pin(io::clock::sleep(Duration::from_millis(
            data().stream_update_ms,
        )));
        let mut delivery_failed = false;
        let mut assistant = loop {
            tokio::select! {
                biased;
                result = &mut inference => break result,
                () = &mut next_update => {
                    if changes.has_changed().unwrap_or(false) {
                        let text = changes.borrow_and_update().clone();
                        if !text.is_empty() && self.discord.display(&mut destination, &text).await.is_err() {
                            // Cancellation drains the existing turn. Never resubmit it to fix delivery.
                            delivery_failed = true; cancel.cancel();
                        }
                    }
                    next_update = Box::pin(io::clock::sleep(Duration::from_millis(data().stream_update_ms)));
                }
            }
        };
        // Release the callback's mutable borrows before reading its final counters.
        drop(inference);
        if clipped {
            assistant.finish = Finish::Length;
        }
        if delivery_failed {
            io::term::err(&format!("{}\n", data().text.delivery_failed));
            return Ok(assistant);
        }
        let text = changes.borrow().clone();
        let warning = if clipped {
            Some(data().text.limit.as_str())
        } else {
            match assistant.finish {
                Finish::Aborted => Some(data().text.stopped.as_str()),
                Finish::Length => Some(data().text.limit.as_str()),
                Finish::Error => Some(data().text.failed.as_str()),
                Finish::ToolCalls => Some(data().text.unsupported.as_str()),
                Finish::Done if assistant.tool_calls().next().is_some() => {
                    Some(data().text.unsupported.as_str())
                }
                _ => None,
            }
        };
        if warning == Some(data().text.unsupported.as_str()) {
            assistant.finish = Finish::ToolCalls;
        }
        let mut display = text;
        if let Some(warning) = warning {
            if !display.is_empty() {
                display.push_str("\n\n");
            }
            display.push_str(warning);
        }
        if display.is_empty() {
            display = data().text.no_reply.clone();
        }
        if self
            .discord
            .display(&mut destination, &display)
            .await
            .is_err()
        {
            io::term::err(&format!("{}\n", data().text.delivery_failed));
        }
        Ok(assistant)
    }
}

fn prune(messages: &mut Vec<Message>, turns: usize, bytes: usize) {
    while messages.len() > turns * 2
        || serde_json::to_vec(messages).map_or(usize::MAX, |v| v.len()) > bytes
    {
        if messages.is_empty() {
            break;
        }
        messages.drain(..2.min(messages.len()));
    }
}

#[cfg(test)]
mod tests;

//! Session state a run shares with whoever drives it: queued messages, pending prompts, the
//! mode, and the interrupt. Frontends change it while a run is live.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::agent::message::Part;
use crate::agent::prompt::Reply;
use crate::mode::Mode;
use crate::permission::rule::Action;
use crate::tools::files::Files;

/// A mode change the model has to hear about.
pub enum Reminder {
    /// A mode with instructions of its own.
    Entered(&'static Mode),
    /// Out of a mode with instructions, into one without, so they no longer hold.
    Left {
        now: &'static Mode,
    },
    Restored {
        now: &'static Mode,
    },
}

type PendingAsks = HashMap<u64, (bool, oneshot::Sender<Reply>)>;

pub struct Shared {
    pub depth: usize,
    pub session: Mutex<std::path::PathBuf>,
    pub slots: Arc<tokio::sync::Semaphore>,
    inherited_cancel: Option<CancellationToken>,
    cancel: Mutex<CancellationToken>,
    steer: Mutex<VecDeque<Vec<Part>>>,
    follow: Mutex<VecDeque<Vec<Part>>>,
    /// Waiting prompts, and whether each is a permission prompt.
    asks: Arc<Mutex<PendingAsks>>,
    next_ask: Arc<AtomicU64>,
    mode: Arc<Mutex<&'static Mode>>,
    agent_profile: Arc<Mutex<Option<String>>>,
    /// The mode the model was last told about.
    reminded: Mutex<Option<&'static Mode>>,
    restored: AtomicBool,
    cwd: Mutex<Option<std::path::PathBuf>>,
    oriented: AtomicBool,
    pub files: Files,
}

impl Shared {
    pub fn new(mode: &'static Mode) -> Self {
        Self {
            depth: 0,
            session: Mutex::default(),
            slots: Arc::new(tokio::sync::Semaphore::new(
                crate::limits::get().subagent.parallel.value,
            )),
            inherited_cancel: None,
            cancel: Mutex::new(CancellationToken::new()),
            steer: Mutex::default(),
            follow: Mutex::default(),
            asks: Arc::default(),
            next_ask: Arc::new(AtomicU64::new(1)),
            mode: Arc::new(Mutex::new(mode)),
            agent_profile: Arc::default(),
            reminded: Mutex::default(),
            restored: AtomicBool::new(false),
            cwd: Mutex::default(),
            oriented: AtomicBool::new(false),
            files: Files::default(),
        }
    }

    /// A fresh interrupt for a new run.
    pub fn begin_run(&self) -> CancellationToken {
        let token = self.inherited_cancel.clone().unwrap_or_default();
        *self.cancel.lock().unwrap() = token.clone();
        token
    }

    pub fn child(&self, cancel: CancellationToken) -> Self {
        let mut child = Self::new(self.mode());
        child.depth = self.depth + 1;
        child.inherited_cancel = Some(cancel.child_token());
        child.asks = self.asks.clone();
        child.next_ask = self.next_ask.clone();
        child.mode = self.mode.clone();
        child.agent_profile = self.agent_profile.clone();
        child.slots = self.slots.clone();
        child.files = self.files.child();
        *child.cwd.lock().unwrap() = self.cwd();
        child.oriented.store(true, Ordering::Release);
        child
    }

    pub fn interrupt(&self) {
        self.cancel.lock().unwrap().cancel();
    }

    pub fn mode(&self) -> &'static Mode {
        *self.mode.lock().unwrap()
    }

    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        self.cwd.lock().unwrap().clone()
    }

    pub fn set_cwd(&self, dir: std::path::PathBuf) {
        *self.cwd.lock().unwrap() = Some(dir);
    }

    pub fn orient_once(&self) -> bool {
        !self.oriented.swap(true, Ordering::AcqRel)
    }

    pub fn agent_profile(&self) -> Option<String> {
        self.agent_profile.lock().unwrap().clone()
    }

    pub fn set_agent_profile(&self, id: Option<String>) {
        *self.agent_profile.lock().unwrap() = id;
    }

    /// Switching into a mode that allows everything also approves permission prompts already
    /// waiting (questions still wait for their answer).
    pub fn set_mode(&self, mode: &'static Mode) {
        *self.mode.lock().unwrap() = mode;
        if mode.verdicts.ask == Action::Allow {
            let mut asks = self.asks.lock().unwrap();
            let permissions: Vec<u64> = asks
                .iter()
                .filter(|(_, (permission, _))| *permission)
                .map(|(id, _)| *id)
                .collect();
            for id in permissions {
                if let Some((_, waiting)) = asks.remove(&id) {
                    let _ = waiting.send(Reply::Yes);
                }
            }
        }
    }

    /// Drops a prompt nobody will answer now (the run was interrupted).
    pub fn forget(&self, id: u64) {
        self.asks.lock().unwrap().remove(&id);
    }

    /// Whether prompt `id` is still waiting for an answer.
    pub fn waiting(&self, id: u64) -> bool {
        self.asks.lock().unwrap().contains_key(&id)
    }

    /// What to tell the model about the mode now, if it changed since the model was last told.
    pub fn take_reminder(&self) -> Option<Reminder> {
        let mode = self.mode();
        let mut reminded = self.reminded.lock().unwrap();
        if self.restored.swap(false, Ordering::AcqRel) {
            *reminded = Some(mode);
            return Some(if mode.reminder.is_some() {
                Reminder::Entered(mode)
            } else {
                Reminder::Restored { now: mode }
            });
        }
        if reminded.is_some_and(|m| m.id == mode.id) {
            return (self.depth == 0 && mode.repeat_reminder).then_some(Reminder::Entered(mode));
        }
        let left = reminded.replace(mode);
        if mode.reminder.is_some() {
            Some(Reminder::Entered(mode))
        } else if left.is_some_and(|m| m.reminder.is_some()) {
            Some(Reminder::Left { now: mode })
        } else {
            None
        }
    }

    /// A new conversation: the model has been told nothing, read nothing, and nothing waits.
    pub fn start_over(&self) {
        *self.cwd.lock().unwrap() = None;
        self.oriented.store(false, Ordering::Release);
        self.set_agent_profile(None);
        *self.reminded.lock().unwrap() = None;
        self.steer.lock().unwrap().clear();
        self.follow.lock().unwrap().clear();
        self.files.forget_all();
        self.restored.store(false, Ordering::Release);
    }

    pub fn resumed(&self) {
        self.files.forget_all();
        self.restored.store(true, Ordering::Release);
    }

    /// Delivered after the current tool calls finish.
    pub fn steer(&self, parts: Vec<Part>) {
        self.steer.lock().unwrap().push_back(parts);
    }

    /// Delivered when the run would otherwise stop.
    pub fn follow_up(&self, parts: Vec<Part>) {
        self.follow.lock().unwrap().push_back(parts);
    }

    pub fn drain_steer(&self) -> Vec<Vec<Part>> {
        self.steer.lock().unwrap().drain(..).collect()
    }

    pub fn drain_follow(&self) -> Vec<Vec<Part>> {
        self.follow.lock().unwrap().drain(..).collect()
    }

    pub fn register_ask(&self, permission: bool) -> (u64, oneshot::Receiver<Reply>) {
        let id = self.next_ask.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.asks.lock().unwrap().insert(id, (permission, tx));
        (id, rx)
    }

    /// `false` when that prompt is no longer waiting.
    pub fn answer(&self, id: u64, reply: Reply) -> bool {
        match self.asks.lock().unwrap().remove(&id) {
            Some((_, waiting)) => waiting.send(reply).is_ok(),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_reminders_repeat_but_other_modes_do_not() {
        let state = Shared::new(crate::mode::get("plan").unwrap());
        assert!(matches!(state.take_reminder(), Some(Reminder::Entered(_))));
        assert!(matches!(state.take_reminder(), Some(Reminder::Entered(_))));
        state.set_mode(crate::mode::get("auto").unwrap());
        assert!(matches!(state.take_reminder(), Some(Reminder::Left { .. })));
        assert!(state.take_reminder().is_none());
    }

    #[tokio::test]
    async fn child_prompts_share_ids_and_parent_interrupt_cancels_children() {
        let parent = Shared::new(crate::mode::get("manual").unwrap());
        let token = parent.begin_run();
        let child = parent.child(token.clone());
        let child_token = child.begin_run();
        let (parent_id, _) = parent.register_ask(true);
        let (child_id, reply) = child.register_ask(true);
        assert_ne!(parent_id, child_id);
        assert!(parent.answer(child_id, Reply::Yes));
        assert!(matches!(reply.await, Ok(Reply::Yes)));
        parent.interrupt();
        assert!(token.is_cancelled());
        assert!(child_token.is_cancelled());
    }
}

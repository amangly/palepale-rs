use std::path::{Path, PathBuf};

use crate::agent::message::Message;
use crate::session::store::{SavedState, Selection, Store};

/// The conversation in memory, mirrored to its session file when it has one.
pub struct Transcript {
    pub ids: Vec<String>,
    pub snapshots: std::collections::HashMap<String, super::entry::Snapshot>,
    pub compactions: Vec<super::entry::Compaction>,
    pub messages: Vec<Message>,
    store: Option<Store>,
    /// A write failed; the file stopped being written and no longer holds everything.
    unsaved: bool,
    pub state: SavedState,
    written_state: Option<SavedState>,
}

/// Why the session file stopped being written.
pub struct Unsaved {
    pub path: PathBuf,
    pub error: String,
}

impl Transcript {
    pub fn new(store: Option<Store>) -> Self {
        Self {
            ids: Vec::new(),
            snapshots: Default::default(),
            compactions: Vec::new(),
            messages: Vec::new(),
            store,
            unsaved: false,
            state: SavedState::default(),
            written_state: None,
        }
    }

    pub fn open(paths: &crate::paths::Paths, selection: &Selection) -> anyhow::Result<Self> {
        if matches!(selection, Selection::New) {
            return Ok(Self::new(Some(Store::create(paths))));
        }
        let (store, loaded) = Store::open(paths, selection)?;
        let mut transcript = Self {
            ids: loaded.ids,
            snapshots: loaded.snapshots,
            compactions: loaded.compactions,
            messages: loaded.messages,
            store: Some(store),
            unsaved: false,
            written_state: Some(loaded.state.clone()),
            state: loaded.state,
        };
        // Recovery records uncertain outcomes; it never calls a tool again.
        let mut pending = Vec::new();
        for message in &transcript.messages {
            match message {
                Message::Assistant(reply) => pending.extend(reply.tool_calls().cloned()),
                Message::Tool(result) => pending.retain(|call| call.id != result.call_id),
                _ => {}
            }
        }
        for call in pending {
            transcript
                .push(Message::Tool(crate::agent::message::ToolResult {
                    call_id: call.id,
                    name: call.name,
                    content: crate::session::text::get().interrupted.clone(),
                    attachments: Vec::new(),
                    is_error: true,
                    details: None,
                    asks: Vec::new(),
                    refused: Some("interrupted".into()),
                }))
                .map_err(|failure| anyhow::anyhow!(failure.error))?;
        }
        Ok(transcript)
    }

    pub fn save_state(&mut self, state: SavedState) -> Result<(), Unsaved> {
        self.state = state;
        if self.messages.is_empty()
            || self.unsaved
            || self.written_state.as_ref() == Some(&self.state)
        {
            return Ok(());
        }
        let result = match &mut self.store {
            Some(store) => store.save_state(&self.state).map_err(|error| Unsaved {
                path: store.path.clone(),
                error: format!("{error:#}"),
            }),
            None => Ok(()),
        };
        self.unsaved |= result.is_err();
        if result.is_ok() {
            self.written_state = Some(self.state.clone());
        }
        result
    }

    /// The session file, once it exists and while it holds the whole conversation.
    pub fn path(&self) -> Option<&Path> {
        self.store
            .as_ref()
            .filter(|s| s.written() && !self.unsaved)
            .map(|s| s.path.as_path())
    }

    /// Identifies the session in file names (plans); stable for a stored session.
    pub fn id(&self) -> String {
        self.store
            .as_ref()
            .map_or_else(|| "unsaved".to_owned(), Store::stem)
    }

    pub fn file(&self) -> &Path {
        &self.store.as_ref().expect("stored transcript").path
    }

    pub fn stored_file(&self) -> Option<&Path> {
        self.store.as_ref().map(|store| store.path.as_path())
    }

    /// Keeps `message`, then writes it. The first failed write is returned and ends saving:
    /// later messages stay in memory only, so a full disk costs the file, not the conversation.
    pub fn push(&mut self, message: Message) -> Result<(), Unsaved> {
        let written = match &mut self.store {
            Some(store) if !self.unsaved => store.append(&message).map_err(|e| Unsaved {
                path: store.path.clone(),
                error: format!("{e:#}"),
            }),
            _ => Ok(()),
        };
        self.unsaved |= written.is_err();
        self.ids.push(
            self.store
                .as_ref()
                .and_then(|s| s.last_id())
                .filter(|_| written.is_ok())
                .map(str::to_owned)
                .unwrap_or_else(super::entry::new_id),
        );
        self.messages.push(message);
        written?;
        self.save_state(self.state.clone())
    }

    pub fn context(&self) -> Vec<Message> {
        let latest = self.compactions.iter().rev().find_map(|c| {
            let head = self.ids.iter().position(|id| id == &c.head)?;
            let kept = self.ids.iter().position(|id| id == &c.kept)?;
            (kept <= head).then_some((c, kept))
        });
        let Some((summary, kept)) = latest else {
            return crate::agent::context::build(&self.messages);
        };
        let mut messages = vec![Message::User {
            parts: vec![crate::agent::message::Part::Reminder {
                text: crate::text::template::fill(
                    &super::workflow::text().summary,
                    &[("summary", &summary.summary)],
                ),
            }],
        }];
        // Active catalogs may predate the retained tail; keep their latest snapshots.
        let catalogs = crate::agent::context::build(&self.messages[..kept]);
        for message in catalogs {
            if let Message::User { parts } = message {
                let parts: Vec<_> = parts.into_iter().filter(|p| matches!(p, crate::agent::message::Part::Reminder { text } if crate::agent::context::catalog_prefixes().iter().any(|tag| text.starts_with(tag.as_str())))).collect();
                if !parts.is_empty() {
                    messages.push(Message::User { parts });
                }
            }
        }
        messages.extend_from_slice(&self.messages[kept..]);
        crate::agent::context::build(&messages)
    }

    pub fn compact(
        &mut self,
        kept: usize,
        summary: String,
        usage: Option<crate::agent::message::Usage>,
    ) -> Result<(), Unsaved> {
        let Some(head) = self.ids.last().cloned() else {
            return Ok(());
        };
        let Some(kept) = self.ids.get(kept).cloned() else {
            return Ok(());
        };
        self.record(super::entry::Entry::Compaction {
            head: head.clone(),
            kept: kept.clone(),
            summary: summary.clone(),
            usage,
        })?;
        self.compactions.push(super::entry::Compaction {
            head,
            kept,
            summary,
            usage,
        });
        Ok(())
    }

    pub fn snapshot(
        &mut self,
        of: String,
        before: String,
        after: Option<String>,
    ) -> Result<(), Unsaved> {
        self.record(super::entry::Entry::Snapshot {
            of: of.clone(),
            before: before.clone(),
            after: after.clone(),
        })?;
        self.snapshots
            .insert(of, super::entry::Snapshot { before, after });
        Ok(())
    }

    pub fn rewind(&mut self, len: usize) -> Result<(), Unsaved> {
        let head = len.checked_sub(1).and_then(|i| self.ids.get(i)).cloned();
        if self.unsaved {
            return Err(Unsaved {
                path: self.file().to_owned(),
                error: super::workflow::text().unsaved.clone(),
            });
        }
        if let Some(store) = &mut self.store {
            store.rewind(head, &self.state).map_err(|e| Unsaved {
                path: store.path.clone(),
                error: e.to_string(),
            })?;
        }
        self.messages.truncate(len);
        self.ids.truncate(len);
        self.compactions
            .retain(|c| self.ids.contains(&c.head) && self.ids.contains(&c.kept));
        Ok(())
    }

    fn record(&mut self, entry: super::entry::Entry) -> Result<(), Unsaved> {
        if let Some(store) = &mut self.store {
            if self.unsaved {
                return Err(Unsaved {
                    path: store.path.clone(),
                    error: super::workflow::text().unsaved.clone(),
                });
            }
            if let Err(error) = store.record(&entry) {
                self.unsaved = true;
                return Err(Unsaved {
                    path: store.path.clone(),
                    error: error.to_string(),
                });
            }
        }
        Ok(())
    }

    pub fn usage(&self) -> crate::agent::message::Usage {
        let mut total = crate::agent::message::Usage::default();
        for usage in self
            .messages
            .iter()
            .filter_map(|m| match m {
                Message::Assistant(a) => a.usage,
                Message::Tool(t) => t
                    .details
                    .as_ref()
                    .and_then(|d| d.get("usage"))
                    .and_then(|u| serde_json::from_value(u.clone()).ok()),
                _ => None,
            })
            .chain(self.compactions.iter().filter_map(|c| c.usage))
        {
            total.input += usage.input;
            total.cached += usage.cached;
            total.output += usage.output;
            total.reasoning += usage.reasoning;
            total.cost_micros += usage.cost_micros;
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::Part;
    use crate::io;
    use crate::paths::Paths;

    fn user(text: &str) -> Message {
        Message::User {
            parts: vec![Part::Text { text: text.into() }],
        }
    }

    #[test]
    fn latest_workspace_snapshot_survives_compaction() {
        let mut transcript = Transcript::new(None);
        assert!(
            transcript
                .push(Message::User {
                    parts: vec![
                        Part::Reminder {
                            text: "<workspace>current files</workspace>".into()
                        },
                        Part::Text {
                            text: "earlier request".into()
                        }
                    ]
                })
                .is_ok()
        );
        assert!(transcript.push(user("latest request")).is_ok());
        assert!(transcript.compact(1, "summary".into(), None).is_ok());
        let context = serde_json::to_string(&transcript.context()).unwrap();
        assert!(context.contains("current files"));
        assert!(context.contains("latest request"));
        assert!(!context.contains("earlier request"));
    }

    #[test]
    fn compaction_and_rewind_survive_reopening_without_erasing_history() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let mut transcript = Transcript::new(Some(Store::create(&paths)));
        for prompt in ["one", "two", "three"] {
            assert!(transcript.push(user(prompt)).is_ok());
        }
        assert!(
            transcript
                .compact(1, "earlier summary".into(), None)
                .is_ok()
        );
        let file = transcript.file().to_owned();
        drop(transcript);
        let mut resumed = Transcript::open(&paths, &Selection::Latest).unwrap();
        assert_eq!(resumed.messages.len(), 3);
        assert_eq!(resumed.compactions.len(), 1);
        assert!(
            serde_json::to_string(&resumed.context())
                .unwrap()
                .contains("earlier summary")
        );
        assert!(resumed.rewind(1).is_ok());
        assert!(resumed.push(user("new branch")).is_ok());
        drop(resumed);
        let resumed = Transcript::open(&paths, &Selection::Latest).unwrap();
        assert_eq!(resumed.messages.len(), 2);
        assert!(resumed.compactions.is_empty());
        let physical = io::fs::read_string(&file).unwrap().unwrap();
        assert!(physical.contains("three"));
        assert!(physical.contains("new branch"));
    }

    #[test]
    fn there_is_no_file_to_name_until_something_is_said() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let mut transcript = Transcript::new(Some(Store::create(&paths)));
        assert!(transcript.path().is_none());
        assert!(transcript.push(user("one")).is_ok());
        assert!(
            transcript
                .path()
                .is_some_and(|p| p.starts_with(paths.sessions_dir()))
        );
    }

    #[test]
    fn a_failed_write_keeps_the_message_and_is_reported_once() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().into());
        let mut store = Store::create(&paths);
        // A file where its directory should be: every later append fails.
        let blocker = root.path().join("blocker");
        io::fs::write_atomic(&blocker, b"", io::fs::Access::Shared).unwrap();
        store.path = blocker.join("session.jsonl");
        let mut transcript = Transcript::new(Some(store));

        let first = transcript.push(user("one"));
        assert!(first.is_err_and(|u| u.path.ends_with("session.jsonl")));
        assert!(transcript.push(user("two")).is_ok());
        assert_eq!(transcript.messages.len(), 2);
        assert!(transcript.path().is_none());
    }
}

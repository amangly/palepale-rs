use std::path::{Path, PathBuf};

use crate::agent::message::Message;
use crate::session::store::{SavedState, Selection, Store};

/// The conversation in memory, mirrored to its session file when it has one.
pub struct Transcript {
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
        self.messages.push(message);
        written?;
        self.save_state(self.state.clone())
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

use std::path::PathBuf;

use crate::agent::message::Message;
use crate::io;
use crate::paths::Paths;
use crate::session::entry::{self, Entry, FORMAT_VERSION};
use crate::text::template::fill;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedState {
    pub model: String,
    pub mode: String,
    pub effort: Option<String>,
    pub agent_profile: Option<String>,
    pub system_prompt: String,
}

pub struct Summary {
    pub id: String,
    pub title: String,
    pub model: String,
    pub activity: jiff::Timestamp,
}

#[derive(Clone, Default)]
pub enum Selection {
    #[default]
    New,
    Latest,
    Id(String),
}

/// An append-only JSONL session file; the only writer of its file. Nothing is written until
/// the first message, so a session that never says anything leaves no file.
pub struct Store {
    pub path: PathBuf,
    header: Entry,
    written: bool,
    last: Option<String>,
    lock: Option<io::fs::FileLock>,
}

impl Store {
    pub fn create(paths: &Paths) -> Self {
        let id = entry::new_id();
        let created = io::clock::now();
        let stamp = created.strftime("%Y%m%dT%H%M%S");
        Self {
            path: paths.sessions_dir().join(format!("{stamp}_{id}.jsonl")),
            header: Entry::Session {
                v: FORMAT_VERSION,
                id,
                created,
                cwd: paths.project.display().to_string(),
                palepale: env!("CARGO_PKG_VERSION").to_owned(),
            },
            written: false,
            last: None,
            lock: None,
        }
    }

    pub fn open(
        paths: &Paths,
        selection: &Selection,
    ) -> anyhow::Result<(Self, crate::session::reader::Loaded)> {
        let text = crate::session::text::get();
        let id = match selection {
            Selection::New => anyhow::bail!("a new session cannot be resumed"),
            Selection::Latest => list(paths)?
                .first()
                .map(|item| item.id.clone())
                .ok_or_else(|| anyhow::anyhow!(text.empty.clone()))?,
            Selection::Id(id) => id.clone(),
        };
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            anyhow::bail!(fill(&text.missing, &[("id", &id)]));
        }
        let path = paths.sessions_dir().join(format!("{id}.jsonl"));
        if !io::fs::canonical(&path).starts_with(io::fs::canonical(&paths.sessions_dir())) {
            anyhow::bail!(text.different_project.clone());
        }
        let lock = io::fs::lock(&path.with_extension("lock"))
            .map_err(|_| anyhow::anyhow!(text.locked.clone()))?;
        let bytes = io::fs::read_bytes_limited(&path, text.max_bytes.value)?
            .ok_or_else(|| anyhow::anyhow!(fill(&text.missing, &[("id", &id)])))?;
        let contents = readable(&bytes)?;
        let loaded = crate::session::reader::load(&contents).map_err(|error| {
            anyhow::anyhow!(fill(&text.history_error, &[("error", &error.to_string())]))
        })?;
        let Entry::Session { cwd, .. } = &loaded.header else {
            unreachable!()
        };
        if io::fs::canonical(std::path::Path::new(cwd)) != io::fs::canonical(&paths.project) {
            anyhow::bail!(text.different_project.clone());
        }
        if loaded.valid_bytes < bytes.len() {
            io::fs::write_atomic(
                &path.with_extension("partial"),
                &bytes[loaded.valid_bytes..],
                io::fs::Access::Private,
            )?;
            io::fs::truncate(&path, loaded.valid_bytes as u64)?;
        }
        if loaded.valid_bytes > 0 && !contents[..loaded.valid_bytes].ends_with('\n') {
            io::fs::append_line(&path, "")?;
        }
        Ok((
            Self {
                path,
                header: loaded.header.clone(),
                written: true,
                last: loaded.head.clone(),
                lock: Some(lock),
            },
            loaded,
        ))
    }

    fn ensure_open(&mut self) -> anyhow::Result<()> {
        if self.lock.is_none() {
            self.lock = Some(io::fs::lock(&self.path.with_extension("lock"))?);
        }
        if !self.written {
            // A new JSONL is private before its first message is written.
            let _file = io::fs::Appender::create(&self.path, io::fs::Access::Private)?;
            self.write(&self.header)?;
            self.written = true;
        }
        Ok(())
    }

    pub fn save_state(&mut self, state: &SavedState) -> anyhow::Result<()> {
        self.ensure_open()?;
        self.write(&Entry::State {
            head: self.last.clone(),
            model: state.model.clone(),
            mode: state.mode.clone(),
            effort: state.effort.clone(),
            agent_profile: state.agent_profile.clone(),
            system_prompt: state.system_prompt.clone(),
        })
    }

    /// Whether the file exists yet: the header goes out with the first message.
    pub fn written(&self) -> bool {
        self.written
    }

    /// The file name without extension, unique per session.
    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn append(&mut self, message: &Message) -> anyhow::Result<()> {
        self.ensure_open()?;
        let id = entry::new_id();
        let parent = self.last.clone();
        self.write(&Entry::Message {
            id: id.clone(),
            parent,
            ts: io::clock::now(),
            message: message.clone(),
        })?;
        self.last = Some(id);
        Ok(())
    }

    fn write(&self, entry: &Entry) -> anyhow::Result<()> {
        io::fs::append_line(&self.path, &serde_json::to_string(entry)?)?;
        Ok(())
    }
}

pub fn list(paths: &Paths) -> anyhow::Result<Vec<Summary>> {
    let text = crate::session::text::get();
    let entries = match io::fs::list_dir(&paths.sessions_dir()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut found = Vec::new();
    for entry in entries
        .into_iter()
        .rev()
        .filter(|entry| !entry.is_dir && entry.name.ends_with(".jsonl"))
        .take(text.list_count.value * 10)
    {
        let path = paths.sessions_dir().join(&entry.name);
        if io::fs::is_link(&path) {
            continue;
        }
        let Some(bytes) = io::fs::read_bytes_limited(&path, text.max_bytes.value)
            .ok()
            .flatten()
        else {
            continue;
        };
        let Ok(contents) = readable(&bytes) else {
            continue;
        };
        let Ok(loaded) = crate::session::reader::load(&contents) else {
            continue;
        };
        let Entry::Session { cwd, .. } = loaded.header else {
            continue;
        };
        if io::fs::canonical(std::path::Path::new(&cwd)) != io::fs::canonical(&paths.project) {
            continue;
        }
        found.push(Summary {
            id: entry.name.trim_end_matches(".jsonl").to_owned(),
            title: loaded.title,
            model: loaded.state.model,
            activity: loaded.activity,
        });
    }
    found.sort_by(|a, b| b.activity.cmp(&a.activity).then_with(|| b.id.cmp(&a.id)));
    found.truncate(text.list_count.value);
    Ok(found)
}

/// Permit only a split UTF-8 codepoint at EOF; the JSON reader then validates that the
/// containing final record is incomplete. Recovery preserves the original raw bytes.
fn readable(bytes: &[u8]) -> anyhow::Result<std::borrow::Cow<'_, str>> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(std::borrow::Cow::Borrowed(text)),
        Err(error) if error.error_len().is_none() => Ok(String::from_utf8_lossy(bytes)),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_header_then_linked_messages() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::at(home.path().into(), home.path().join("proj"));
        let mut store = Store::create(&paths);
        assert!(
            !store.path.exists(),
            "a session that has said nothing has no file"
        );
        store.append(&Message::user_text("one")).unwrap();
        store.append(&Message::user_text("two")).unwrap();
        let text = io::fs::read_string(&store.path).unwrap().unwrap();
        let entries: Vec<Entry> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(matches!(entries[0], Entry::Session { .. }));
        let (Entry::Message { id: first, .. }, Entry::Message { parent, .. }) =
            (&entries[1], &entries[2])
        else {
            panic!("two messages expected");
        };
        assert_eq!(parent.as_ref(), Some(first));
    }

    #[test]
    fn writers_exclude_each_other_then_resume_the_same_history_and_settings() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path().join("home"), temp.path().join("project"));
        io::fs::make_dir(&paths.project).unwrap();
        let mut transcript =
            crate::session::transcript::Transcript::open(&paths, &Selection::New).unwrap();
        transcript.state = SavedState {
            model: "saved-model".into(),
            mode: "plan".into(),
            system_prompt: "review".into(),
            agent_profile: Some("profile-1".into()),
            effort: Some("high".into()),
        };
        transcript
            .push(Message::user_text("first request"))
            .ok()
            .unwrap();
        let id = transcript.id();
        let second = Store::open(&paths, &Selection::Id(id.clone()));
        assert!(
            second
                .err()
                .unwrap()
                .to_string()
                .contains("another process")
        );
        assert_eq!(list(&paths).unwrap().len(), 1);
        let state = transcript.state.clone();
        let path = transcript.file().to_owned();
        drop(transcript);
        io::fs::append_line(&path, "").unwrap();
        // An empty complete line is corruption rather than a truncated write.
        assert!(Store::open(&paths, &Selection::Id(id.clone())).is_err());
        let bytes = io::fs::read_bytes(&path).unwrap();
        io::fs::truncate(&path, bytes.len() as u64 - 1).unwrap();
        let (mut store, loaded) = Store::open(&paths, &Selection::Latest).unwrap();
        assert_eq!(loaded.state, state);
        assert_eq!(loaded.messages, vec![Message::user_text("first request")]);
        store.append(&Message::user_text("second request")).unwrap();
        drop(store);
        let (_, loaded) = Store::open(&paths, &Selection::Id(id)).unwrap();
        assert_eq!(loaded.messages.len(), 2);
    }

    #[test]
    fn a_colliding_project_bucket_cannot_resume_a_foreign_header() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path().join("home"), temp.path().join("project"));
        let mut store = Store::create(&paths);
        store.append(&Message::user_text("private")).unwrap();
        let id = store.stem();
        let path = store.path.clone();
        drop(store);
        let text = io::fs::read_string(&path).unwrap().unwrap();
        let text = text
            .lines()
            .map(|line| {
                let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
                if value["type"] == "session" {
                    value["cwd"] = temp.path().join("other").display().to_string().into();
                }
                value.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        io::fs::write_atomic(&path, text.as_bytes(), io::fs::Access::Private).unwrap();
        assert!(
            Store::open(&paths, &Selection::Id(id))
                .err()
                .unwrap()
                .to_string()
                .contains("another project")
        );
    }

    #[test]
    fn a_split_utf8_codepoint_at_eof_is_preserved_in_the_recovery_sidecar() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path().join("home"), temp.path().join("project"));
        let mut store = Store::create(&paths);
        store.append(&Message::user_text("valid request")).unwrap();
        let path = store.path.clone();
        let id = store.stem();
        drop(store);
        let mut bytes = io::fs::read_bytes(&path).unwrap();
        let tail = b"{\"type\":\"message\",\"message\":\"\xc3";
        bytes.extend_from_slice(tail);
        io::fs::write_atomic(&path, &bytes, io::fs::Access::Private).unwrap();
        let (resumed, loaded) = Store::open(&paths, &Selection::Id(id)).unwrap();
        assert_eq!(loaded.messages, vec![Message::user_text("valid request")]);
        assert_eq!(
            io::fs::read_bytes(&path.with_extension("partial")).unwrap(),
            tail
        );
        drop(resumed);
        assert!(
            crate::session::reader::load(&io::fs::read_string(&path).unwrap().unwrap()).is_ok()
        );
    }
}

//! Reading a session file back: the messages of every line that parses, in order. A line this
//! build cannot read (a newer build's type, part or finish) is skipped, not fatal.

use crate::agent::message::Message;
use crate::session::entry::Entry;
use crate::session::store::SavedState;

/// A validated active branch, separate from the append-only physical record order.
pub struct Loaded {
    pub ids: Vec<String>,
    pub snapshots: std::collections::HashMap<String, super::entry::Snapshot>,
    pub compactions: Vec<super::entry::Compaction>,
    pub header: Entry,
    pub head: Option<String>,
    pub messages: Vec<Message>,
    pub state: SavedState,
    pub valid_bytes: usize,
    pub title: String,
    pub activity: jiff::Timestamp,
}

pub fn load(text: &str) -> anyhow::Result<Loaded> {
    use std::collections::{HashMap, HashSet};
    let mut nodes = HashMap::new();
    let mut header = None;
    let mut head = None;
    let mut state = SavedState::default();
    let mut valid_bytes = 0;
    let mut activity = None;
    let mut snapshots = HashMap::new();
    let mut compactions = Vec::new();
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let entry: Entry = match serde_json::from_str(line) {
            Ok(entry) => entry,
            Err(error) if error.is_eof() && !line.ends_with('\n') && header.is_some() => break,
            Err(error) => anyhow::bail!("record {}: {error}", index + 1),
        };
        match &entry {
            Entry::Snapshot { of, before, after } if header.is_some() => {
                snapshots.insert(
                    of.clone(),
                    super::entry::Snapshot {
                        before: before.clone(),
                        after: after.clone(),
                    },
                );
            }
            Entry::Compaction {
                head,
                kept,
                summary,
                usage,
            } if header.is_some() => {
                compactions.push(super::entry::Compaction {
                    head: head.clone(),
                    kept: kept.clone(),
                    summary: summary.clone(),
                    usage: *usage,
                });
            }
            Entry::Parent { .. } if header.is_some() => {}
            Entry::Session { v, created, .. } if index == 0 && *v == 1 => {
                header = Some(entry.clone());
                activity = Some(*created);
            }
            Entry::Session { .. } => anyhow::bail!("invalid or unsupported session header"),
            Entry::Message {
                id,
                parent,
                ts,
                message,
            } if header.is_some() => {
                if nodes
                    .insert(id.clone(), (parent.clone(), message.clone()))
                    .is_some()
                {
                    anyhow::bail!("duplicate entry ID `{id}`");
                }
                head = Some(id.clone());
                activity = Some(*ts);
            }
            Entry::State {
                head: active,
                model,
                mode,
                effort,
                agent_profile,
                system_prompt,
            } if header.is_some() => {
                head.clone_from(active);
                state = SavedState {
                    model: model.clone(),
                    mode: mode.clone(),
                    effort: effort.clone(),
                    agent_profile: agent_profile.clone(),
                    system_prompt: system_prompt.clone(),
                };
            }
            _ => anyhow::bail!("missing session header"),
        }
        valid_bytes += line.len();
    }
    let header = header.ok_or_else(|| anyhow::anyhow!("missing session header"))?;
    let mut messages = Vec::new();
    let mut ids = Vec::new();
    let mut visited = HashSet::new();
    let mut cursor = head.clone();
    while let Some(id) = cursor.take() {
        if !visited.insert(id.clone()) {
            anyhow::bail!("cycle in active history at `{id}`");
        }
        let (parent, message) = nodes
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("missing active entry `{id}`"))?;
        messages.push(message.clone());
        ids.push(id);
        cursor = parent.clone();
    }
    messages.reverse();
    ids.reverse();
    compactions.retain(|c| ids.contains(&c.head) && ids.contains(&c.kept));
    if state.model.is_empty() {
        state.model = messages
            .iter()
            .rev()
            .find_map(|message| match message {
                Message::Assistant(reply) => Some(reply.model.clone()),
                _ => None,
            })
            .unwrap_or_default();
    }
    if state.effort.is_none() {
        state.effort = messages.iter().rev().find_map(|message| match message {
            Message::Assistant(reply) => reply.effort.clone(),
            _ => None,
        });
    }
    let title = messages
        .iter()
        .find_map(|message| match message {
            Message::User { parts } => parts.iter().find_map(|part| match part {
                crate::agent::message::Part::Text { text } => {
                    text.lines().next().map(|s| s.chars().take(60).collect())
                }
                _ => None,
            }),
            _ => None,
        })
        .unwrap_or_default();
    Ok(Loaded {
        ids,
        snapshots,
        compactions,
        header,
        head,
        messages,
        state,
        valid_bytes,
        title,
        activity: activity.expect("a header has a timestamp"),
    })
}

pub fn messages(text: &str) -> Vec<Message> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
        .filter_map(|entry| match entry {
            Entry::Message { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_it_cannot_read_is_skipped() {
        let v1 = include_str!("../../tests/fixtures/session/v1.jsonl");
        let text = format!("{v1}{{\"type\":\"from_the_future\"}}\nnot json\n");
        assert_eq!(messages(&text).len(), messages(v1).len());
        assert_eq!(messages(v1).len(), 10);
    }

    #[test]
    fn active_heads_select_a_branch_and_truncated_tails_do_not_hide_corruption() {
        let header = r#"{"type":"session","v":1,"id":"session","created":"2026-10-15T00:00:00Z","cwd":"/repo"}"#;
        let message = |id: &str, parent: Option<&str>, text: &str| {
            serde_json::json!({
                "type":"message", "id":id, "parent":parent, "ts":"2026-10-15T00:00:00Z",
                "message":{"role":"user","parts":[{"type":"text","text":text}]}
            })
            .to_string()
        };
        let text = format!(
            "{header}\n{}\n{}\n{}\n{{\"type\":\"state\",\"head\":\"branch\",\"mode\":\"plan\"}}\n",
            message("first", None, "one"),
            message("old", Some("first"), "old-branch"),
            message("branch", Some("first"), "new-branch")
        );
        let loaded = load(&format!("{text}{{\"type\":")).unwrap();
        assert_eq!(loaded.valid_bytes, text.len());
        assert_eq!(loaded.state.mode, "plan");
        assert_eq!(
            loaded.messages,
            vec![Message::user_text("one"), Message::user_text("new-branch")]
        );
        assert!(load(&format!("{text}bad-record\n")).is_err());
        assert!(load(&format!("{text}{{\"type\":\"from_the_future\"}}")).is_err());
        assert!(load(&format!("{text}{}\n", message("first", None, "duplicate"))).is_err());
        assert!(
            load(&format!(
                "{text}{{\"type\":\"state\",\"head\":\"missing\"}}\n"
            ))
            .is_err()
        );
        assert!(
            load(&format!(
                "{header}\n{}\n",
                message("cycle", Some("cycle"), "bad")
            ))
            .is_err()
        );
    }

    #[test]
    fn frozen_v1_history_reconstructs_without_changing_the_fixture() {
        // The frozen fixture concatenates two example sessions; its first five lines are
        // the real linked v1 conversation. The rest exercise optional message fields.
        let text = include_str!("../../tests/fixtures/session/v1.jsonl")
            .lines()
            .take(5)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let loaded = load(&text).unwrap();
        assert_eq!(loaded.messages, messages(&text));
        assert_eq!(loaded.state.model, "demo-2");
        assert_eq!(loaded.state.effort.as_deref(), Some("high"));
    }
}

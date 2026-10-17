//! Turn rollback and its display text.
use super::transcript::Transcript;
use crate::agent::message::{Message, Part};
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
pub struct Text {
    pub summary: String,
    pub unsaved: String,
    pub nothing: String,
    pub undone: String,
    pub no_snapshot: String,
    pub incomplete: String,
    pub compacting: String,
    pub compacted: String,
    pub compact_empty: String,
    pub summary_empty: String,
    pub task_invalid: String,
    pub task_depth: String,
    pub task_no_run: String,
    pub task_unknown: String,
    pub task_cancelled: String,
    pub task_ended: String,
}
pub fn text() -> &'static Text {
    static TEXT: LazyLock<Text> = LazyLock::new(|| {
        toml::from_str(include_str!("../../data/workflow.toml")).expect("workflow text")
    });
    &TEXT
}

pub fn last_prompt(transcript: &Transcript) -> Option<usize> {
    transcript.messages.iter().rposition(|m| matches!(m, Message::User { parts } if parts.iter().any(|p| matches!(p, Part::Text { .. } | Part::Attachment(_) | Part::McpPrompt { .. }))))
}

pub async fn undo(
    paths: &crate::paths::Paths,
    transcript: &mut Transcript,
) -> anyhow::Result<(Vec<Part>, String, bool)> {
    let at = last_prompt(transcript).ok_or_else(|| anyhow::anyhow!(text().nothing.clone()))?;
    let Message::User { parts } = &transcript.messages[at] else {
        unreachable!()
    };
    let prompt = parts
        .iter()
        .filter(|p| !matches!(p, Part::Reminder { .. }))
        .cloned()
        .collect();
    let snapshot = transcript
        .ids
        .get(at)
        .and_then(|id| transcript.snapshots.get(id))
        .cloned();
    // Persist the head first; a failed writer must never silently restore files.
    transcript
        .rewind(at)
        .map_err(|e| anyhow::anyhow!(e.error))?;
    let mut retry_ready = true;
    let note = if let Some(snapshot) = snapshot {
        if let Some(after) = snapshot.after {
            let shadow = crate::io::snapshot::Shadow::open(paths).await;
            match shadow.restore(&snapshot.before, Some(&after)).await {
                Ok(restored) => {
                    retry_ready = restored.failed.is_empty() && restored.kept.is_empty();
                    crate::text::template::fill(
                        &text().undone,
                        &[
                            ("files", &restored.files.to_string()),
                            ("kept", &restored.kept.join(", ")),
                            ("failed", &restored.failed.join("; ")),
                        ],
                    )
                }
                Err(error) => {
                    retry_ready = false;
                    error
                }
            }
        } else {
            retry_ready = false;
            text().incomplete.clone()
        }
    } else {
        text().no_snapshot.clone()
    };
    Ok((prompt, note, retry_ready))
}

use crate::io::search::{self, MentionPaths};
use crate::text::{
    mentions,
    styled::{Line, Style},
};
use crate::tui::{boxed::boxed, editor::Editor, keymap::Action};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub struct Picker {
    seen: (String, usize),
    dismissed: bool,
    selected: usize,
    root: PathBuf,
    discovery: Option<(
        std::sync::mpsc::Receiver<MentionPaths>,
        tokio_util::sync::CancellationToken,
    )>,
    paths: Vec<String>,
    truncated: bool,
    ready: bool,
    found: Vec<(String, String)>,
    token: Option<mentions::Mention>,
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.close();
    }
}

impl Picker {
    pub fn loading(&self) -> bool {
        self.discovery.is_some()
    }

    fn close(&mut self) {
        if let Some((_, cancel)) = self.discovery.take() {
            cancel.cancel();
        }
        self.paths.clear();
        self.ready = false;
        self.token = None;
    }

    pub fn sync(&mut self, editor: &Editor, root: &Path, agents: &[(String, String)]) {
        let text = editor.text();
        let cursor = editor.cursor();
        if self.seen != (text.clone(), cursor) {
            self.seen = (text.clone(), cursor);
            self.dismissed = false;
            self.selected = 0;
        }
        let token = mentions::active(&text, cursor);
        if self.dismissed || token.is_none() || text.trim_start().starts_with('/') {
            self.close();
            return;
        }
        if self.root != root {
            self.close();
            self.root = root.to_owned();
        }
        self.token = token;
        if !self.ready && self.discovery.is_none() {
            self.discovery = Some(search::mention_paths(root.to_owned()));
        }
        if let Some((receive, _)) = &self.discovery {
            match receive.try_recv() {
                Ok(result) => {
                    self.paths = result.paths;
                    self.truncated = result.truncated;
                    self.ready = true;
                    self.discovery = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ready = true;
                    self.truncated = true;
                    self.discovery = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let query = &self.token.as_ref().unwrap().path;
        let mut candidates: Vec<_> = self
            .paths
            .iter()
            .map(|p| {
                let kind = if p.ends_with('/') {
                    &mentions::text().folder
                } else {
                    &mentions::text().file
                };
                let path = if crate::tools::task::catalog().any(|(id, _)| id == p) {
                    format!("./{p}")
                } else {
                    p.clone()
                };
                (path, kind.clone())
            })
            .chain(agents.iter().cloned())
            .filter_map(|(path, label)| {
                mentions::score(&path, query).map(|rank| (rank, path, label))
            })
            .collect();
        candidates.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        self.found = candidates
            .into_iter()
            .take(crate::limits::get().tui.palette_rows.value)
            .map(|(_, p, l)| (p, l))
            .collect();
        self.selected = self.selected.min(self.found.len().saturating_sub(1));
    }

    pub fn key(&mut self, action: Action, editor: &mut Editor) -> bool {
        let Some(token) = &self.token else {
            return false;
        };
        match action {
            Action::Escape => {
                self.dismissed = true;
                self.close();
            }
            Action::Up => {
                self.selected = self
                    .selected
                    .checked_sub(1)
                    .unwrap_or(self.found.len().saturating_sub(1))
            }
            Action::Down => self.selected = (self.selected + 1) % self.found.len().max(1),
            Action::Tab | Action::Submit if !self.found.is_empty() => {
                let replacement = format!("{} ", mentions::quote(&self.found[self.selected].0));
                editor.replace(token.span.clone(), &replacement);
                self.seen = (editor.text(), editor.cursor());
                self.dismissed = true;
                self.close();
            }
            Action::Tab | Action::Submit if self.loading() => {}
            _ => return false,
        }
        true
    }

    pub fn render(&self, width: usize) -> Vec<Line> {
        if self.token.is_none() {
            return Vec::new();
        }
        let mut rows: Vec<_> = self
            .found
            .iter()
            .enumerate()
            .map(|(i, (path, label))| {
                Line::styled(
                    format!(
                        "{}{}  {}",
                        if i == self.selected { "› " } else { "  " },
                        path,
                        label
                    ),
                    Style::fg(if i == self.selected {
                        "accent"
                    } else {
                        "muted"
                    }),
                )
            })
            .collect();
        if self.loading() {
            rows.push(Line::styled(&mentions::text().loading, Style::fg("muted")));
        }
        if self.truncated {
            rows.push(Line::styled(
                &mentions::text().incomplete,
                Style::fg("muted"),
            ));
        }
        if rows.is_empty() {
            rows.push(Line::styled(&mentions::text().empty, Style::fg("muted")));
        }
        boxed(
            Line::styled(&mentions::text().title, Style::fg("muted")),
            &rows,
            width,
        )
    }
}

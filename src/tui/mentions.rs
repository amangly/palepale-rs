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
    skills: Vec<(String, String)>,
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
    pub fn set_skills(&mut self, skills: Vec<(String, String)>) {
        self.skills = skills;
    }

    pub fn loading(&self) -> bool {
        self.discovery.is_some()
    }

    fn close(&mut self) {
        if let Some((_, cancel)) = self.discovery.take() {
            cancel.cancel();
        }
        self.paths.clear();
        self.ready = false;
        self.truncated = false;
        self.token = None;
    }

    pub fn sync(
        &mut self,
        editor: &Editor,
        root: &Path,
        agents: &[(String, String)],
        policy: &crate::permission::gate::Policy,
    ) {
        let text = editor.text();
        let cursor = editor.cursor();
        if self.seen != (text.clone(), cursor) {
            self.seen = (text.clone(), cursor);
            self.dismissed = false;
            self.selected = 0;
        }
        let token = mentions::active(&text, cursor).filter(|t| matches!(t.sigil, '@' | '$'));
        if self.dismissed || token.is_none() || text.trim_start().starts_with('/') {
            self.close();
            return;
        }
        if token.as_ref().is_some_and(|t| t.sigil == '$') {
            self.close();
            let token = token.unwrap();
            let found = self
                .skills
                .iter()
                .filter(|(name, _)| {
                    use crate::permission::gate::{Ask, Decision};
                    !matches!(policy.decide(&[Ask::new("skill", name)]), Decision::Deny(_))
                })
                .cloned();
            self.found = ranked(found, &token.path);
            self.selected = self.selected.min(self.found.len().saturating_sub(1));
            self.token = Some(token);
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
        let candidates = self
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
            .chain(agents.iter().cloned());
        self.found = ranked(candidates, query);
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
                let name = &self.found[self.selected].0;
                let mut replacement = if token.sigil == '$' {
                    format!("${name}")
                } else {
                    mentions::quote(name)
                };
                if !editor.text()[token.span.end..].starts_with(char::is_whitespace) {
                    replacement.push(' ');
                }
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
        let Some(token) = &self.token else {
            return Vec::new();
        };
        let skills = token.sigil == '$';
        let visible = crate::limits::get().tui.palette_rows.value.max(1);
        let start = (self.selected + 1).saturating_sub(visible);
        let mut rows: Vec<_> = self
            .found
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
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
            rows.push(Line::styled(
                if skills {
                    &mentions::text().skills_empty
                } else {
                    &mentions::text().empty
                },
                Style::fg("muted"),
            ));
        }
        boxed(
            Line::styled(
                if skills {
                    &mentions::text().skills_title
                } else {
                    &mentions::text().title
                },
                Style::fg("muted"),
            ),
            &rows,
            width,
        )
    }
}

fn ranked(
    candidates: impl Iterator<Item = (String, String)>,
    query: &str,
) -> Vec<(String, String)> {
    let mut matches: Vec<_> = candidates
        .filter_map(|(path, label)| mentions::score(&path, query).map(|rank| (rank, path, label)))
        .collect();
    matches.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    matches.into_iter().map(|(_, p, l)| (p, l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{
        gate::Policy,
        rule::{Action as RuleAction, Rule},
    };

    #[test]
    fn every_match_is_reachable_beyond_the_first_visible_page() {
        let count = crate::limits::get().tui.palette_rows.value + 3;
        let mut picker = Picker::default();
        picker.found = ranked(
            (0..count).map(|i| (format!("file-{i:03}.txt"), String::new())),
            "",
        );
        picker.token = mentions::active("draft @", 7);
        let mut editor = Editor::new(10);
        editor.set_text("draft @");
        for _ in 1..count {
            assert!(picker.key(Action::Down, &mut editor));
        }
        assert!(picker.key(Action::Submit, &mut editor));
        assert_eq!(editor.text(), format!("draft @file-{:03}.txt ", count - 1));
    }

    #[test]
    fn skill_completion_preserves_draft_and_respects_denials_and_escape() {
        let rules = [Rule {
            permission: "skill".into(),
            pattern: "review-private".into(),
            action: RuleAction::Deny,
        }];
        let policy = Policy {
            rules: &rules,
            mode: crate::mode::get("manual").unwrap(),
            plan_file: "",
        };
        let mut picker = Picker::default();
        picker.set_skills(vec![
            ("review-private".into(), "Denied".into()),
            ("review-public".into(), "Available".into()),
        ]);
        let mut editor = Editor::new(10);
        editor.set_text("你好 $rev suffix");
        for _ in " suffix".chars() {
            editor.apply(Action::Left);
        }
        picker.sync(&editor, Path::new("."), &[], &policy);
        assert_eq!(picker.found.len(), 1);
        assert_eq!(picker.found[0].0, "review-public");
        assert!(picker.key(Action::Submit, &mut editor));
        assert_eq!(editor.text(), "你好 $review-public suffix");
        assert!(!picker.key(Action::Submit, &mut editor));

        editor.set_text("draft $");
        picker.sync(&editor, Path::new("."), &[], &policy);
        assert!(picker.key(Action::Escape, &mut editor));
        picker.sync(&editor, Path::new("."), &[], &policy);
        assert!(picker.render(80).is_empty());
        assert_eq!(editor.text(), "draft $");
        editor.insert("rev");
        picker.sync(&editor, Path::new("."), &[], &policy);
        assert!(!picker.render(80).is_empty());
    }
}

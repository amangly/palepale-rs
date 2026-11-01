//! The server's current coding profiles, picked for this conversation.

use crate::api::agent_profiles::Profile;
use crate::commands::godmode;
use crate::text::styled::{Line, Style};
use crate::tui::boxed::boxed;
use crate::tui::keymap::Action;
use crate::tui::{picker, ui};

pub struct Picker {
    ids: Vec<Option<String>>,
    rows: Vec<Line>,
    selected: usize,
    custom: bool,
}

pub enum Step {
    Stay,
    Pick(Option<String>),
    Close,
    Custom,
    ApplyCustom,
}

impl Picker {
    pub fn new(profiles: Vec<Profile>, current: Option<&str>) -> Self {
        let mut ids = vec![None];
        let words = godmode::text();
        let mut rows = vec![Line::plain(format!("{}  {}", words.off, words.off_summary))];
        for profile in profiles {
            let mut row = Line::styled(&profile.name, Style::fg("accent"));
            if !profile.description.is_empty() && profile.description != profile.name {
                row.push(format!("  ({})", profile.description), Style::fg("muted"));
            }
            ids.push(Some(profile.id));
            rows.push(row);
        }
        rows.push(Line::plain(format!(
            "{}  {}",
            words.custom, words.custom_summary
        )));
        let selected = ids
            .iter()
            .position(|id| id.as_deref() == current)
            .unwrap_or_else(|| {
                if current == Some(&words.custom_prompt_id) {
                    rows.len() - 1
                } else {
                    0
                }
            });
        Self {
            ids,
            rows,
            selected,
            custom: false,
        }
    }

    pub fn custom() -> Self {
        Self {
            ids: vec![],
            rows: vec![
                Line::plain(&godmode::text().custom_apply),
                Line::plain(&godmode::text().custom_cancel),
            ],
            selected: 0,
            custom: true,
        }
    }

    pub fn key(&mut self, action: Option<Action>) -> Step {
        match action {
            Some(Action::Up) => {
                self.selected = self.selected.saturating_sub(1);
                Step::Stay
            }
            Some(Action::Down) => {
                self.selected = (self.selected + 1).min(self.rows.len() - 1);
                Step::Stay
            }
            Some(Action::Right | Action::Submit) if self.custom => {
                if self.selected == 0 {
                    Step::ApplyCustom
                } else {
                    Step::Close
                }
            }
            Some(Action::Right | Action::Submit) if self.selected == self.ids.len() => Step::Custom,
            Some(Action::Right | Action::Submit) => Step::Pick(self.ids[self.selected].clone()),
            Some(Action::Left | Action::Escape) => Step::Close,
            _ => Step::Stay,
        }
    }

    pub fn render(&self, width: usize) -> Vec<Line> {
        let body = picker::marked(self.rows.clone(), self.selected);
        let needed = body.iter().map(Line::width).max().unwrap_or(0) + 4;
        let mut lines = boxed(
            Line::styled(&godmode::text().title, Style::default().bold()),
            &body,
            needed.min(width),
        );
        lines.push(Line::styled(&ui::get().picker.hint, Style::fg("muted")).truncate(width));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_is_available_without_server_profiles_and_apply_can_be_cancelled() {
        let mut picker = Picker::new(vec![], None);
        picker.key(Some(Action::Down));
        assert!(matches!(picker.key(Some(Action::Submit)), Step::Custom));
        let mut picker = Picker::custom();
        assert!(matches!(
            picker.key(Some(Action::Submit)),
            Step::ApplyCustom
        ));
        picker.key(Some(Action::Down));
        assert!(matches!(picker.key(Some(Action::Submit)), Step::Close));
    }
}

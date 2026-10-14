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
}

pub enum Step {
    Stay,
    Pick(Option<String>),
    Close,
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
        let selected = ids
            .iter()
            .position(|id| id.as_deref() == current)
            .unwrap_or(0);
        Self {
            ids,
            rows,
            selected,
        }
    }

    pub fn key(&mut self, action: Option<Action>) -> Step {
        match action {
            Some(Action::Up) => {
                self.selected = self.selected.saturating_sub(1);
                Step::Stay
            }
            Some(Action::Down) => {
                self.selected = (self.selected + 1).min(self.ids.len() - 1);
                Step::Stay
            }
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

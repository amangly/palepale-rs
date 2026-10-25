//! Reasoning effort selection for the currently selected model.

use crate::text::styled::{Line, Style};
use crate::tui::boxed::boxed;
use crate::tui::keymap::Action;
use crate::tui::picker;
use crate::tui::ui;

pub struct Picker {
    ids: Vec<String>,
    rows: Vec<Line>,
    selected: usize,
}

pub enum Step {
    Stay,
    Pick(String),
    Close,
}

impl Picker {
    pub fn new(efforts: &[String], current: Option<&str>, default: Option<&str>) -> Self {
        let selected_id = current
            .filter(|id| efforts.iter().any(|effort| effort == id))
            .or_else(|| default.filter(|id| efforts.iter().any(|effort| effort == id)))
            .or_else(|| efforts.first().map(String::as_str));
        let selected = selected_id
            .and_then(|id| efforts.iter().position(|effort| effort == id))
            .unwrap_or(0);
        Self {
            ids: efforts.to_vec(),
            rows: efforts.iter().map(Line::plain).collect(),
            selected,
        }
    }

    pub fn key(&mut self, action: Option<Action>) -> Step {
        let last = self.ids.len().saturating_sub(1);
        match action {
            Some(Action::Up) => self.selected = self.selected.saturating_sub(1),
            Some(Action::Down) => self.selected = (self.selected + 1).min(last),
            Some(Action::Right | Action::Submit) => {
                return self
                    .ids
                    .get(self.selected)
                    .map_or(Step::Close, |id| Step::Pick(id.clone()));
            }
            Some(Action::Left | Action::Escape) => return Step::Close,
            _ => {}
        }
        Step::Stay
    }

    pub fn render(&self, width: usize) -> Vec<Line> {
        let body = picker::marked(self.rows.clone(), self.selected);
        let needed = body
            .iter()
            .map(Line::width)
            .max()
            .unwrap_or(0)
            .max(Line::plain(&ui::get().picker.effort_title).width())
            + 4;
        let mut lines = boxed(
            Line::styled(&ui::get().picker.effort_title, Style::default().bold()),
            &body,
            needed.min(width),
        );
        lines.push(Line::styled(&ui::get().picker.effort_hint, Style::fg("muted")).truncate(width));
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels() -> Vec<String> {
        ["low", "medium", "high"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn prefers_the_current_level_then_default() {
        let efforts = levels();
        let picker = Picker::new(&efforts, Some("high"), Some("low"));
        assert_eq!(picker.selected, 2);
        let picker = Picker::new(&efforts, Some("missing"), Some("medium"));
        assert_eq!(picker.selected, 1);
    }

    #[test]
    fn enter_returns_the_selected_level_and_escape_closes() {
        let efforts = levels();
        let mut picker = Picker::new(&efforts, None, Some("low"));
        picker.key(Some(Action::Down));
        assert!(matches!(picker.key(Some(Action::Submit)), Step::Pick(id) if id == "medium"));
        assert!(matches!(picker.key(Some(Action::Escape)), Step::Close));
    }

    #[test]
    fn render_stays_within_the_terminal_width() {
        let picker = Picker::new(&levels(), None, None);
        assert!(picker.render(20).iter().all(|line| line.width() <= 20));
    }
}

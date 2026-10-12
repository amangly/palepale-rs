//! Reasoning choices for the current model; default clears the session override.

use crate::api::models::Model;
use crate::effort;
use crate::text::styled::{Line, Style};
use crate::text::template::fill;
use crate::tui::boxed::boxed;
use crate::tui::keymap::Action;
use crate::tui::{picker, ui};

pub struct Picker {
    ids: Vec<Option<String>>,
    rows: Vec<Line>,
    selected: usize,
    title: String,
}

pub enum Step {
    Stay,
    Pick(Option<String>),
    Close,
}

impl Picker {
    pub fn new(model: &Model, requested: Option<&str>) -> Self {
        let words = &ui::get().effort_picker;
        let mut ids = vec![None];
        let default = model
            .default_reasoning_effort
            .as_deref()
            .map(|id| format!(" ({id})"))
            .unwrap_or_default();
        let mut rows = vec![Line::plain(format!(
            "{}  {}{default}",
            words.default_label, words.default_summary
        ))];
        for id in model.effort_levels() {
            let Some(level) = effort::get(id) else {
                continue;
            };
            ids.push(Some(id.to_owned()));
            let mut row = Line::styled(id, Style::fg(&level.color));
            row.push(format!("  {}", level.summary), Style::fg("muted"));
            rows.push(row);
        }
        let selected = requested
            .and_then(|_| model.effort(requested))
            .and_then(|id| ids.iter().position(|choice| choice.as_deref() == Some(id)))
            .unwrap_or(0);
        Self {
            ids,
            rows,
            selected,
            title: fill(&words.title, &[("model", &model.id)]),
        }
    }

    pub fn key(&mut self, action: Option<Action>) -> Step {
        let last = self.ids.len().saturating_sub(1);
        match action {
            Some(Action::Up) => self.selected = self.selected.saturating_sub(1),
            Some(Action::Down) => self.selected = (self.selected + 1).min(last),
            Some(Action::Right | Action::Submit) => {
                return Step::Pick(self.ids[self.selected].clone());
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
            .max(crate::text::styled::width(&self.title))
            + 4;
        let mut lines = boxed(
            Line::styled(&self.title, Style::default().bold()),
            &body,
            needed.min(width),
        );
        lines.push(
            Line::styled(
                format!(" {}", ui::get().effort_picker.hint),
                Style::fg("muted"),
            )
            .truncate(width),
        );
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_follow_the_model_and_default_clears_the_override() {
        let mut model = crate::api::models::fixture().remove(0);
        model.reasoning_efforts = Some(vec!["low".into(), "xhigh".into()]);
        let mut picker = Picker::new(&model, Some("xhigh"));
        assert!(matches!(picker.key(Some(Action::Submit)), Step::Pick(Some(id)) if id == "xhigh"));
        let rendered = picker
            .render(100)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!rendered.contains("medium"));
        assert!(!rendered.contains("max  "));
        picker.key(Some(Action::Up));
        picker.key(Some(Action::Up));
        assert!(matches!(picker.key(Some(Action::Submit)), Step::Pick(None)));
        assert!(matches!(picker.key(Some(Action::Escape)), Step::Close));
        for width in [8, 30, 100] {
            assert!(
                picker
                    .render(width)
                    .iter()
                    .all(|line| line.width() <= width)
            );
        }
    }
}

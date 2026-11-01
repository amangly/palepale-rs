use crate::session::store::Summary;
use crate::text::styled::{Line, Style};
use crate::tui::{boxed::boxed, keymap::Action, picker};

pub struct Picker {
    sessions: Vec<Summary>,
    selected: usize,
}

pub enum Step {
    Stay,
    Close,
    Resume(String),
}

impl Picker {
    pub fn new(sessions: Vec<Summary>) -> Self {
        Self {
            sessions,
            selected: 0,
        }
    }
    pub fn key(&mut self, action: Option<Action>) -> Step {
        match action {
            Some(Action::Up) => self.selected = self.selected.saturating_sub(1),
            Some(Action::Down) => {
                self.selected = (self.selected + 1).min(self.sessions.len().saturating_sub(1))
            }
            Some(Action::Right | Action::Submit) => {
                return self
                    .sessions
                    .get(self.selected)
                    .map_or(Step::Close, |session| Step::Resume(session.id.clone()));
            }
            Some(Action::Left | Action::Escape | Action::CtrlC) => return Step::Close,
            _ => {}
        }
        Step::Stay
    }
    pub fn render(&self, width: usize) -> Vec<Line> {
        let words = crate::session::text::get();
        let rows = self.sessions.iter().map(|session| {
            Line::plain(format!(
                "{}  {}  {}",
                session.title,
                session.model,
                session.activity.strftime("%m-%d %H:%M UTC")
            ))
            .truncate(width.saturating_sub(6))
        });
        let mut body = picker::marked(rows, self.selected);
        if body.is_empty() {
            body.push(Line::plain(&words.empty).truncate(width.saturating_sub(4)));
        }
        let mut lines = boxed(
            Line::styled(&words.list_title, Style::default().bold()),
            &body,
            width,
        );
        lines.push(Line::styled(&words.picker_hint, Style::fg("muted")).truncate(width));
        lines
    }
}

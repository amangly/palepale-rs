//! Select terminal cells from a stable frame, including wide Unicode graphemes.
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::text::styled::Line;

#[derive(Default)]
pub struct Selection {
    rows: Vec<Line>,
    start: Option<(usize, usize)>,
    end: (usize, usize),
}

impl Selection {
    pub fn remember(&mut self, rows: &[Line], (width, height): (usize, usize)) {
        if !self.active() {
            self.rows = rows
                .iter()
                .take(height)
                .map(|row| row.truncate(width))
                .collect();
        }
    }

    pub fn active(&self) -> bool {
        self.start.is_some()
    }

    pub fn clear(&mut self) {
        self.start = None;
    }

    pub fn begin(&mut self, row: usize, column: usize) {
        self.start = (row < self.rows.len()).then_some((row, column));
        self.end = (row, column);
    }

    pub fn drag(&mut self, row: usize, column: usize) {
        self.end = (row.min(self.rows.len().saturating_sub(1)), column);
    }

    pub fn text(&self) -> Option<String> {
        self.start
            .filter(|start| *start != self.end)
            .map(|_| self.project().1)
    }

    pub fn frame(&self) -> Vec<Line> {
        self.project().0
    }

    fn project(&self) -> (Vec<Line>, String) {
        let Some(start) = self.start else {
            return (self.rows.clone(), String::new());
        };
        let (lo, hi) = (start.min(self.end), start.max(self.end));
        let mut rows = Vec::with_capacity(self.rows.len());
        let mut copied = Vec::new();
        for (row, line) in self.rows.iter().enumerate() {
            let mut output = Line::default();
            let mut text = String::new();
            let mut column = 0;
            for span in &line.spans {
                for grapheme in span.text.graphemes(true) {
                    let width = grapheme.width();
                    let selected = start != self.end
                        && row >= lo.0
                        && row <= hi.0
                        && (row != lo.0 || column + width > lo.1)
                        && (row != hi.0 || column <= hi.1);
                    let mut style = span.style.clone();
                    if selected {
                        style.reverse = !style.reverse;
                        text.push_str(grapheme);
                    }
                    output.push(grapheme, style);
                    column += width;
                }
            }
            rows.push(output);
            if row >= lo.0 && row <= hi.0 {
                copied.push(text.trim_end().to_owned());
            }
        }
        (rows, copied.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_frames_and_outside_drags_are_safe() {
        let mut s = Selection::default();
        s.begin(0, 0);
        s.drag(999, 999);
        assert!(!s.active());
        assert_eq!(s.text(), None);
        s.remember(
            &[Line::plain("one"), Line::plain(""), Line::plain("two  ")],
            (80, 24),
        );
        s.begin(0, 0);
        s.drag(999, 999);
        assert_eq!(s.text().as_deref(), Some("one\n\ntwo"));
        s.clear();
        assert_eq!(s.text(), None);
    }

    #[test]
    fn selection_is_direction_independent_and_preserves_unicode() {
        let mut s = Selection::default();
        s.remember(
            &[Line::plain("a界e\u{301}z"), Line::plain("next  ")],
            (80, 24),
        );
        s.begin(0, 2);
        s.drag(1, 1);
        assert_eq!(s.text().as_deref(), Some("界e\u{301}z\nne"));
        assert!(s.frame()[0].spans.iter().any(|span| span.style.reverse));
        s.remember(&[Line::plain("new streamed text")], (80, 24));
        assert_eq!(s.text().as_deref(), Some("界e\u{301}z\nne"));
        s.begin(1, 1);
        s.drag(0, 2);
        assert_eq!(s.text().as_deref(), Some("界e\u{301}z\nne"));
        s.clear();
        s.remember(&[Line::plain("fresh")], (80, 24));
        s.begin(0, 0);
        assert_eq!(s.text(), None);
        s.drag(0, 4);
        assert_eq!(s.text().as_deref(), Some("fresh"));
    }

    #[test]
    fn selection_copies_only_the_rendered_viewport() {
        let mut s = Selection::default();
        s.remember(
            &[Line::plain("long text"), Line::plain("offscreen")],
            (5, 1),
        );
        s.begin(0, 0);
        s.drag(99, 99);
        assert_eq!(s.text().as_deref(), Some("long…"));
        assert_eq!(s.frame().len(), 1);
    }
}

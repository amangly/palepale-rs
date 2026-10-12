//! The session welcome: identity, workspace, connection state, and useful keys.

use crate::mode;
use crate::text::styled::{Line, Style, width as text_width};
use crate::text::template::fill;
use crate::tui::ui;

#[derive(Clone)]
pub struct Info {
    pub version: &'static str,
    pub cwd: String,
    pub model: String,
    pub effort: Option<String>,
    pub mode_label: String,
    pub mode_color: String,
    pub backend_configured: bool,
    pub tools_supported: bool,
    pub braille: bool,
}

const ART: &str = include_str!("../../data/palepale-ascii.txt");

pub fn render(info: &Info, width: usize) -> Vec<Line> {
    let width = width.max(1);
    let rows: Vec<&str> = ART.lines().filter(|row| !row.trim().is_empty()).collect();
    let indent = rows
        .iter()
        .map(|row| row.len() - row.trim_start().len())
        .min()
        .unwrap_or(0);
    let rows: Vec<&str> = rows.iter().map(|row| row[indent..].trim_end()).collect();
    let art_width = rows.iter().map(|row| text_width(row)).max().unwrap_or(0);
    let words = text(info);
    let beside = width >= art_width + 3 + 28;
    let text_width = if beside { width - art_width - 3 } else { width };
    let words: Vec<Line> = words
        .into_iter()
        .flat_map(|line| line.wrap(text_width, &Line::default()))
        .collect();
    if width < art_width {
        return words;
    }
    let mut out = Vec::new();
    if beside {
        let top = rows.len().saturating_sub(words.len()) / 2;
        for index in 0..rows.len().max(top + words.len()) {
            let mut line =
                Line::styled(rows.get(index).copied().unwrap_or(""), Style::fg("accent"))
                    .padded(art_width, &Style::default());
            if let Some(word) = index.checked_sub(top).and_then(|i| words.get(i)) {
                line.push("   ", Style::default());
                line.extend(word.clone());
            }
            out.push(line);
        }
    } else {
        out.extend(
            rows.iter()
                .map(|row| Line::styled(*row, Style::fg("accent"))),
        );
        out.push(Line::default());
        out.extend(words);
    }
    out.push(Line::default());
    out
}

fn text(info: &Info) -> Vec<Line> {
    let words = ui::get();
    let mut name = Line::styled(env!("CARGO_PKG_NAME"), Style::fg("accent").bold());
    name.push(format!(" {}", info.version), Style::fg("muted"));
    let mut state = Line::styled(&info.mode_label, Style::fg(&info.mode_color).bold());
    let separator = if info.braille { " · " } else { " | " };
    state.push(separator, Style::fg("muted"));
    state.push(&info.model, Style::fg("muted"));
    if let Some(effort) = &info.effort {
        state.push(format!(" {effort}"), Style::fg("muted"));
    }
    let mut lines = vec![
        name,
        Line::styled(&words.text.welcome, Style::fg("text")),
        Line::styled(&info.cwd, Style::fg("muted")),
        Line::default(),
        state,
    ];
    if !info.backend_configured {
        lines.push(Line::styled(
            &crate::backend::text().unavailable,
            Style::fg("muted"),
        ));
    } else if !info.tools_supported {
        lines.push(Line::styled(&words.text.chat_only, Style::fg("muted")));
    }
    lines.push(Line::default());
    let modes = mode::all()
        .iter()
        .map(|m| m.label.as_str())
        .collect::<Vec<_>>()
        .join(if info.braille { " → " } else { " -> " });
    lines.extend(
        words
            .tips
            .iter()
            .map(|tip| Line::styled(fill(tip, &[("modes", &modes)]), Style::fg("muted"))),
    );
    lines.push(Line::default());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(braille: bool) -> Info {
        Info {
            version: "v0.2.0-1",
            cwd: "~/src/project".into(),
            model: crate::backend::text().disconnected.clone(),
            effort: None,
            mode_label: "MANUAL".into(),
            mode_color: "mode_manual".into(),
            backend_configured: false,
            tools_supported: true,
            braille,
        }
    }

    fn snapshot(lines: &[Line]) -> String {
        lines
            .iter()
            .map(Line::to_tagged)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn layouts_by_width() {
        insta::assert_snapshot!("welcome_wide", snapshot(&render(&info(true), 100)));
        insta::assert_snapshot!("welcome_narrow", snapshot(&render(&info(true), 40)));
        insta::assert_snapshot!("welcome_no_braille", snapshot(&render(&info(false), 100)));
    }

    #[test]
    fn supplied_art_uses_the_original_green_accent() {
        assert!(ART.is_ascii());
        assert!(
            render(&info(true), 100)
                .iter()
                .any(|line| line.to_tagged().contains("[accent]"))
        );
        assert_eq!(
            crate::text::theme::get().color("accent"),
            Some(crate::text::theme::Rgb(0, 220, 130))
        );
    }

    #[test]
    fn offline_welcome_never_promises_a_model_or_account() {
        let text = snapshot(&render(&info(true), 100));
        assert!(text.contains("Backend not configured"));
        assert!(!text.contains("/login") && !text.contains("demo-2"));
        for width in [1, 10, 24, 40, 100] {
            assert!(
                render(&info(false), width)
                    .iter()
                    .all(|line| line.width() <= width)
            );
        }
    }

    #[test]
    fn chat_only_welcome_explains_the_tool_limit() {
        let info = Info {
            backend_configured: true,
            tools_supported: false,
            ..info(true)
        };
        assert!(snapshot(&render(&info, 100)).contains("Chat only"));
        for width in [1, 10, 24, 40, 100] {
            assert!(
                render(&info, width)
                    .iter()
                    .all(|line| line.width() <= width)
            );
        }
    }
}

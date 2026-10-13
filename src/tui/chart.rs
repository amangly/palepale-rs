//! Labeled token bars share a scale; narrow terminals keep the numbers.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::text::styled::{Line, Style, width as text_width};
use crate::text::units;

#[derive(Deserialize)]
pub struct Config {
    max_cells: usize,
    filled: String,
    empty: String,
    pub input_color: String,
    pub output_color: String,
}

pub static CONFIG: LazyLock<Config> =
    LazyLock::new(|| toml::from_str(include_str!("../../data/chart.toml")).expect("chart config"));

pub struct Bar<'a> {
    pub label: &'a str,
    pub value: u64,
    pub color: &'a str,
}

pub fn render(bars: &[Bar<'_>], width: usize) -> Vec<Line> {
    let labels = bars
        .iter()
        .map(|bar| text_width(bar.label))
        .max()
        .unwrap_or(0);
    let values: Vec<String> = bars.iter().map(|bar| units::tokens(bar.value)).collect();
    let digits = values
        .iter()
        .map(|value| text_width(value))
        .max()
        .unwrap_or(0);
    let cells = width
        .saturating_sub(labels + digits + 4)
        .min(CONFIG.max_cells);
    let max = bars.iter().map(|bar| bar.value).max().unwrap_or(0);
    bars.iter()
        .zip(values)
        .map(|(bar, value)| {
            if cells < 4 {
                let label_width = width.saturating_sub(text_width(&value) + 2);
                if label_width == 0 {
                    return Line::plain(value).truncate(width);
                }
                let mut line = Line::styled(bar.label, Style::fg("muted")).truncate(label_width);
                line.push(": ", Style::fg("muted"));
                line.push(value, Style::default());
                return line.truncate(width);
            }
            let filled = (u128::from(bar.value) * cells as u128)
                .checked_div(u128::from(max))
                .unwrap_or(0) as usize;
            let mut line =
                Line::styled(bar.label, Style::fg("muted")).padded(labels + 2, &Style::default());
            line.push(CONFIG.filled.repeat(filled), Style::fg(bar.color));
            line.push(
                CONFIG.empty.repeat(cells - filled),
                Style::fg("muted").dim(),
            );
            line.push(
                format!("  {}{value}", " ".repeat(digits - text_width(&value))),
                Style::default(),
            );
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_scale_handles_zero_extreme_counts_and_narrow_columns() {
        for (input, output) in [(0, 0), (12_000, 3_000), (u64::MAX, u64::MAX / 2)] {
            let bars = [
                Bar {
                    label: "Input",
                    value: input,
                    color: "accent",
                },
                Bar {
                    label: "Output",
                    value: output,
                    color: "chart_output",
                },
            ];
            for width in [0, 1, 12, 20, 40, 100] {
                let rows = render(&bars, width);
                assert!(rows.iter().all(|row| row.width() <= width));
                if width >= 12 {
                    for (row, value) in rows.iter().zip([input, output]) {
                        let number = units::tokens(value);
                        if text_width(&number) <= width {
                            assert!(row.text().ends_with(&number));
                        }
                    }
                }
                if width >= 40 {
                    let filled: Vec<usize> = rows
                        .iter()
                        .map(|row| row.text().matches(&CONFIG.filled).count())
                        .collect();
                    if input == 0 {
                        assert_eq!(filled, [0, 0]);
                    } else {
                        assert!(filled[0] > filled[1] && filled[1] > 0);
                    }
                    assert!(rows[0].text().ends_with(&units::tokens(input)));
                    assert!(rows[1].text().ends_with(&units::tokens(output)));
                }
            }
        }
    }
}

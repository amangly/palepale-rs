//! The supplied SVG, rasterized ahead of time into compact colored terminal cells.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::text::styled::{Line, Style};

#[derive(Deserialize)]
struct Config {
    width: usize,
    height: usize,
    alpha_threshold: u8,
    upper: String,
    lower: String,
    full: String,
    empty: String,
}

static CONFIG: LazyLock<Config> =
    LazyLock::new(|| toml::from_str(include_str!("../../data/logo.toml")).expect("logo config"));
const PIXELS: &[u8] = include_bytes!("../../data/palepale-logo.rgba");
static ROWS: LazyLock<Vec<Line>> = LazyLock::new(render);

pub fn rows() -> Vec<Line> {
    ROWS.clone()
}

fn render() -> Vec<Line> {
    let config = &*CONFIG;
    assert_eq!(PIXELS.len(), config.width * config.height * 4);
    assert_eq!(config.height % 2, 0);
    let pixel = |x, y| {
        let start = (y * config.width + x) * 4;
        let rgba = &PIXELS[start..start + 4];
        (rgba[3] >= config.alpha_threshold).then(|| [rgba[0], rgba[1], rgba[2]])
    };
    (0..config.height)
        .step_by(2)
        .map(|y| {
            let mut line = Line::default();
            for x in 0..config.width {
                let (symbol, color) = match (pixel(x, y), pixel(x, y + 1)) {
                    (Some(top), Some(bottom)) => (
                        &config.full,
                        Some(std::array::from_fn(|i| {
                            ((u16::from(top[i]) + u16::from(bottom[i])) / 2) as u8
                        })),
                    ),
                    (Some(top), None) => (&config.upper, Some(top)),
                    (None, Some(bottom)) => (&config.lower, Some(bottom)),
                    (None, None) => (&config.empty, None),
                };
                let style = color.map_or_else(Style::default, |[red, green, blue]| {
                    Style::fg(&format!("#{red:02x}{green:02x}{blue:02x}"))
                });
                line.push(symbol, style);
            }
            line
        })
        .filter(|row| !row.text().trim().is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::theme::{self, Depth};

    #[test]
    fn svg_pixels_keep_the_symmetric_shape_cutouts_and_gradient_in_each_color_mode() {
        let rows = rows();
        assert_eq!(rows.len(), 14);
        assert!(rows.iter().all(|row| row.width() == 32));
        let mut colors = std::collections::BTreeSet::new();
        let mut cutouts = 0;
        for row in &rows {
            let cells: Vec<char> = row.text().chars().collect();
            // Antialiasing can put a mirrored edge on opposite sides of the alpha threshold.
            assert!(
                cells
                    .iter()
                    .zip(cells.iter().rev())
                    .filter(|(a, b)| a != b)
                    .count()
                    <= 4
            );
            assert_eq!(&cells[15..17], &[' ', ' ']);
            cutouts += usize::from(cells[6] == ' ' && cells[13] != ' ');
            for span in &row.spans {
                if let Some(color) = &span.style.fg {
                    colors.insert(color.clone());
                }
            }
            for depth in [Depth::TrueColor, Depth::Ansi256, Depth::None] {
                let ansi = row.to_ansi(theme::get(), depth);
                if depth == Depth::None {
                    assert!(!ansi.contains("38;"));
                }
            }
        }
        assert!(cutouts > 0 && colors.len() > 8);
        assert!(rows.iter().any(|row| row.text().contains('▀')));
        assert!(rows.iter().any(|row| row.text().contains('▄')));
        assert_eq!(
            rows.iter()
                .map(|row| row.text().trim_end().to_owned())
                .collect::<Vec<_>>(),
            include_str!("../../data/palepale-logo-cells.txt")
                .lines()
                .collect::<Vec<_>>()
        );
    }
}

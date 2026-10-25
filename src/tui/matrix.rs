//! ttfx animates only the small logo; the session keeps handling input.

use std::sync::LazyLock;

use serde::Deserialize;
use ttfx::effects::matrix::MatrixConfig;
use ttfx::fx::Engine;
use ttfx::fx::effects::matrix::Matrix;
use ttfx::fx::run::Effect;
use ttfx::utils::graphics::{Color, GradientDirection};

use crate::io::term;
use crate::text::styled::{Line, Style};
use crate::text::theme::{self, Depth};
use crate::tui::welcome;

#[derive(Deserialize)]
struct Config {
    max_frames: usize,
    virtual_frame_rate: i64,
    rain_time: i64,
    rain_symbols: String,
    highlight_color: String,
    rain_colors: Vec<String>,
    final_colors: Vec<String>,
    fall_delay: (i64, i64),
    column_delay: (i64, i64),
    symbol_swap_chance: f64,
    color_swap_chance: f64,
    resolve_delay: i64,
    gradient_steps: Vec<i64>,
    gradient_frames: i64,
}

static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/matrix.toml")).expect("matrix config")
});

pub struct Intro {
    effect: Matrix,
    engine: Engine,
    width: usize,
    height: usize,
    frames: usize,
}

impl Intro {
    #[cfg(test)]
    pub fn frame_count(&self) -> usize {
        self.frames
    }

    pub fn new(width: usize, height: usize, seed: u64) -> Option<Self> {
        let logo = welcome::logo();
        let columns = logo.iter().map(Line::width).max()?;
        if width < columns || height < logo.len() + 5 {
            return None;
        }
        let config = &*CONFIG;
        let color = |value: &str| Color::from_hex(value).expect("matrix color");
        let mut effect = Matrix::new(MatrixConfig {
            highlight_color: color(&config.highlight_color),
            rain_color_gradient: config
                .rain_colors
                .iter()
                .map(|value| color(value))
                .collect(),
            rain_symbols: config
                .rain_symbols
                .chars()
                .map(|value| value.to_string())
                .collect(),
            rain_fall_delay_range: config.fall_delay,
            rain_column_delay_range: config.column_delay,
            rain_time: config.rain_time,
            symbol_swap_chance: config.symbol_swap_chance,
            color_swap_chance: config.color_swap_chance,
            resolve_delay: config.resolve_delay,
            final_gradient_stops: config
                .final_colors
                .iter()
                .map(|value| color(value))
                .collect(),
            final_gradient_steps: config.gradient_steps.clone(),
            final_gradient_frames: config.gradient_frames,
            final_gradient_direction: GradientDirection::Vertical,
        });
        let mut engine = term::effect_engine(
            &logo
                .iter()
                .map(|row| row.to_ansi(theme::get(), Depth::TrueColor))
                .collect::<Vec<_>>()
                .join("\n"),
            columns,
            logo.len(),
            seed,
            config.virtual_frame_rate,
        )
        .ok()?;
        effect.build(&mut engine).ok()?;
        Some(Self {
            effect,
            engine,
            width: columns,
            height: logo.len(),
            frames: 0,
        })
    }

    pub fn advance(&mut self) -> Option<Vec<Line>> {
        if self.frames >= CONFIG.max_frames || !self.effect.next_frame(&mut self.engine) {
            return None;
        }
        self.frames += 1;
        self.engine.clock.advance_frame();
        let mut cells = vec![None; self.width * self.height];
        for slot in 0..self.engine.char_count() as u32 {
            if !self.engine.is_visible(slot) {
                continue;
            }
            let coord = self.engine.coord(slot);
            if coord.column < 1
                || coord.column > self.width as i64
                || coord.row < 1
                || coord.row > self.height as i64
            {
                continue;
            }
            let index = (self.height - coord.row as usize) * self.width + coord.column as usize - 1;
            if cells[index].is_none_or(|old| self.engine.layer(slot) >= self.engine.layer(old)) {
                cells[index] = Some(slot);
            }
        }
        Some(
            cells
                .chunks(self.width)
                .map(|row| {
                    let mut line = Line::default();
                    for slot in row {
                        if let Some(slot) = slot {
                            let visual = self.engine.visual_info(self.engine.current_visual(*slot));
                            let style = visual.fg.map_or_else(
                                || Style::fg("accent"),
                                |color| {
                                    let (red, green, blue) = color.rgb_ints();
                                    Style::fg(&format!("#{red:02x}{green:02x}{blue:02x}"))
                                },
                            );
                            let style = match visual.bg {
                                Some(color) => {
                                    let (red, green, blue) = color.rgb_ints();
                                    style.on(&format!("#{red:02x}{green:02x}{blue:02x}"))
                                }
                                None => style,
                            };
                            line.push(self.engine.symbol(visual.sym), style);
                        } else {
                            line.push(" ", Style::default());
                        }
                    }
                    line
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rain_is_bounded_and_fits_the_logo_on_small_and_large_terminals() {
        assert!(Intro::new(12, 24, 1).is_none());
        assert!(Intro::new(100, 5, 1).is_none());
        for seed in [1, 2, 3] {
            let mut intro = Intro::new(100, 24, seed).unwrap();
            let mut changed = false;
            let mut last = Vec::new();
            while let Some(rows) = intro.advance() {
                assert_eq!(rows.len(), welcome::logo().len());
                assert!(rows.iter().all(|row| row.width() == intro.width));
                for row in &rows {
                    row.to_ansi(
                        crate::text::theme::get(),
                        crate::text::theme::Depth::TrueColor,
                    );
                }
                changed |= rows.iter().any(|row| row.text().contains('Z'));
                last = rows;
            }
            assert!(changed, "Matrix rain never appeared");
            assert!(intro.frames <= CONFIG.max_frames);
            assert_eq!(
                visible_pixels(&last),
                visible_pixels(&welcome::logo()),
                "Matrix did not settle to the logo"
            );
        }
    }

    fn visible_pixels(rows: &[Line]) -> Vec<(usize, usize, char, Option<String>)> {
        let mut pixels = Vec::new();
        for (row, line) in rows.iter().enumerate() {
            for (column, (glyph, color)) in line
                .spans
                .iter()
                .flat_map(|span| {
                    span.text
                        .chars()
                        .map(|glyph| (glyph, span.style.fg.clone()))
                })
                .enumerate()
            {
                if glyph != ' ' {
                    pixels.push((row, column, glyph, color));
                }
            }
        }
        pixels
    }
}

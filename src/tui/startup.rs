//! Startup replacement finishes before any conversation or terminal session starts.
use std::sync::LazyLock;

use futures_util::StreamExt;

use crate::io::{clock, proc, term};
use crate::limits;
use crate::paths::Paths;
use crate::text::styled::{Line, Style};
use crate::text::{template::fill, theme};
use crate::tui::{keymap, keymap::Action, ui};
use crate::update::{self, Outcome, Progress};
use serde::Deserialize;

#[derive(Deserialize)]
struct Words {
    checking: String,
    downloading: String,
    verifying: String,
    installing: String,
    current: String,
    opening: String,
    failed: String,
    cancelled: String,
    hint: String,
}
static WORDS: LazyLock<Words> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        startup: Words,
    }
    toml::from_str::<File>(include_str!("../../data/update.toml"))
        .expect("update text")
        .startup
});

pub async fn update(paths: &Paths, enabled: bool) -> Option<std::process::ExitCode> {
    if !update::due(&update::load(paths), clock::now(), enabled) {
        return None;
    }
    let exe = proc::current_exe().ok()?;
    let raw = match term::Raw::enter() {
        Ok(raw) => raw,
        Err(error) => {
            finish(&fill(&WORDS.failed, &[("error", &error.to_string())]));
            return None;
        }
    };
    let mut inputs = Box::pin(term::inputs());
    let (tx, mut rx) = tokio::sync::watch::channel(Progress::Checking);
    let emit = |progress| {
        // Short phases can finish in one poll; paint them before replacement and reopening.
        if matches!(progress, Progress::Verifying | Progress::Installing) {
            draw(&progress, 0);
        }
        tx.send_replace(progress);
    };
    let operation = update::with_progress(paths, &emit);
    tokio::pin!(operation);
    let mut tick = tokio::time::interval(limits::get().tui.frame_ms.ms());
    let mut frame = 0;
    let result = loop {
        draw(&rx.borrow(), frame);
        frame += 1;
        tokio::select! {
            result = &mut operation => break Some(result),
            _ = term::interrupted() => break None,
            Some(input) = inputs.next() => {
                if let term::Input::Key(key) = input
                    && keymap::get().action(&key) == Some(Action::CtrlC)
                {
                    break None;
                }
            },
            _ = rx.changed() => {},
            _ = tick.tick() => {},
        }
    };
    drop(inputs);
    drop(raw);
    match result {
        Some(Ok(Outcome::Updated(change))) => {
            finish(&fill(&WORDS.opening, &[("tag", &change.to)]));
            match proc::relaunch(&exe).await {
                Ok(code) => Some(code),
                Err(error) => {
                    finish(&fill(&WORDS.failed, &[("error", &error.to_string())]));
                    None
                }
            }
        }
        Some(Ok(Outcome::UpToDate(tag))) => {
            finish(&fill(&WORDS.current, &[("tag", &tag)]));
            None
        }
        Some(Err(error)) => {
            finish(&fill(&WORDS.failed, &[("error", &error.to_string())]));
            None
        }
        None => {
            finish(&WORDS.cancelled);
            None
        }
    }
}

fn draw(progress: &Progress, frame: usize) {
    let spinner = &ui::get().spinner;
    let mut line = Line::styled(
        format!(" {} ", spinner[frame % spinner.len()]),
        Style::fg("accent"),
    );
    match progress {
        Progress::Checking => line.push(&WORDS.checking, Style::default()),
        Progress::Downloading { received, total } => {
            line.push(&WORDS.downloading, Style::default());
            if let Some(total) = total.filter(|total| *total > 0) {
                let pct = received
                    .saturating_mul(100)
                    .checked_div(total)
                    .unwrap_or(0)
                    .min(100);
                let filled = pct as usize / 5;
                line.push(
                    format!(
                        " [{}{}] {pct}%",
                        "━".repeat(filled),
                        "─".repeat(20 - filled)
                    ),
                    Style::fg("accent"),
                );
            } else {
                line.push(
                    format!(" {:.1} MB", *received as f64 / 1_000_000.0),
                    Style::fg("accent"),
                );
            }
        }
        Progress::Verifying => line.push(&WORDS.verifying, Style::default()),
        Progress::Installing => line.push(&WORDS.installing, Style::default()),
    }
    line.push(format!(" · {}", WORDS.hint), Style::fg("muted"));
    write_line(line);
}

fn write_line(line: Line) {
    let width = term::size().0.saturating_sub(1).max(1);
    term::out(&format!(
        "\r\x1b[2K{}",
        line.truncate(width)
            .to_ansi(theme::get(), term::color_depth())
    ));
}

fn finish(text: &str) {
    write_line(Line::styled(format!(" {text}"), Style::fg("muted")));
    term::out("\n");
}

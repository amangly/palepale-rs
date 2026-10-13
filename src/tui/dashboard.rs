//! Session state and server accounting stay separate, and missing data stays unavailable.
use crate::agent::message::Usage;
use crate::api::account::{Me, Overview};
use crate::api::error::ApiError;
use crate::commands::Session;
use crate::io::term::{KeyCode, KeyEvent};
use crate::text::styled::{Line, Style};
use crate::text::{template::fill, units};
use crate::tui::{boxed::boxed, keymap::Action};
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
struct Words {
    title: String,
    tabs: Vec<String>,
    hint: String,
    loading: String,
    signed_out: String,
    offline: String,
    unavailable: String,
    no_context: String,
    default_effort: String,
    ready: String,
    paused: String,
    session_cost: String,
    usage_scope: String,
    usage_failed: String,
    labels: Vec<String>,
}
static WORDS: LazyLock<Words> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/dashboard.toml")).expect("dashboard text")
});

pub struct Dashboard {
    pub id: u64,
    pub revision: u64,
    tab: usize,
    offset: usize,
    loading: bool,
    me: Option<Me>,
    usage: Option<Overview>,
    error: Option<String>,
    usage_error: Option<String>,
}

pub enum Step {
    Stay,
    Close,
    Refresh,
}

pub struct Info<'a> {
    pub session: &'a Session,
    pub usage: Usage,
    pub requests: u64,
    pub project: &'a str,
    pub configured: bool,
    pub logged_in: bool,
}

impl Dashboard {
    pub fn new(id: u64, revision: u64, loading: bool) -> Self {
        Self {
            id,
            revision,
            tab: 0,
            offset: 0,
            loading,
            me: None,
            usage: None,
            error: None,
            usage_error: None,
        }
    }

    pub fn finished(&mut self, me: Result<Me, ApiError>, usage: Result<Overview, ApiError>) {
        self.loading = false;
        match me {
            Ok(me) => self.me = Some(me),
            Err(error) => {
                self.me = None;
                self.error = Some(error.to_string());
            }
        }
        match usage {
            Ok(usage) => self.usage = Some(usage),
            Err(error) => {
                self.usage = None;
                self.usage_error = Some(error.to_string());
            }
        }
    }

    pub fn key(&mut self, action: Option<Action>, key: &KeyEvent) -> Step {
        match action {
            Some(Action::Escape | Action::Quit) => return Step::Close,
            Some(Action::Tab | Action::Right) => {
                self.tab = (self.tab + 1) % WORDS.tabs.len();
                self.offset = 0;
            }
            Some(Action::Left) => {
                self.tab = (self.tab + WORDS.tabs.len() - 1) % WORDS.tabs.len();
                self.offset = 0;
            }
            Some(Action::Up) => self.offset = self.offset.saturating_sub(1),
            Some(Action::Down) => self.offset = self.offset.saturating_add(1),
            _ if key.code == KeyCode::Char('r') && !self.loading => return Step::Refresh,
            _ => {}
        }
        Step::Stay
    }

    pub fn refreshing(&mut self, id: u64, revision: u64, loading: bool) {
        self.id = id;
        if revision != self.revision {
            self.me = None;
            self.usage = None;
        }
        self.revision = revision;
        self.loading = loading;
        self.error = None;
        self.usage_error = None;
    }

    pub fn render(&mut self, info: &Info<'_>, width: usize, max_rows: usize) -> Vec<Line> {
        let words = &*WORDS;
        let mut rows = Vec::new();
        let mut tabs = Line::default();
        for (index, tab) in words.tabs.iter().enumerate() {
            tabs.push(
                format!(" {} ", tab),
                if index == self.tab {
                    Style::fg("accent").bold()
                } else {
                    Style::fg("muted")
                },
            );
        }
        rows.push(tabs);
        let mut body = Vec::new();
        if self.tab == 0 {
            body.push(row(0, crate::release::version()));
            body.push(row(
                1,
                if info.configured {
                    &crate::backend::text().connected
                } else {
                    &crate::backend::text().disconnected
                },
            ));
            body.push(row(2, &info.session.model));
            body.push(row(
                3,
                info.session
                    .effort
                    .as_deref()
                    .unwrap_or(&words.default_effort),
            ));
            body.push(row(4, &info.session.mode));
            body.push(match info.session.context_pct {
                Some(pct) => meter(5, pct, width),
                None => row(5, &words.no_context),
            });
            body.push(row(6, &info.requests.to_string()));
            body.push(row(
                7,
                &format!(
                    "{} / {}",
                    units::tokens(info.usage.input),
                    units::tokens(info.usage.output)
                ),
            ));
            body.push(row(
                8,
                &format!(
                    "{} / {}",
                    units::tokens(info.usage.cached),
                    units::tokens(info.usage.reasoning)
                ),
            ));
            body.push(label(
                &words.session_cost,
                &units::usd_micros(info.usage.cost_micros),
            ));
        } else {
            body.push(Line::styled(&words.usage_scope, Style::fg("muted")));
        }
        if info.configured && info.logged_in {
            if let Some(me) = &self.me {
                body.push(row(11, &format!("…{}", me.account_hint)));
                body.push(row(12, &units::usd_micros(me.available())));
                body.push(Line::styled(
                    if me.can_spend {
                        &words.ready
                    } else {
                        &words.paused
                    },
                    Style::fg(if me.can_spend { "done" } else { "warn" }),
                ));
                if self.tab == 1 {
                    body.push(row(
                        13,
                        &me.reserved_microdollars
                            .map(units::usd_micros)
                            .unwrap_or_else(|| words.unavailable.clone()),
                    ));
                    if let (Some(budget), Some(used)) =
                        (me.token_budget.filter(|v| *v > 0), me.used_tokens)
                    {
                        let total = used.saturating_add(me.reserved_tokens.unwrap_or(0));
                        body.push(meter(21, total.saturating_mul(100) / budget, width));
                    }
                }
            }
            if self.tab == 1 {
                if let Some(usage) = &self.usage {
                    body.push(row(14, &units::usd_micros(usage.spent_microdollars)));
                    body.push(row(15, &usage.confirmed_requests.to_string()));
                    body.push(row(16, &usage.unknown_requests.to_string()));
                    body.push(row(17, &units::tokens(usage.prompt_tokens)));
                    body.push(row(18, &units::tokens(usage.completion_tokens)));
                    body.push(meter(
                        19,
                        usage
                            .cached_tokens
                            .saturating_mul(100)
                            .checked_div(usage.prompt_tokens)
                            .unwrap_or(0),
                        width,
                    ));
                    body.push(row(20, &units::tokens(usage.reasoning_tokens)));
                }
                if let Some(error) = &self.usage_error {
                    body.push(Line::styled(
                        fill(&words.usage_failed, &[("error", error)]),
                        Style::fg("warn"),
                    ));
                }
            }
            if let Some(error) = &self.error {
                body.push(Line::styled(error, Style::fg("warn")));
            }
            if self.loading {
                body.push(Line::styled(&words.loading, Style::fg("accent")));
            }
        } else {
            body.push(Line::styled(
                if info.configured {
                    &words.signed_out
                } else {
                    &words.offline
                },
                Style::fg("muted"),
            ));
        }
        if self.tab == 0 {
            body.push(row(9, info.project));
            body.push(row(10, &info.session.file));
        }
        let available = max_rows.saturating_sub(4).max(1);
        self.offset = self.offset.min(body.len().saturating_sub(available));
        let offset = self.offset;
        rows.extend(body.into_iter().skip(offset).take(available));
        let mut lines = boxed(
            Line::styled(&words.title, Style::default().bold()),
            &rows,
            width,
        );
        lines.push(Line::styled(&words.hint, Style::fg("muted")).truncate(width));
        lines
    }
}

fn label(name: &str, value: &str) -> Line {
    let mut line = Line::styled(format!("{name}: "), Style::fg("muted"));
    line.push(value, Style::default());
    line
}
fn row(index: usize, value: &str) -> Line {
    label(&WORDS.labels[index], value)
}
fn meter(index: usize, pct: u64, width: usize) -> Line {
    let cells = width
        .saturating_sub(WORDS.labels[index].len() + 14)
        .clamp(4, 20);
    let filled = pct.min(100) as usize * cells / 100;
    let mut line = row(index, "");
    line.push(
        format!(
            "{}{} {pct}%",
            "━".repeat(filled),
            "─".repeat(cells - filled)
        ),
        Style::fg(if pct >= 90 { "warn" } else { "accent" }),
    );
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::ErrorInfo;
    use crate::io::term::KeyModifiers;

    #[test]
    fn cards_fit_small_terminals_and_unavailable_usage_stays_unavailable() {
        let session = Session {
            model: "demo".into(),
            effort: None,
            mode: "MANUAL".into(),
            context_pct: Some(120),
            file: "/saved/session".into(),
        };
        let info = Info {
            session: &session,
            usage: Usage::default(),
            requests: 0,
            project: "/project",
            configured: true,
            logged_in: true,
        };
        let mut card = Dashboard::new(1, 0, true);
        card.finished(
            Ok(Me {
                account_hint: "1234".into(),
                balance_microdollars: 12300000,
                ..Me::default()
            }),
            Err(ErrorInfo::local("not_found", "No usage endpoint").into()),
        );
        for (width, height) in [(12, 6), (32, 12), (100, 24)] {
            let lines = card.render(&info, width, height);
            assert!(lines.len() <= height);
            assert!(lines.iter().all(|line| line.width() <= width));
        }
        card.key(
            Some(Action::Tab),
            &KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        let text = card
            .render(&info, 100, 24)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Usage unavailable") && text.contains("$12.30"));
        assert!(!text.contains("Charged: $0.00") && !text.contains("Token budget used:"));
        card.refreshing(2, 1, true);
        let text = card
            .render(&info, 100, 24)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("1234") && !text.contains("$12.30"));
    }
}

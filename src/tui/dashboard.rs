//! Session state and server accounting stay separate, and missing data stays unavailable.
use crate::agent::message::Usage;
use crate::api::account::{Me, Overview};
use crate::api::error::ApiError;
use crate::commands::Session;
use crate::io::term::{KeyCode, KeyEvent};
use crate::text::styled::{Line, Style};
use crate::text::{template::fill, units};
use crate::tui::{boxed::boxed, chart, keymap::Action};
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
struct Words {
    title: String,
    max_width: usize,
    separator: String,
    tabs: Vec<String>,
    hint: String,
    loading: String,
    signed_out: String,
    offline: String,
    unavailable: String,
    no_context: String,
    paused: String,
    session_cost: String,
    usage_scope: String,
    usage_failed: String,
    labels: Labels,
}

#[derive(Deserialize)]
struct Labels {
    model: String,
    mode: String,
    context: String,
    requests: String,
    account: String,
    available: String,
    reserved: String,
    charged: String,
    unresolved: String,
    input: String,
    output: String,
    cached: String,
    reasoning: String,
    budget: String,
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
        let labels = &words.labels;
        let width = width.min(words.max_width);
        let inner = width.saturating_sub(4);
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
            body.push(label(&labels.model, &info.session.model));
            body.push(joined(
                label(&labels.mode, &info.session.mode),
                label(&labels.requests, &info.requests.to_string()),
            ));
            if let Some(profile) = &info.session.agent_profile {
                body.push(label(&crate::commands::godmode::text().label, profile));
            }
            body.push(match info.session.context_pct {
                Some(pct) => chart::meter(&labels.context, pct, 100, inner),
                None => label(&labels.context, &words.no_context),
            });
            body.extend(token_chart(
                info.usage.input,
                info.usage.output,
                info.usage.cached,
                info.usage.reasoning,
                inner,
            ));
            body.push(label(
                &words.session_cost,
                &units::usd_micros(info.usage.cost_micros),
            ));
        }
        if info.configured && info.logged_in {
            if let Some(me) = &self.me {
                let available = label(&labels.available, &units::usd_micros(me.available()));
                if self.tab == 1 {
                    body.push(joined(
                        label(&labels.account, &format!("…{}", me.account_hint)),
                        Line::styled(&words.usage_scope, Style::fg("muted")),
                    ));
                    body.push(joined(
                        available,
                        label(
                            &labels.reserved,
                            &me.reserved_microdollars
                                .map(units::usd_micros)
                                .unwrap_or_else(|| words.unavailable.clone()),
                        ),
                    ));
                    if let (Some(budget), Some(used)) =
                        (me.token_budget.filter(|v| *v > 0), me.used_tokens)
                    {
                        let total = used.saturating_add(me.reserved_tokens.unwrap_or(0));
                        body.push(chart::meter(&labels.budget, total, budget, inner));
                    }
                } else {
                    body.push(available);
                }
                if !me.can_spend {
                    body.push(Line::styled(&words.paused, Style::fg("warn")));
                }
            }
            if self.tab == 1 {
                if let Some(usage) = &self.usage {
                    let mut totals = joined(
                        label(
                            &labels.charged,
                            &units::usd_micros(usage.spent_microdollars),
                        ),
                        label(&labels.requests, &usage.confirmed_requests.to_string()),
                    );
                    if usage.unknown_requests > 0 {
                        totals = joined(
                            totals,
                            label(&labels.unresolved, &usage.unknown_requests.to_string()),
                        );
                    }
                    body.push(totals);
                    body.extend(token_chart(
                        usage.prompt_tokens,
                        usage.completion_tokens,
                        usage.cached_tokens,
                        usage.reasoning_tokens,
                        inner,
                    ));
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
        let body: Vec<Line> = body
            .into_iter()
            .flat_map(|line| line.wrap(inner.max(1), &Line::default()))
            .collect();
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
fn joined(mut left: Line, right: Line) -> Line {
    left.push(&WORDS.separator, Style::fg("muted"));
    left.extend(right);
    left
}
fn token_chart(input: u64, output: u64, cached: u64, reasoning: u64, width: usize) -> Vec<Line> {
    let mut rows = chart::render(
        &[
            chart::Bar {
                label: &WORDS.labels.input,
                value: input,
                color: &chart::CONFIG.input_color,
            },
            chart::Bar {
                label: &WORDS.labels.output,
                value: output,
                color: &chart::CONFIG.output_color,
            },
        ],
        width,
    );
    let pct = u128::from(cached) * 100 / u128::from(input.max(1));
    rows.push(
        joined(
            label(
                &WORDS.labels.cached,
                &format!("{} ({pct}%)", units::tokens(cached)),
            ),
            label(&WORDS.labels.reasoning, &units::tokens(reasoning)),
        )
        .truncate(width),
    );
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::error::ErrorInfo;
    use crate::io::term::KeyModifiers;

    #[test]
    fn populated_cards_stay_compact_and_keep_charges_and_small_token_counts_visible() {
        let session = Session {
            model: "gpt-6-astra".into(),
            mode: "MANUAL".into(),
            agent_profile: None,
            context_pct: Some(8),
        };
        let info = Info {
            session: &session,
            usage: Usage::default(),
            requests: 0,
            configured: true,
            logged_in: true,
        };
        let mut card = Dashboard::new(1, 0, false);
        card.finished(
            Ok(Me {
                account_hint: "5594".into(),
                balance_microdollars: 4_947_622,
                reserved_microdollars: Some(33_768),
                can_spend: true,
                token_budget: Some(1_000_000),
                used_tokens: Some(1),
                ..Me::default()
            }),
            Ok(Overview {
                confirmed_requests: 13,
                unknown_requests: 1,
                spent_microdollars: 18_610,
                prompt_tokens: 88_900,
                completion_tokens: 1_500,
                cached_tokens: 32_300,
                reasoning_tokens: 341,
            }),
        );
        for tab in [0, 1] {
            card.tab = tab;
            let rows = card.render(&info, 180, 40);
            assert!(rows.len() <= 12 && rows.iter().all(|row| row.width() <= 64));
            let text = rows.iter().map(Line::text).collect::<Vec<_>>().join("\n");
            assert!(text.contains("Available: $4.947622"));
            if tab == 1 {
                assert!(text.contains("Charged: $0.01861") && text.contains("Unresolved: 1"));
                assert!(text.contains("<1%") && text.contains("88.9K") && text.contains("1.5K"));
                let output = rows
                    .iter()
                    .find(|row| row.text().contains("│ Output "))
                    .unwrap();
                assert_eq!(output.text().matches('█').count(), 1);
            }
        }
    }

    #[test]
    fn cards_fit_small_terminals_and_unavailable_usage_stays_unavailable() {
        let session = Session {
            model: "demo".into(),
            mode: "MANUAL".into(),
            agent_profile: None,
            context_pct: Some(120),
        };
        let info = Info {
            session: &session,
            usage: Usage::default(),
            requests: 0,
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
        assert!(!text.contains("Charged: $0.00") && !text.contains("Budget"));
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

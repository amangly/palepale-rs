//! The system prompt, ordered stable-first so vendors that cache prompt prefixes can reuse it.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::api::models::Model;
use crate::io;
use crate::paths::Paths;
use crate::text::template;

const BASE: &str = include_str!("../../data/prompts/system.md");
const CHAT_ONLY: &str = include_str!("../../data/prompts/chat_only.md");
const ENV: &str = include_str!("../../data/prompts/env.md");
const EXTENSIONS: &str = include_str!("../../data/prompts/extensions.md");
const HEADLESS: &str = include_str!("../../data/prompts/headless.md");

pub fn headless(system: String) -> String {
    format!("{system}\n\n{}", HEADLESS.trim_end())
}

static FORUMS: LazyLock<Forums> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/prompts/forums.toml"))
        .expect("data/prompts/forums.toml is checked by tests")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Forums {
    frame: String,
    line: String,
    hint: String,
}

/// Reminders a mode can name in data/modes/*.toml; one line per prompt file.
const REMINDERS: &[(&str, &str)] =
    &[("plan_mode", include_str!("../../data/prompts/plan_mode.md"))];

pub fn build(paths: &Paths, model: &Model) -> String {
    build_with_base(
        paths,
        model,
        if model.capabilities.tool_calling {
            BASE
        } else {
            CHAT_ONLY
        },
    )
}

pub fn selected(app: &crate::app::App, model: &Model, id: &str) -> anyhow::Result<String> {
    match crate::commands::prompt::body(app, id)? {
        Some(base) => Ok(build_with_base(&app.paths, model, &base)),
        None => Ok(build(&app.paths, model)),
    }
}

fn build_with_base(paths: &Paths, model: &Model, base: &str) -> String {
    let cwd = paths.project.display().to_string();
    let date = io::clock::today();
    let env = template::fill(
        ENV,
        &[
            ("cwd", &cwd),
            ("platform", std::env::consts::OS),
            ("date", &date),
            ("model", &model.id),
        ],
    );
    let parts = [
        Some(base.trim_end().to_owned()),
        model.capabilities.tool_calling.then(|| {
            template::fill(
                EXTENSIONS,
                &[
                    (
                        "executable",
                        &io::proc::current_exe()
                            .unwrap_or_else(|_| "palepale".into())
                            .display()
                            .to_string(),
                    ),
                    ("user_config", &paths.user_config().display().to_string()),
                    (
                        "project_config",
                        &paths.project_config().display().to_string(),
                    ),
                ],
            )
            .trim_end()
            .to_owned()
        }),
        forums(paths),
        Some(env.trim_end().to_owned()),
    ];
    parts.into_iter().flatten().collect::<Vec<_>>().join("\n\n")
}

/// The forums this machine can browse, each with its hint, so the model knows to look there.
fn forums(paths: &Paths) -> Option<String> {
    // An unreadable store is reported where forums are used; the prompt just goes without.
    let list = crate::forums::store::load(paths).ok()?;
    if list.is_empty() {
        return None;
    }
    let lines: Vec<String> = list
        .iter()
        .map(|forum| {
            let line = template::fill(
                &FORUMS.line,
                &[("name", &forum.name), ("host", &forum.host())],
            );
            match forum.hint.as_str() {
                "" => line,
                hint => format!(
                    "{line}\n{}",
                    template::fill(&FORUMS.hint, &[("hint", hint)])
                ),
            }
        })
        .collect();
    Some(
        template::fill(&FORUMS.frame, &[("forums", &lines.join("\n"))])
            .trim()
            .to_owned(),
    )
}

pub fn reminder(name: &str) -> Option<&'static str> {
    REMINDERS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| text.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Capabilities, Pricing};

    fn model() -> Model {
        Model {
            id: "m".into(),
            display_name: String::new(),
            description: String::new(),
            context_window: 1000,
            max_output_tokens: 100,
            pricing: Pricing {
                input_per_1m_usd: 1.0,
                cached_input_per_1m_usd: 0.1,
                output_per_1m_usd: 2.0,
            },
            capabilities: Capabilities::default(),
            input_modalities: None,
            capability_source: None,
            reasoning_efforts: None,
            default_reasoning_effort: None,
        }
    }

    #[test]
    fn setup_targets_palepale_even_with_an_empty_catalog_and_a_custom_home() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("custom-home"), root.path().join("project"));
        let mut model = model();
        model.capabilities.tool_calling = true;
        let prompt = build(&paths, &model);
        assert!(prompt.contains(&paths.user_config().display().to_string()));
        assert!(prompt.contains(&paths.project_config().display().to_string()));
        assert!(prompt.contains("target Palepale unless the user explicitly names another client"));
        assert!(prompt.contains("palepale mcp add") && prompt.contains("palepale mcp check"));
        assert!(prompt.contains(".agents/skills") && prompt.contains("startup_timeout_secs"));
        model.capabilities.tool_calling = false;
        assert!(!build(&paths, &model).contains("<palepale_setup>"));
    }

    #[test]
    fn the_forums_to_browse_are_named_with_their_hints() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::at(root.path().join("home"), root.path().join("proj"));
        // With no store yet, the seed is what there is to browse.
        let seed = &crate::forums::data().seed[0];
        let prompt = build(&paths, &model());
        assert!(
            prompt.contains(&format!(
                "- {} ({})\n  {}",
                seed.name,
                seed.host(),
                seed.hint
            )),
            "{prompt}"
        );
        assert!(prompt.find("<forums>") < prompt.find("<env>"));
        crate::forums::store::remove(&paths, &seed.name).unwrap();
        assert!(!build(&paths, &model()).contains("<forums>"));
    }
}

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Spec};
use crate::app::App;
use crate::io;
use crate::text::template::fill;

const SRC: &str = include_str!("../../data/commands/prompt.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static DATA: LazyLock<Data> = LazyLock::new(|| toml::from_str(SRC).expect("prompt command data"));

#[derive(Deserialize)]
struct Data {
    title: String,
    hint: String,
    selected: String,
    missing: String,
    default_label: String,
    chat_label: String,
    max_bytes: usize,
    max_files: usize,
}

pub struct Prompt;

pub fn files(app: &App) -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    let mut roots = vec![app.paths.home.join("prompts")];
    if crate::trust::is_trusted(&app.paths).unwrap_or(false) {
        roots.push(app.paths.project.join(".palepale/prompts"));
    }
    for root in roots {
        for entry in io::fs::list_dir(&root)
            .unwrap_or_default()
            .into_iter()
            .take(DATA.max_files)
        {
            if entry.is_dir || !entry.name.ends_with(".md") {
                continue;
            }
            let id = entry.name.trim_end_matches(".md");
            if id.is_empty() || matches!(id, "default" | "chat") {
                continue;
            }
            let path = io::fs::canonical(&root.join(&entry.name));
            if path.starts_with(io::fs::canonical(&root)) {
                found.insert(id.to_owned(), path);
            }
        }
    }
    found
}

pub fn body(app: &App, id: &str) -> anyhow::Result<Option<String>> {
    if id == super::godmode::text().custom_prompt_id {
        return super::godmode::custom_body(app).map(Some);
    }
    if id == "default" {
        return Ok(None);
    }
    if id == "chat" {
        return Ok(Some(include_str!("../../data/prompts/chat_only.md").into()));
    }
    let files = files(app);
    let file = files
        .get(id)
        .ok_or_else(|| anyhow::anyhow!(fill(&DATA.missing, &[("id", id)])))?;
    let text = io::fs::read_limited(file, DATA.max_bytes)?
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!(fill(&DATA.missing, &[("id", id)])))?;
    Ok(Some(text))
}

impl Command for Prompt {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            if args.is_empty() {
                let mut lines = vec![
                    DATA.title.clone(),
                    format!("default  {}", DATA.default_label),
                    format!("chat  {}", DATA.chat_label),
                ];
                lines.extend(
                    files(cx.app)
                        .into_iter()
                        .map(|(id, file)| format!("{id}  {}", file.display())),
                );
                lines.push(DATA.hint.clone());
                return Ok(lines
                    .iter()
                    .map(|line| crate::text::controls::for_terminal(line).into_owned())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .into());
            }
            let [id] = args else {
                return Err(cx.usage(&SPEC));
            };
            body(cx.app, id)?;
            crate::settings::edit(&cx.app.paths, false, |document| {
                document["system_prompt"] = toml_edit::value(id);
                Ok(())
            })?;
            Ok(Outcome {
                text: fill(&DATA.selected, &[("id", id)]),
                effects: vec![Effect::SetSystemPrompt(id.clone())],
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_prompts_require_trust_and_override_only_the_matching_user_id() {
        let temp = tempfile::tempdir().unwrap();
        let app = App::for_tests(temp.path());
        for (file, body) in [
            (app.paths.home.join("prompts/review.md"), "user review"),
            (app.paths.home.join("prompts/other.md"), "other prompt"),
            (
                app.paths.project.join(".palepale/prompts/review.md"),
                "project review",
            ),
        ] {
            io::fs::write_atomic(&file, body.as_bytes(), io::fs::Access::Shared).unwrap();
        }
        assert_eq!(
            body(&app, "review").unwrap().as_deref(),
            Some("user review")
        );
        crate::trust::trust(&app.paths).unwrap();
        assert_eq!(
            body(&app, "review").unwrap().as_deref(),
            Some("project review")
        );
        assert_eq!(
            body(&app, "other").unwrap().as_deref(),
            Some("other prompt")
        );
        assert!(body(&app, "../review").is_err());
        assert!(body(&app, "default").unwrap().is_none());
    }
}

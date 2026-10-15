//! Current instruction files, refreshed at model boundaries and framed with their scopes.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::Deserialize;

use crate::agent::message::Message;
use crate::io;
use crate::paths::Paths;
use crate::text::template::fill;

#[derive(Deserialize)]
struct Config {
    files: Vec<String>,
    frame: String,
    cut: String,
    unavailable: String,
    intro: String,
    max_scopes: usize,
}

fn config() -> &'static Config {
    static CONFIG: LazyLock<Config> = LazyLock::new(|| {
        toml::from_str(include_str!("../../data/prompts/project.toml")).expect("instruction config")
    });
    &CONFIG
}

pub fn root(paths: &Paths) -> PathBuf {
    let project = io::fs::canonical(&paths.project);
    project
        .ancestors()
        .find(|dir| {
            matches!(
                io::fs::kind(&dir.join(".git")),
                Ok(io::fs::Kind::Dir | io::fs::Kind::File)
            )
        })
        .unwrap_or(&project)
        .to_owned()
}

pub fn snapshot(paths: &Paths, history: &[Message]) -> String {
    let config = config();
    let root = root(paths);
    let mut scopes = BTreeSet::new();
    add_scopes(&mut scopes, &root, &io::fs::canonical(&paths.project));
    // Only completed reads/searches introduce another subtree. Arguments alone grant nothing.
    for message in history {
        if let Message::Tool(result) = message
            && result.refused.is_none()
        {
            if result.name == "grep"
                && let Some(paths_found) = result
                    .details
                    .as_ref()
                    .and_then(|details| details.get("paths"))
                    .and_then(serde_json::Value::as_array)
            {
                for found in paths_found.iter().filter_map(serde_json::Value::as_str) {
                    let path = io::fs::canonical(&paths.project.join(found));
                    if path.starts_with(&root)
                        && let Some(dir) = path.parent()
                    {
                        add_scopes(&mut scopes, &root, dir);
                    }
                }
            }
            for ask in &result.asks {
                if !matches!(ask.permission.as_str(), "read" | "search") {
                    continue;
                }
                for pattern in &ask.patterns {
                    let path = io::fs::canonical(&paths.project.join(pattern));
                    if !path.starts_with(&root) {
                        continue;
                    }
                    let dir = if matches!(io::fs::kind(&path), Ok(io::fs::Kind::Dir)) {
                        path.as_path()
                    } else {
                        path.parent().unwrap_or(&path)
                    };
                    add_scopes(&mut scopes, &root, dir);
                }
            }
        }
    }
    let home = io::fs::canonical(&paths.home);
    scopes.remove(&home);
    let mut sources = vec![(home, true)];
    sources.extend(
        scopes
            .into_iter()
            .take(config.max_scopes)
            .map(|scope| (scope, false)),
    );
    let mut remaining = crate::limits::get().agent.instructions_bytes.value;
    let mut sections = vec![config.intro.clone()];
    let mut seen_files = BTreeSet::new();
    for (scope, global) in sources {
        if remaining == 0 {
            break;
        }
        for name in &config.files {
            let file = scope.join(name);
            let real = io::fs::canonical(&file);
            // A project rule must not pull instructions through a link outside that project.
            if !real.starts_with(if global { &scope } else { &root }) {
                break;
            }
            if seen_files.contains(&real) {
                break;
            }
            let contents = match io::fs::read_prefix(&real, remaining) {
                Ok(None) => continue,
                Ok(Some((text, cut))) => {
                    let text = text.trim_end().to_owned();
                    if text.is_empty() && !cut {
                        break;
                    }
                    remaining = remaining.saturating_sub(text.len());
                    let cut_note = if cut {
                        remaining = 0;
                        config.cut.clone()
                    } else {
                        String::new()
                    };
                    format!("{text}{cut_note}")
                }
                Err(_) => config.unavailable.clone(),
            };
            let shown_scope = if global {
                "global".to_owned()
            } else {
                scope.display().to_string()
            };
            sections.push(fill(
                &config.frame,
                &[
                    ("file", &file.display().to_string()),
                    ("scope", &shown_scope),
                    ("text", &contents),
                ],
            ));
            seen_files.insert(real);
            // Presence, including empty or unreadable AGENTS.md, suppresses CLAUDE.md here.
            break;
        }
    }
    format!(
        "<active_instructions>\n{}\n</active_instructions>",
        sections.join("\n\n")
    )
}

/// Empty projects need no reminder. A previously active snapshot still receives an empty
/// replacement after its last source is removed.
pub fn current(paths: &Paths, history: &[Message]) -> Option<String> {
    let text = snapshot(paths, history);
    (text.contains("<project-instructions ")
        || crate::skills::last_catalog(history, "<active_instructions>").is_some())
    .then_some(text)
}

fn add_scopes(scopes: &mut BTreeSet<PathBuf>, root: &Path, dir: &Path) {
    for ancestor in dir.ancestors() {
        if !ancestor.starts_with(root) {
            break;
        }
        scopes.insert(ancestor.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::ToolResult;
    use crate::permission::gate::Ask;

    #[test]
    fn global_ancestors_nested_scope_and_refresh_are_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        let paths = Paths::at(temp.path().join("custom-home"), repo.join("src"));
        io::fs::make_dir(&repo.join(".git")).unwrap();
        for (file, text) in [
            (paths.home.join("AGENTS.md"), "global-rule"),
            (repo.join("AGENTS.md"), "root-rule"),
            (paths.project.join("CLAUDE.md"), "fallback-rule"),
            (paths.project.join("nested/AGENTS.md"), "nested-rule"),
            (paths.project.join("other/AGENTS.md"), "sibling-rule"),
        ] {
            io::fs::write_atomic(&file, text.as_bytes(), io::fs::Access::Shared).unwrap();
        }
        let initial = snapshot(&paths, &[]);
        assert!(initial.find("global-rule") < initial.find("root-rule"));
        assert!(initial.find("root-rule") < initial.find("fallback-rule"));
        assert!(!initial.contains("nested-rule") && !initial.contains("sibling-rule"));
        let history = vec![Message::Tool(ToolResult {
            call_id: "one".into(),
            name: "read".into(),
            content: String::new(),
            attachments: vec![],
            is_error: false,
            details: None,
            asks: vec![Ask::new("read", "nested/file.rs")],
            refused: None,
        })];
        assert!(snapshot(&paths, &history).contains("nested-rule"));
        io::fs::write_atomic(&repo.join("AGENTS.md"), b"new-rule", io::fs::Access::Shared).unwrap();
        io::fs::remove(&paths.project.join("CLAUDE.md")).unwrap();
        let refreshed = snapshot(&paths, &history);
        assert!(refreshed.contains("new-rule"));
        assert!(!refreshed.contains("root-rule") && !refreshed.contains("fallback-rule"));
    }

    #[test]
    fn empty_agents_suppresses_fallback_and_unicode_truncation_is_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path().join("home"), temp.path().join("repo"));
        io::fs::write_atomic(
            &paths.project.join("AGENTS.md"),
            b"",
            io::fs::Access::Shared,
        )
        .unwrap();
        io::fs::write_atomic(
            &paths.project.join("CLAUDE.md"),
            b"fallback-must-stay-inactive",
            io::fs::Access::Shared,
        )
        .unwrap();
        assert!(!snapshot(&paths, &[]).contains("fallback-must-stay-inactive"));
        io::fs::write_atomic(
            &paths.project.join("AGENTS.md"),
            "é".repeat(crate::limits::get().agent.instructions_bytes.value)
                .as_bytes(),
            io::fs::Access::Shared,
        )
        .unwrap();
        let current = snapshot(&paths, &[]);
        assert!(current.contains("byte budget is exhausted"));
        assert!(!current.contains('\u{fffd}'));
        assert!(current.len() < crate::limits::get().agent.instructions_bytes.value + 2000);
    }

    #[cfg(unix)]
    #[test]
    fn project_instruction_links_cannot_escape_the_repository() {
        let temp = tempfile::tempdir().unwrap();
        let paths = Paths::at(temp.path().join("home"), temp.path().join("repo"));
        io::fs::make_dir(&paths.project).unwrap();
        let outside = temp.path().join("outside.md");
        io::fs::write_atomic(&outside, b"outside-marker", io::fs::Access::Shared).unwrap();
        std::os::unix::fs::symlink(outside, paths.project.join("AGENTS.md")).unwrap();
        assert!(!snapshot(&paths, &[]).contains("outside-marker"));
    }
}

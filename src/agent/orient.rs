//! Bounded project facts for the opening user turn, outside the cached system prompt.
use crate::{io, limits, paths::Paths, text::template::fill};
use serde::Deserialize;
use std::{path::Path, sync::LazyLock};

#[derive(Deserialize)]
struct Text {
    files: String,
    more: String,
    branch: String,
    dirty: String,
    clean: String,
    commits: String,
}
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/orient.toml")).expect("orientation text")
});

pub async fn snapshot(paths: &Paths) -> String {
    let files = listing(&paths.project);
    let git = git_snapshot(&paths.project).await.unwrap_or_default();
    fill(
        include_str!("../../data/prompts/orient.md"),
        &[
            ("cwd", &paths.project.display().to_string()),
            ("files", &files),
            ("git", &git),
        ],
    )
    .trim_end()
    .to_owned()
}

fn listing(dir: &Path) -> String {
    let Ok(entries) = io::fs::list_dir(dir) else {
        return String::new();
    };
    if entries.is_empty() {
        return String::new();
    }
    let shown: Vec<_> = entries
        .iter()
        .take(limits::get().agent.orient_entries.value)
        .map(|entry| {
            if entry.is_dir {
                format!("{}/", entry.name)
            } else {
                entry.name.clone()
            }
        })
        .collect();
    fill(
        &TEXT.files,
        &[
            ("entries", &shown.join("  ")),
            ("more", &more(entries.len().saturating_sub(shown.len()))),
        ],
    )
}

fn more(count: usize) -> String {
    if count == 0 {
        String::new()
    } else {
        fill(&TEXT.more, &[("count", &count.to_string())])
    }
}

async fn git_snapshot(dir: &Path) -> Option<String> {
    let git = io::proc::which("git")?;
    let branch = git_run(&git, dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await?;
    let mut out = fill(&TEXT.branch, &[("branch", &branch)]);
    let status = git_run(&git, dir, &["status", "--short"]).await?;
    if status.is_empty() {
        out.push_str(&TEXT.clean);
    } else {
        let lines: Vec<_> = status
            .lines()
            .take(limits::get().agent.orient_status_lines.value)
            .collect();
        out.push_str(&fill(
            &TEXT.dirty,
            &[
                ("status", &lines.join("\n  ")),
                (
                    "more",
                    &more(status.lines().count().saturating_sub(lines.len())),
                ),
            ],
        ));
    }
    let n = limits::get().agent.orient_commits.value.to_string();
    if let Some(log) = git_run(&git, dir, &["log", "--oneline", "-n", &n])
        .await
        .filter(|s| !s.is_empty())
    {
        out.push_str(&fill(&TEXT.commits, &[("log", &log.replace('\n', "\n  "))]));
    }
    Some(out)
}

async fn git_run(git: &Path, dir: &Path, words: &[&str]) -> Option<String> {
    let args: Vec<_> = words.iter().map(|s| (*s).to_owned()).collect();
    let captured = io::exec::exec(&io::exec::Program {
        program: git,
        args: &args,
        cwd: dir,
        env: &[("GIT_OPTIONAL_LOCKS".into(), "0".into())],
        unset: &[],
        timeout: limits::get().agent.orient_timeout_secs.secs(),
    })
    .await
    .ok()?;
    captured
        .ok()
        .then(|| String::from_utf8_lossy(&captured.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn opening_context_contains_real_files_and_is_bounded() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..60 {
            io::fs::write_atomic(
                &root.path().join(format!("{i:03}.txt")),
                b"x",
                io::fs::Access::Shared,
            )
            .unwrap();
        }
        let paths = Paths::at(root.path().join("home"), root.path().to_owned());
        let text = snapshot(&paths).await;
        assert!(text.contains("<workspace>"));
        assert!(text.contains("000.txt"));
        assert!(!text.contains("059.txt"));
        assert!(text.contains("10 more"));
    }
}

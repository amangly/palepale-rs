//! File search, embedded from ripgrep's crates so nothing needs downloading. Honors .gitignore.
//! Blocking: callers run it off the async threads.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::{GlobBuilder, GlobMatcher};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::WalkBuilder;

pub struct MentionPaths {
    pub paths: Vec<String>,
    pub truncated: bool,
}

/// Discovery runs outside the terminal loop and stops when its picker closes.
pub fn mention_paths(
    root: PathBuf,
) -> (
    std::sync::mpsc::Receiver<MentionPaths>,
    tokio_util::sync::CancellationToken,
) {
    let (send, receive) = std::sync::mpsc::channel();
    let cancel = tokio_util::sync::CancellationToken::new();
    let worker = cancel.clone();
    tokio::task::spawn_blocking(move || {
        let _ = send.send(discover_mentions(&root, &worker));
    });
    (receive, cancel)
}

fn discover_mentions(root: &Path, cancel: &tokio_util::sync::CancellationToken) -> MentionPaths {
    let max = crate::limits::get().mentions.entries.value;
    let mut result = MentionPaths {
        paths: Vec::new(),
        truncated: false,
    };
    for (visited, entry) in WalkBuilder::new(root)
        .standard_filters(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .enumerate()
    {
        if cancel.is_cancelled() {
            break;
        }
        if visited >= max {
            result.truncated = true;
            break;
        }
        let Ok(entry) = entry else {
            result.truncated = true;
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(kind) = entry.file_type() else {
            continue;
        };
        // Exact directory references still work without filling file suggestions.
        if !kind.is_file() {
            continue;
        }
        let name = relative_slashes(root, entry.path());
        if name.contains(['"', '\n', '\r', '`']) {
            continue;
        }
        result.paths.push(name);
    }
    result.paths.sort();
    result
}

pub struct Found {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
}

/// What a content search reports about each file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    /// The matching lines, with this many lines of context before and after each.
    Lines { context: usize },
    /// Only that the file matched; each file stops at its first match.
    Files,
    /// How many lines matched.
    Counts,
}

/// One file's part of a search: its lines (matches and their context) under `Report::Lines`, and
/// how many lines matched.
pub struct FileHits {
    pub path: PathBuf,
    pub lines: Vec<HitLine>,
    pub matched: usize,
}

pub struct HitLine {
    pub number: u64,
    pub text: String,
    /// False for a line shown as context.
    pub matched: bool,
}

/// Files under `root` matching `pattern`. A pattern without `/` matches file names at any depth.
pub fn glob(root: &Path, pattern: &str) -> Result<Vec<Found>, String> {
    let matcher = compile(pattern)?;
    let by_name = !pattern.contains('/');
    let mut found = Vec::new();
    for entry in WalkBuilder::new(root).build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let relative = relative_slashes(root, path);
        let hit = matcher.is_match(&relative)
            || (by_name
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| matcher.is_match(n)));
        if hit {
            found.push(Found {
                path: path.to_path_buf(),
                modified: entry.metadata().ok().and_then(|m| m.modified().ok()),
            });
        }
    }
    Ok(found)
}

pub struct TreeEntry {
    pub name: String,
    /// 1 for an entry of `root` itself.
    pub depth: usize,
    pub is_dir: bool,
}

/// What lies under `root`, `depth` levels down, in tree order with each directory's entries by
/// name, less what .gitignore ignores and `.git` itself.
pub fn tree(root: &Path, depth: usize) -> Vec<TreeEntry> {
    WalkBuilder::new(root)
        .max_depth(Some(depth))
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git")
        .sort_by_file_name(|a, b| a.cmp(b))
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0)
        .map(|e| TreeEntry {
            name: e.file_name().to_string_lossy().into_owned(),
            depth: e.depth(),
            is_dir: e.file_type().is_some_and(|t| t.is_dir()),
        })
        .collect()
}

pub struct Grep<'a> {
    pub roots: &'a [&'a Path],
    pub pattern: &'a str,
    pub glob: Option<&'a str>,
    pub ignore_case: bool,
    pub report: Report,
    /// Stop after this many matching lines (`Lines`) or files (`Files`, `Counts`).
    pub max: usize,
}

/// The files that matched, in walk order, and whether the search stopped at `max`.
pub fn grep(query: &Grep<'_>) -> Result<(Vec<FileHits>, bool), String> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(query.ignore_case)
        .build(query.pattern)
        .map_err(|e| e.to_string())?;
    let filter = query.glob.map(compile).transpose()?;
    let context = match query.report {
        Report::Lines { context } => context,
        Report::Files | Report::Counts => 0,
    };
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(true)
        .before_context(context)
        .after_context(context)
        .build();
    let mut files: Vec<FileHits> = Vec::new();
    let mut lines = 0;
    let Some((first, rest)) = query.roots.split_first() else {
        return Ok((files, false));
    };
    let mut walk = WalkBuilder::new(first);
    for root in rest {
        walk.add(root);
    }
    let mut seen = std::collections::HashSet::new();
    for entry in walk.build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if let Some(filter) = &filter {
            let root = query
                .roots
                .iter()
                .find(|root| path.starts_with(root))
                .copied()
                .unwrap_or(first);
            let relative = relative_slashes(root, path);
            let name = entry.file_name().to_string_lossy();
            if !filter.is_match(&relative) && !filter.is_match(name.as_ref()) {
                continue;
            }
        }
        if !seen.insert(crate::io::fs::canonical(path)) {
            continue;
        }
        let mut sink = Collect {
            report: query.report,
            lines: Vec::new(),
            matched: 0,
            room: query.max.saturating_sub(lines),
        };
        let _ = searcher.search_path(&matcher, path, &mut sink);
        if sink.matched == 0 {
            continue;
        }
        lines += sink.matched;
        files.push(FileHits {
            path: path.to_path_buf(),
            lines: sink.lines,
            matched: sink.matched,
        });
        let reached = match query.report {
            Report::Lines { .. } => lines,
            Report::Files | Report::Counts => files.len(),
        };
        if reached >= query.max {
            return Ok((files, true));
        }
    }
    Ok((files, false))
}

/// Gathers one file's matches for `grep`.
struct Collect {
    report: Report,
    lines: Vec<HitLine>,
    matched: usize,
    /// Matching lines the search may still take before its cap.
    room: usize,
}

impl Collect {
    fn push(&mut self, number: Option<u64>, bytes: &[u8], matched: bool) {
        self.lines.push(HitLine {
            number: number.unwrap_or(0),
            text: String::from_utf8_lossy(bytes)
                .trim_end_matches(['\n', '\r'])
                .to_owned(),
            matched,
        });
    }
}

impl Sink for Collect {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.matched += 1;
        match self.report {
            Report::Files => Ok(false),
            Report::Counts => Ok(true),
            Report::Lines { .. } => {
                self.push(m.line_number(), m.bytes(), true);
                Ok(self.matched < self.room)
            }
        }
    }

    fn context(&mut self, _: &Searcher, c: &SinkContext<'_>) -> Result<bool, Self::Error> {
        self.push(c.line_number(), c.bytes(), false);
        Ok(true)
    }
}

fn compile(pattern: &str) -> Result<GlobMatcher, String> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| e.to_string())
}

fn relative_slashes(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in [
            ("src/main.rs", "fn main() {\n    run();\n}\n"),
            ("src/lib/util.rs", "pub fn run() {}\n"),
            ("notes.md", "run the thing\n"),
            ("target/junk.rs", "fn run() {}\n"),
            (".gitignore", "target/\n"),
        ] {
            crate::io::fs::write_atomic(
                &dir.path().join(path),
                text.as_bytes(),
                crate::io::fs::Access::Shared,
            )
            .unwrap();
        }
        // `ignore` only applies .gitignore inside a git repository.
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        dir
    }

    fn names(found: &[PathBuf], root: &Path) -> Vec<String> {
        let mut names: Vec<String> = found.iter().map(|p| relative_slashes(root, p)).collect();
        names.sort();
        names
    }

    #[test]
    fn mention_discovery_lists_searchable_files_only_and_cancels() {
        let dir = tree();
        for (path, text) in [
            (".hidden/private.txt", "hidden"),
            (".ignore", "cache/\n"),
            ("cache/compiled.txt", "ignored"),
        ] {
            crate::io::fs::write_atomic(
                &dir.path().join(path),
                text.as_bytes(),
                crate::io::fs::Access::Shared,
            )
            .unwrap();
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        let result = discover_mentions(dir.path(), &cancel);
        assert_eq!(result.paths, ["notes.md", "src/lib/util.rs", "src/main.rs"]);
        assert!(!result.truncated);
        std::fs::remove_dir(dir.path().join(".git")).unwrap();
        assert_eq!(discover_mentions(dir.path(), &cancel).paths, result.paths);
        cancel.cancel();
        assert!(discover_mentions(dir.path(), &cancel).paths.is_empty());
    }

    #[test]
    fn glob_matches_names_anywhere_and_paths_exactly() {
        let dir = tree();
        let all: Vec<PathBuf> = glob(dir.path(), "*.rs")
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(
            names(&all, dir.path()),
            vec!["src/lib/util.rs", "src/main.rs"]
        );
        let top: Vec<PathBuf> = glob(dir.path(), "src/*.rs")
            .unwrap()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(names(&top, dir.path()), vec!["src/main.rs"]);
    }

    #[test]
    fn tree_goes_down_in_order_and_skips_what_git_ignores() {
        let dir = tree();
        let shown: Vec<String> = super::tree(dir.path(), 2)
            .iter()
            .map(|e| {
                format!(
                    "{}{}{}",
                    "  ".repeat(e.depth - 1),
                    e.name,
                    if e.is_dir { "/" } else { "" }
                )
            })
            .collect();
        assert_eq!(
            shown,
            [".gitignore", "notes.md", "src/", "  lib/", "  main.rs"]
        );
    }

    #[test]
    fn grep_finds_lines_respects_filters_and_caps() {
        let dir = tree();
        let query = Grep {
            roots: &[dir.path()],
            pattern: r"\brun\b",
            glob: Some("*.rs"),
            ignore_case: false,
            report: Report::Lines { context: 0 },
            max: 10,
        };
        let (hits, truncated) = grep(&query).unwrap();
        assert!(!truncated);
        let mut lines: Vec<String> = hits
            .iter()
            .flat_map(|f| {
                let name = relative_slashes(dir.path(), &f.path);
                f.lines.iter().map(move |l| format!("{name}:{}", l.number))
            })
            .collect();
        lines.sort();
        assert_eq!(lines, vec!["src/lib/util.rs:1", "src/main.rs:2"]);
        let (capped, truncated) = grep(&Grep {
            max: 1,
            glob: None,
            ..query
        })
        .unwrap();
        assert_eq!(capped.len(), 1);
        assert!(truncated);
    }

    #[test]
    fn grep_combines_roots_without_duplicate_files_and_shares_its_cap() {
        let dir = tree();
        let source = dir.path().join("src");
        let file = source.join("main.rs");
        let roots = [source.as_path(), file.as_path()];
        let query = Grep {
            roots: &roots,
            pattern: "run",
            glob: None,
            ignore_case: false,
            report: Report::Files,
            max: 10,
        };
        let (hits, _) = grep(&query).unwrap();
        assert_eq!(hits.len(), 2);
        let (hits, capped) = grep(&Grep { max: 1, ..query }).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(capped);
    }

    #[test]
    fn grep_reports_context_files_or_counts() {
        let dir = tree();
        let roots = [dir.path()];
        let query = |report| Grep {
            roots: &roots,
            pattern: "run",
            glob: Some("main.rs"),
            ignore_case: false,
            report,
            max: 10,
        };
        let (hits, _) = grep(&query(Report::Lines { context: 1 })).unwrap();
        let shown: Vec<(u64, bool)> = hits[0]
            .lines
            .iter()
            .map(|l| (l.number, l.matched))
            .collect();
        assert_eq!(shown, [(1, false), (2, true), (3, false)]);
        let (hits, _) = grep(&query(Report::Files)).unwrap();
        assert!(hits[0].lines.is_empty() && hits[0].matched == 1);
        let (hits, _) = grep(&Grep {
            glob: None,
            ..query(Report::Counts)
        })
        .unwrap();
        let total: usize = hits.iter().map(|f| f.matched).sum();
        assert_eq!((hits.len(), total), (3, 3));
    }
}

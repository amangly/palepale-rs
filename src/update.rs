//! Verified release replacement, with progress for the interactive startup screen.

use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, anyhow, bail};
use futures_util::StreamExt;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use sha2::{Digest, Sha256};

use crate::api::endpoints;
use crate::io::http::{Http, Method, Request};
use crate::io::{self, fs::Access};
use crate::limits;
use crate::paths::Paths;
use crate::release;
use crate::text::template::fill;

static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(include_str!("../data/update.toml"))
        .expect("data/update.toml is checked by tests")
        .text
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    updated: String,
    opened: String,
    up_to_date: String,
    development: String,
    unsupported_tag: String,
    not_published: String,
    listing: String,
    no_platform: String,
    missing: String,
    not_writable: String,
    mismatch: String,
    http: String,
    too_large: String,
    signature: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate(String),
    Updated(Change),
}

#[derive(Clone, Debug)]
pub enum Progress {
    Checking,
    Downloading { received: u64, total: Option<u64> },
    Verifying,
    Installing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub from: String,
    pub to: String,
}

impl Outcome {
    pub fn say(&self) -> String {
        match self {
            Self::UpToDate(tag) => fill(&TEXT.up_to_date, &[("tag", tag)]),
            Self::Updated(change) => change.say(),
        }
    }
}

impl Change {
    pub fn opened(&self) -> String {
        fill(&TEXT.opened, &[("from", &self.from), ("to", &self.to)])
    }
    pub fn say(&self) -> String {
        fill(&TEXT.updated, &[("from", &self.from), ("to", &self.to)])
    }
}

/// What the updater remembers between runs. Every field is optional, so older files load.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub checked_at: Option<Timestamp>,
    /// An update not yet announced: the next start says so, then clears it.
    #[serde(default)]
    pub updated: Option<Change>,
    #[serde(default)]
    pub last_error: Option<String>,
}

pub fn load(paths: &Paths) -> State {
    io::fs::read_string(&paths.update_state())
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(paths: &Paths, state: &State) {
    // Losing the state costs one early check or one missed notice, nothing more.
    if let Ok(json) = serde_json::to_vec_pretty(state) {
        let _ = io::fs::write_atomic(&paths.update_state(), &json, Access::Shared);
    }
}

/// The update an earlier run installed, once: reading it clears it.
pub fn take_news(paths: &Paths) -> Option<Change> {
    let mut state = load(paths);
    let news = state.updated.take()?;
    save(paths, &state);
    Some(news)
}

/// Short restart bursts share a recent check instead of exhausting the release feed's limit.
pub fn due(state: &State, now: Timestamp, enabled: bool) -> bool {
    let interval = SignedDuration::try_from(limits::get().update.check_interval_secs.secs())
        .unwrap_or(SignedDuration::MAX);
    enabled
        && release::tag().is_some()
        && !endpoints::origin("releases").is_empty()
        && state
            .checked_at
            .is_none_or(|at| now.duration_since(at) >= interval)
}

/// Checks and updates this binary now, recording the look and its result.
pub async fn now(paths: &Paths) -> anyhow::Result<Outcome> {
    with_progress(paths, &|_| {}).await
}

pub async fn with_progress(
    paths: &Paths,
    progress: &(dyn Fn(Progress) + Sync),
) -> anyhow::Result<Outcome> {
    let current = release::tag().ok_or_else(|| anyhow!(TEXT.development.clone()))?;
    if endpoints::origin("releases").is_empty() {
        anyhow::bail!(crate::backend::text().releases_unconfigured.clone());
    }
    let exe = io::proc::current_exe()?;
    io::fs::sweep_replaced(&exe);
    let http = Http::new(limits::get().http.connect_timeout_ms.ms())?;
    let latest = endpoints::url(
        endpoints::get("latest_release"),
        &[("repository", release::repository())],
    );
    let mut state = load(paths);
    let result = run_with_progress(&http, &latest, current, &exe, progress).await;
    // The reopened process shares this check even after a slow download.
    state.checked_at = Some(io::clock::now());
    match &result {
        Ok(Outcome::Updated(change)) => {
            state.updated = Some(change.clone());
            state.last_error = None;
        }
        Ok(Outcome::UpToDate(_)) => state.last_error = None,
        Err(e) => state.last_error = Some(format!("{e:#}")),
    }
    save(paths, &state);
    result
}

#[derive(Deserialize)]
struct Latest {
    tag_name: String,
    assets: Vec<Remote>,
}

#[derive(Deserialize)]
struct Remote {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
}

/// Replaces `exe`, built as release `current`, with the release `latest_url` names, after
/// checking the download against the release's checksums.
/// Configured signing keys require an authenticated checksum manifest bound to the tag.
#[cfg(test)]
async fn run(http: &Http, latest_url: &str, current: &str, exe: &Path) -> anyhow::Result<Outcome> {
    run_with_progress(http, latest_url, current, exe, &|_| {}).await
}

async fn run_with_progress(
    http: &Http,
    latest_url: &str,
    current: &str,
    exe: &Path,
    progress: &(dyn Fn(Progress) + Sync),
) -> anyhow::Result<Outcome> {
    progress(Progress::Checking);
    let listing = get(
        http,
        latest_url,
        limits::get().update.check_timeout_secs.secs(),
        true,
    )
    .await;
    let latest: Latest = listing
        .and_then(|body| Ok(serde_json::from_slice(&body)?))
        .map_err(|e| anyhow!(fill(&TEXT.listing, &[("error", &format!("{e:#}"))])))?;
    let newer = release::newer(&latest.tag_name, current).ok_or_else(|| {
        anyhow!(fill(
            &TEXT.unsupported_tag,
            &[("latest", &latest.tag_name), ("current", current)]
        ))
    })?;
    if !newer {
        return Ok(Outcome::UpToDate(current.to_owned()));
    }
    let ours = release::asset().ok_or_else(|| {
        anyhow!(fill(
            &TEXT.no_platform,
            &[
                ("os", std::env::consts::OS),
                ("arch", std::env::consts::ARCH)
            ]
        ))
    })?;
    let find = |name: &str| {
        latest
            .assets
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| {
                anyhow!(fill(
                    &TEXT.missing,
                    &[("tag", &latest.tag_name), ("file", name)]
                ))
            })
    };
    let (file, sums) = (find(&ours.file)?, find(release::sums())?);
    let dir = exe.parent().context("the program has no folder")?;
    if !io::fs::writable(dir) {
        bail!(fill(
            &TEXT.not_writable,
            &[("dir", &dir.display().to_string())]
        ));
    }
    let bytes = download_binary(http, file, progress).await?;
    progress(Progress::Verifying);
    let sums = get(
        http,
        &sums.browser_download_url,
        limits::get().update.check_timeout_secs.secs(),
        false,
    )
    .await?;
    let signing = release::signing();
    if !signing.keys.is_empty() {
        let signature = find(&signing.file)?;
        let signature = get(
            http,
            &signature.browser_download_url,
            limits::get().update.check_timeout_secs.secs(),
            false,
        )
        .await?;
        if !release::signature::check(
            &sums,
            &String::from_utf8_lossy(&signature),
            &latest.tag_name,
            signing,
        ) {
            bail!(TEXT.signature.clone());
        }
    }
    if !matches(&bytes, &String::from_utf8_lossy(&sums), &ours.file) {
        bail!(fill(&TEXT.mismatch, &[("file", &ours.file)]));
    }
    progress(Progress::Installing);
    let name = exe
        .file_name()
        .map_or_else(|| "palepale".into(), |n| n.to_string_lossy());
    let staged = io::fs::executable_stage(dir, &name);
    io::fs::write_executable(&staged, &bytes)?;
    if let Err(e) = io::fs::replace_executable(&staged, exe) {
        let _ = io::fs::remove(&staged);
        return Err(e.into());
    }
    Ok(Outcome::Updated(Change {
        from: current.to_owned(),
        to: latest.tag_name,
    }))
}

async fn download_binary(
    http: &Http,
    file: &Remote,
    progress: &(dyn Fn(Progress) + Sync),
) -> anyhow::Result<Vec<u8>> {
    let bounds = &limits::get().update;
    let read = async {
        let mut response = http
            .open(Request {
                method: Method::Get,
                url: file.browser_download_url.clone(),
                bearer: None,
                headers: &[],
                json: None,
                timeout: bounds.check_timeout_secs.secs(),
            })
            .await?;
        if response.head.status >= 400 {
            bail!(fill(
                &TEXT.http,
                &[
                    ("url", &file.browser_download_url),
                    ("status", &response.head.status.to_string())
                ]
            ));
        }
        let mut bytes = Vec::new();
        progress(Progress::Downloading {
            received: 0,
            total: file.size,
        });
        while let Some(chunk) = response.body.next().await {
            let chunk = chunk?;
            if chunk.len() > bounds.download_max_bytes.value.saturating_sub(bytes.len()) {
                bail!("{}", TEXT.too_large);
            }
            bytes.extend(chunk);
            progress(Progress::Downloading {
                received: bytes.len() as u64,
                total: file.size,
            });
        }
        Ok(bytes)
    };
    tokio::time::timeout(bounds.download_timeout_secs.secs(), read).await?
}

async fn get(
    http: &Http,
    url: &str,
    timeout: std::time::Duration,
    listing: bool,
) -> anyhow::Result<Vec<u8>> {
    let read = async {
        let mut resp = http
            .open(Request {
                method: Method::Get,
                url: url.to_owned(),
                bearer: None,
                headers: &[("Accept", "application/vnd.github+json")],
                json: None,
                timeout,
            })
            .await?;
        if listing && resp.head.status == 404 {
            bail!(fill(&TEXT.not_published, &[("url", url)]));
        }
        if resp.head.status >= 400 {
            bail!(fill(
                &TEXT.http,
                &[("url", url), ("status", &resp.head.status.to_string())]
            ));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.body.next().await {
            let chunk = chunk?;
            if chunk.len()
                > limits::get()
                    .update
                    .metadata_max_bytes
                    .value
                    .saturating_sub(bytes.len())
            {
                bail!("{}", TEXT.too_large);
            }
            bytes.extend(chunk);
        }
        Ok(bytes)
    };
    tokio::time::timeout(timeout, read).await?
}

/// Whether `bytes` hash to what `sums` (sha256sum's `<hex>  <name>` lines) says `name` is.
fn matches(bytes: &[u8], sums: &str, name: &str) -> bool {
    let want = sums.lines().find_map(|line| {
        let (hex, file) = line.split_once(char::is_whitespace)?;
        (file.trim_start().trim_start_matches('*') == name).then(|| hex.to_lowercase())
    });
    let got = crate::text::digest::sha256_hex(bytes);
    want.is_some_and(|want| want == got)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Router;
    use axum::routing::get;
    use serde_json::json;

    use super::*;

    fn sums_for(bytes: &[u8], name: &str) -> String {
        let hex: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("{hex}  {name}\n")
    }

    #[test]
    fn a_download_matches_only_its_own_line() {
        let sums = format!("{}{}", sums_for(b"one", "a"), sums_for(b"two", "b"));
        assert!(matches(b"two", &sums, "b"));
        assert!(!matches(b"one", &sums, "b"));
        assert!(!matches(b"two", &sums, "c"));
    }

    #[test]
    fn restart_bursts_share_a_check_and_later_starts_check_again() {
        let now: Timestamp = "2026-10-03T12:00:00Z".parse().unwrap();
        let recent = State {
            checked_at: Some(now - SignedDuration::from_secs(10)),
            ..State::default()
        };
        let stale = State {
            checked_at: Some(now - SignedDuration::from_secs(61)),
            ..State::default()
        };
        assert!(!due(&recent, now, true));
        assert!(!due(&stale, now, false));
        // A test binary is a development build, which never updates itself.
        assert_eq!(due(&stale, now, true), release::tag().is_some());
    }

    #[tokio::test]
    async fn oversized_metadata_and_stalled_bodies_fail_before_installing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route(
                "/large",
                get(|| async { vec![b' '; limits::get().update.metadata_max_bytes.value + 1] }),
            )
            .route(
                "/slow",
                get(|| async {
                    axum::body::Body::from_stream(futures_util::stream::once(async {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        Ok::<_, std::convert::Infallible>(b"{}".to_vec())
                    }))
                }),
            );
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let http = Http::new(std::time::Duration::from_secs(1)).unwrap();
        assert!(
            super::get(
                &http,
                &format!("{base}/large"),
                std::time::Duration::from_secs(1),
                true
            )
            .await
            .unwrap_err()
            .to_string()
            .contains(&TEXT.too_large)
        );
        assert!(
            super::get(
                &http,
                &format!("{base}/slow"),
                std::time::Duration::from_millis(50),
                false
            )
            .await
            .is_err()
        );
    }

    /// A stand-in for GitHub: the newest release `tag`, its file for this platform, and sums.
    async fn github(tag: &str, file: Vec<u8>, sums: String) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let name = release::asset().unwrap().file.clone();
        let latest = json!({"tag_name": tag, "assets": [
            {"name": name, "browser_download_url": format!("{base}/file")},
            {"name": release::sums(), "browser_download_url": format!("{base}/sums")},
        ]});
        let file = Arc::new(file);
        let router = Router::new()
            .route("/latest", get(move || async move { axum::Json(latest) }))
            .route("/file", get(move || async move { file.as_ref().clone() }))
            .route("/sums", get(move || async move { sums }));
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("{base}/latest")
    }

    #[tokio::test]
    async fn a_newer_release_replaces_the_program_once_its_checksum_matches() {
        let Some(asset) = release::asset() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("palepale");
        io::fs::write_executable(&exe, b"old build").unwrap();
        let http = Http::new(std::time::Duration::from_secs(5)).unwrap();

        let url = github(
            "v0.2.0-2",
            b"new build".to_vec(),
            sums_for(b"new build", &asset.file),
        )
        .await;
        let outcome = run(&http, &url, "v0.2.0-1", &exe).await.unwrap();
        assert_eq!(
            outcome,
            Outcome::Updated(Change {
                from: "v0.2.0-1".into(),
                to: "v0.2.0-2".into()
            })
        );
        assert_eq!(io::fs::read_bytes(&exe).unwrap(), b"new build");
        assert_eq!(
            run(&http, &url, "v0.2.0-2", &exe).await.unwrap(),
            Outcome::UpToDate("v0.2.0-2".into())
        );
        assert_eq!(
            run(&http, &url, "v0.2.0-10", &exe).await.unwrap(),
            Outcome::UpToDate("v0.2.0-10".into())
        );
        let invalid = github("v0.2.0-preview", b"unverified".to_vec(), String::new()).await;
        assert!(
            run(&http, &invalid, "v0.2.0-2", &exe)
                .await
                .unwrap_err()
                .to_string()
                .contains("expected vMAJOR")
        );
        assert_eq!(io::fs::read_bytes(&exe).unwrap(), b"new build");

        let tampered = github(
            "v0.2.0-3",
            b"evil build".to_vec(),
            sums_for(b"new build", &asset.file),
        )
        .await;
        let refused = run(&http, &tampered, "v0.2.0-2", &exe).await.unwrap_err();
        assert!(refused.to_string().contains("does not match"), "{refused}");
        assert_eq!(io::fs::read_bytes(&exe).unwrap(), b"new build");
    }
}

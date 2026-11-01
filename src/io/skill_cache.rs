//! Download Agent Skills indexes into an isolated, versioned cache. Publish complete sets only.
#![allow(clippy::disallowed_methods)]

use super::{clock, fs};
use crate::{extensions, limits, settings};
use anyhow::{Context, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Deserialize, Serialize)]
struct Entry {
    name: String,
    files: Vec<String>,
    version: Option<String>,
}
#[derive(Deserialize)]
struct Index {
    skills: Vec<Entry>,
}
#[derive(Deserialize, Serialize)]
struct Manifest {
    checked_at: i64,
    roots: Vec<PathBuf>,
}

pub fn source_dir(url: &str, home: &Path) -> PathBuf {
    home.join("cache/skills")
        .join(extensions::hash(url.as_bytes()))
}

pub fn validate(raw: &str) -> anyhow::Result<url::Url> {
    let mut url = url::Url::parse(raw)?;
    let local = matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        bail!("Use HTTPS for a remote skill source");
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        bail!("Skill source URL cannot contain credentials or a fragment");
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

fn manifest(root: &Path) -> Option<Manifest> {
    let source = fs::read_limited(
        &root.join("index-cache.json"),
        limits::get().extensions.skill_file_bytes.value,
    )
    .ok()??;
    serde_json::from_str(&source).ok()
}

pub fn cached(url: &str, home: &Path) -> Vec<PathBuf> {
    let root = fs::canonical(&source_dir(url, home));
    manifest(&root)
        .map(|m| {
            m.roots
                .into_iter()
                .filter(|p| {
                    let path = fs::canonical(p);
                    path.starts_with(&root)
                        && fs::kind(&path.join(".complete")).is_ok_and(|k| k == fs::Kind::File)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn relative(file: &str) -> bool {
    !file.is_empty()
        && file != ".complete"
        && !file.contains(['\\', ':', '\0'])
        && file
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
}

fn child(base: &url::Url, parts: &[&str]) -> anyhow::Result<url::Url> {
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Skill source needs a hierarchical URL"))?
        .pop_if_empty()
        .extend(parts.iter().copied());
    Ok(url)
}

async fn fetch(client: &reqwest::Client, url: url::Url, cap: usize) -> anyhow::Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.without_url())?
        .error_for_status()
        .map_err(|e| e.without_url())?;
    if response.content_length().is_some_and(|n| n > cap as u64) {
        bail!("Skill download exceeds its size limit");
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.without_url())?;
        if chunk.len() > cap.saturating_sub(body.len()) {
            bail!("Skill download exceeds its size limit");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub async fn pull(raw: &str, home: &Path, refresh: bool) -> anyhow::Result<()> {
    let base = validate(raw)?;
    let root = source_dir(raw, home);
    let now = clock::now().as_second();
    if !refresh
        && let Some(old) = manifest(&root)
        && (0..limits::get().extensions.skill_source_cache_secs.value as i64)
            .contains(&(now - old.checked_at))
        && cached(raw, home).len() == old.roots.len()
    {
        return Ok(());
    }
    if settings::env("PALEPALE_OFFLINE").is_some() {
        return Ok(());
    }
    let origin = base.origin();
    let client = reqwest::Client::builder()
        .connect_timeout(limits::get().http.connect_timeout_ms.ms())
        .timeout(limits::get().extensions.connect_timeout_secs.secs())
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() < 5 && attempt.url().origin() == origin {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()?;
    let bytes = fetch(
        &client,
        child(&base, &["index.json"])?,
        limits::get().extensions.skill_asset_bytes.value,
    )
    .await?;
    let index: Index = serde_json::from_slice(&bytes).context("Invalid skill index")?;
    if index.skills.len() > limits::get().extensions.skill_count.value {
        bail!("Skill index exceeds its skill limit");
    }
    let mut roots = Vec::new();
    let mut names = std::collections::HashSet::new();
    let mut total = bytes.len();
    for entry in index.skills {
        if !crate::skills::valid_name(&entry.name) || !names.insert(entry.name.clone()) {
            bail!("Skill index contains an invalid or duplicate skill name");
        }
        if !entry.files.iter().any(|s| s == "SKILL.md") {
            continue;
        }
        if entry.files.len() > limits::get().extensions.skill_source_files.value
            || entry.files.iter().any(|s| !relative(s))
        {
            bail!("Skill index contains invalid paths or too many files");
        }
        let identity = extensions::hash(&serde_json::to_vec(&entry)?);
        let dir = root.join(identity).join(&entry.name);
        if !fs::canonical(&dir).starts_with(fs::canonical(&root)) {
            bail!("Skill cache folder escapes its source");
        }
        if fs::kind(&dir.join(".complete"))? != fs::Kind::File {
            for file in &entry.files {
                let mut segments = vec![entry.name.as_str()];
                segments.extend(file.split('/'));
                let cap = if file == "SKILL.md" {
                    limits::get().extensions.skill_file_bytes.value
                } else {
                    limits::get().extensions.skill_asset_bytes.value
                };
                let body = fetch(&client, child(&base, &segments)?, cap).await?;
                total = total
                    .checked_add(body.len())
                    .context("Skill download exceeded its size limit")?;
                if total > limits::get().extensions.skill_source_bytes.value {
                    bail!("Skill source exceeds its total size limit");
                }
                let destination = dir.join(file);
                if !fs::canonical(&destination).starts_with(fs::canonical(&dir)) {
                    bail!("Skill cache path escapes its directory");
                }
                fs::write_atomic(&destination, &body, fs::Access::Private)?;
            }
            fs::write_atomic(&dir.join(".complete"), b"complete\n", fs::Access::Private)?;
        }
        roots.push(fs::canonical(&dir));
    }
    fs::write_atomic(
        &root.join("index-cache.json"),
        &serde_json::to_vec(&Manifest {
            checked_at: now,
            roots,
        })?,
        fs::Access::Private,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_paths_cannot_be_absolute_traversing_or_platform_specific() {
        for file in [
            "../escape",
            "/absolute",
            "C:/escape",
            "refs/../escape",
            "refs\\escape",
            "refs//file",
            ".complete",
        ] {
            assert!(!relative(file), "{file}");
        }
        assert!(relative("references/guide.md"));
        assert!(validate("https://example.com/skills").is_ok());
        assert!(validate("https://login:secret@example.com/skills").is_err());
    }

    #[tokio::test]
    async fn versions_publish_complete_skill_sets_and_failed_refresh_preserves_the_previous_set() {
        use axum::{
            Router,
            extract::{OriginalUri, State},
            response::IntoResponse,
            routing::get,
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        #[derive(Clone)]
        struct Source {
            version: Arc<AtomicUsize>,
            hits: Arc<AtomicUsize>,
        }
        async fn serve(
            State(source): State<Source>,
            OriginalUri(uri): OriginalUri,
        ) -> axum::response::Response {
            source.hits.fetch_add(1, Ordering::SeqCst);
            let version = source.version.load(Ordering::SeqCst);
            match uri.path() {
                "/skills/index.json" => axum::Json(serde_json::json!({"skills":[{"name":"remote-review","version":version.to_string(),"files":["SKILL.md","references/guide.md"]}]})).into_response(),
                "/skills/remote-review/SKILL.md" => format!("---\nname: remote-review\ndescription: Remote workflow\n---\nVersion {version} instructions").into_response(),
                "/skills/remote-review/references/guide.md" if version != 2 => "Supporting reference".into_response(),
                _ => axum::http::StatusCode::NOT_FOUND.into_response(),
            }
        }
        let source = Source {
            version: Arc::new(AtomicUsize::new(1)),
            hits: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/skills", listener.local_addr().unwrap());
        let router = Router::new()
            .fallback(get(serve))
            .with_state(source.clone());
        let task = tokio::spawn(async {
            axum::serve(listener, router).await.unwrap();
        });
        let home = tempfile::tempdir().unwrap();
        pull(&url, home.path(), true).await.unwrap();
        let original = cached(&url, home.path());
        assert_eq!(original.len(), 1);
        assert!(
            fs::read_string(&original[0].join("SKILL.md"))
                .unwrap()
                .unwrap()
                .contains("Version 1")
        );
        let hits = source.hits.load(Ordering::SeqCst);
        pull(&url, home.path(), false).await.unwrap();
        assert_eq!(source.hits.load(Ordering::SeqCst), hits);
        source.version.store(2, Ordering::SeqCst);
        assert!(pull(&url, home.path(), true).await.is_err());
        assert_eq!(cached(&url, home.path()), original);
        source.version.store(3, Ordering::SeqCst);
        pull(&url, home.path(), true).await.unwrap();
        let current = cached(&url, home.path());
        assert_ne!(current, original);
        assert!(
            fs::read_string(&current[0].join("SKILL.md"))
                .unwrap()
                .unwrap()
                .contains("Version 3")
        );
        task.abort();
    }
}

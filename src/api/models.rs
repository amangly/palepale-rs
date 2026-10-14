use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::api::Call;
use crate::app::App;
use crate::io;
use crate::limits;

#[cfg(test)]
const FIXTURE: &str = include_str!("../../tests/fixtures/api/models.json");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub pricing: Pricing,
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Public request levels, not private provider tier mappings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
    #[serde(default)]
    pub default_reasoning_effort: Option<String>,
}

/// USD per million tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pricing {
    pub input_per_1m_usd: f64,
    #[serde(default)]
    pub cached_input_per_1m_usd: f64,
    pub output_per_1m_usd: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub streaming: bool,
    pub reasoning: bool,
    pub tool_calling: bool,
    pub temperature: bool,
    pub caching: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Live,
    Cache,
    Unavailable,
    Unconfigured,
}

pub struct Catalog {
    pub models: Vec<Model>,
    pub source: Source,
    pub fetched_at: Option<Timestamp>,
    /// Why the live list was not used, when that is worth telling the user.
    pub note: Option<String>,
}

impl Catalog {
    pub fn selected(&self, requested: &str) -> anyhow::Result<&Model> {
        if requested.is_empty() {
            self.models
                .first()
                .ok_or_else(|| anyhow::anyhow!("No models available from the configured backend."))
        } else {
            self.get(requested).ok_or_else(|| {
                anyhow::anyhow!("unknown model `{requested}`; see `palepale models`")
            })
        }
    }

    pub fn get(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id)
    }
}

#[derive(Serialize, Deserialize)]
struct Listing {
    data: Vec<Model>,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    fetched_at: Timestamp,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    api_origin: Option<String>,
    data: Vec<Model>,
}

/// With an explicit backend: fresh cache, live list, stale cache, then an empty catalog.
pub async fn load(app: &App, refresh: bool) -> Catalog {
    if !app.api.is_configured() {
        return Catalog {
            models: Vec::new(),
            source: Source::Unconfigured,
            fetched_at: None,
            note: None,
        };
    }
    let path = app.paths.models_cache();
    let origin = crate::api::endpoints::origin("api");
    let cached = read_cache(&path, &origin);
    let now = io::clock::now();
    let ttl = SignedDuration::try_from(limits::get().models.cache_ttl_secs.secs())
        .unwrap_or(SignedDuration::MAX);

    if !refresh
        && let Some(cache) = &cached
        && cache.release.as_deref() == crate::release::tag()
        && is_fresh(cache.fetched_at, now, ttl)
    {
        return from_cache(cache, None);
    }

    let note = match fetch(app).await {
        Ok(models) => {
            let cache = Cached {
                fetched_at: now,
                release: crate::release::tag().map(str::to_owned),
                api_origin: Some(origin),
                data: models,
            };
            // A cache that fails to write only costs a fetch next time.
            if let Ok(json) = serde_json::to_vec_pretty(&cache) {
                let _ = io::fs::write_atomic(&path, &json, io::fs::Access::Shared);
            }
            return Catalog {
                models: cache.data,
                source: Source::Live,
                fetched_at: Some(now),
                note: None,
            };
        }
        Err(why) => why,
    };

    match &cached {
        Some(cache) => from_cache(cache, Some(note)),
        None => Catalog {
            models: Vec::new(),
            source: Source::Unavailable,
            fetched_at: None,
            note: Some(note),
        },
    }
}

async fn fetch(app: &App) -> Result<Vec<Model>, String> {
    let timeout = limits::get().models.fetch_timeout_ms.ms();
    let resp = app
        .api
        .call("models", Call::default(), timeout)
        .await
        .map_err(|e| e.to_string())?;
    let listing: Listing =
        serde_json::from_slice(&resp.body).map_err(|e| format!("unreadable model list: {e}"))?;
    if !usable(&listing.data) {
        return Err("palepale returned an empty model list".into());
    }
    Ok(listing.data)
}

/// An empty list must not read as "palepale has no models", whether it came live or from the
/// cache; what we had before is kept instead.
fn usable(models: &[Model]) -> bool {
    !models.is_empty()
}

fn read_cache(path: &std::path::Path, origin: &str) -> Option<Cached> {
    let text = io::fs::read_string(path).ok().flatten()?;
    serde_json::from_str::<Cached>(&text)
        .ok()
        .filter(|cache| cache.api_origin.as_deref() == Some(origin) && usable(&cache.data))
}

fn is_fresh(fetched_at: Timestamp, now: Timestamp, ttl: SignedDuration) -> bool {
    let age = now.duration_since(fetched_at);
    age >= SignedDuration::ZERO && age < ttl
}

fn from_cache(cache: &Cached, note: Option<String>) -> Catalog {
    Catalog {
        models: cache.data.clone(),
        source: Source::Cache,
        fetched_at: Some(cache.fetched_at),
        note,
    }
}

#[cfg(test)]
pub fn fixture() -> Vec<Model> {
    serde_json::from_str::<Listing>(FIXTURE)
        .expect("model fixture is checked by tests")
        .data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_parses_and_is_not_empty() {
        assert!(!fixture().is_empty());
    }

    #[test]
    fn legacy_metadata_stays_absent_when_cached() {
        let mut value: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        value["data"][0]
            .as_object_mut()
            .unwrap()
            .remove("reasoning_efforts");
        let legacy: Model = serde_json::from_value(value["data"][0].clone()).unwrap();
        let cached: Model = serde_json::from_value(serde_json::to_value(&legacy).unwrap()).unwrap();
        assert!(cached.reasoning_efforts.is_none());
        assert_eq!(
            cached.default_reasoning_effort,
            legacy.default_reasoning_effort
        );
    }

    #[test]
    fn an_empty_cache_counts_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let write = |data: Vec<Model>| {
            let cache = Cached {
                fetched_at: io::clock::now(),
                release: None,
                api_origin: Some("http://127.0.0.1:8080".into()),
                data,
            };
            io::fs::write_atomic(
                &path,
                &serde_json::to_vec(&cache).unwrap(),
                io::fs::Access::Shared,
            )
            .unwrap();
        };
        write(Vec::new());
        assert!(read_cache(&path, "http://127.0.0.1:8080").is_none());
        write(fixture());
        assert!(read_cache(&path, "http://127.0.0.1:8080").is_some());
        assert!(read_cache(&path, "https://api.palepale.tech").is_none());
    }

    #[test]
    fn freshness_respects_ttl_and_clock_skew() {
        let t: Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
        let ttl = SignedDuration::from_secs(60);
        assert!(is_fresh(t, t + SignedDuration::from_secs(59), ttl));
        assert!(!is_fresh(t, t + SignedDuration::from_secs(60), ttl));
        assert!(!is_fresh(t, t - SignedDuration::from_secs(1), ttl));
    }
}

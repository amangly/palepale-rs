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
    #[serde(default)]
    pub default_max_output_tokens: Option<u64>,
    #[serde(default = "available_by_default")]
    pub available: bool,
    pub pricing: Pricing,
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Input kinds explicitly verified by the gateway. Missing means unknown, never "all".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    /// Optional provenance for capability metadata, kept public and non-sensitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_source: Option<String>,
    /// Public request levels, not private provider tier mappings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
    #[serde(default)]
    pub default_reasoning_effort: Option<String>,
}

fn available_by_default() -> bool {
    true
}

impl Model {
    pub fn supports_effort(&self, effort: &str) -> bool {
        self.capabilities.reasoning
            && self
                .reasoning_efforts
                .as_ref()
                .is_some_and(|levels| levels.iter().any(|level| level == effort))
    }

    /// Reject requests the catalog says this model cannot accept before opening the gateway.
    /// Unknown attachment metadata stays distinguishable from an explicit refusal.
    pub fn preflight(
        &self,
        messages: &[crate::agent::message::Message],
        tool_count: usize,
    ) -> anyhow::Result<()> {
        if tool_count > 0 && !self.capabilities.tool_calling {
            anyhow::bail!(crate::text::template::fill(
                &crate::api::preflight::config().tools,
                &[("model", &self.id)]
            ));
        }
        let mut required = std::collections::BTreeSet::new();
        for message in messages {
            if self.input_modalities.is_some()
                && matches!(message,
                crate::agent::message::Message::User { parts } if parts.iter().any(|part|
                    matches!(part, crate::agent::message::Part::Text { .. })))
            {
                required.insert("text".into());
            }
            let attachments = match message {
                crate::agent::message::Message::User { parts } => parts
                    .iter()
                    .filter_map(|part| match part {
                        crate::agent::message::Part::Attachment(attachment) => Some(attachment),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                crate::agent::message::Message::Tool(result) => result.attachments.iter().collect(),
                crate::agent::message::Message::Assistant(_) => Vec::new(),
            };
            for attachment in attachments {
                required.insert(modality(&attachment.mime_type));
            }
        }
        if required.is_empty() {
            return Ok(());
        }
        let Some(supported) = &self.input_modalities else {
            anyhow::bail!(crate::text::template::fill(
                &crate::api::preflight::config().unknown,
                &[
                    ("model", &self.id),
                    (
                        "kind",
                        &required.iter().cloned().collect::<Vec<_>>().join(", ")
                    )
                ]
            ));
        };
        for kind in required {
            if !supported
                .iter()
                .any(|value| value.eq_ignore_ascii_case(&kind))
            {
                anyhow::bail!(crate::text::template::fill(
                    &crate::api::preflight::config().unsupported,
                    &[("model", &self.id), ("kind", &kind)]
                ));
            }
        }
        Ok(())
    }
}

fn modality(mime: &str) -> String {
    if mime.starts_with("image/") {
        "image".into()
    } else if mime.starts_with("audio/") {
        "audio".into()
    } else if mime == "application/pdf" {
        "pdf".into()
    } else {
        "file".into()
    }
}

/// Capability-only view for the model picker; retain MIME types without duplicating payloads.
pub fn input_requirements(
    messages: &[crate::agent::message::Message],
) -> Vec<crate::agent::message::Message> {
    use crate::agent::message::{Attachment, Message, Part};
    let mut mimes = std::collections::BTreeSet::new();
    for message in messages {
        match message {
            Message::User { parts } => {
                for part in parts {
                    if let Part::Attachment(attachment) = part {
                        mimes.insert(attachment.mime_type.clone());
                    }
                }
            }
            Message::Tool(result) => {
                for attachment in &result.attachments {
                    mimes.insert(attachment.mime_type.clone());
                }
            }
            _ => {}
        }
    }
    vec![Message::User {
        parts: mimes
            .into_iter()
            .map(|mime_type| {
                Part::Attachment(Attachment {
                    mime_type,
                    data: String::new(),
                    filename: None,
                })
            })
            .collect(),
    }]
}

/// USD per million tokens.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pricing {
    #[serde(deserialize_with = "crate::api::decimal::rate")]
    pub input_per_1m_usd: f64,
    #[serde(default, deserialize_with = "crate::api::decimal::rate")]
    pub cached_input_per_1m_usd: f64,
    #[serde(deserialize_with = "crate::api::decimal::rate")]
    pub output_per_1m_usd: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Capabilities {
    pub streaming: bool,
    pub reasoning: bool,
    pub reasoning_content: bool,
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
                .iter()
                .find(|model| model.available)
                .ok_or_else(|| anyhow::anyhow!("No models available from the configured backend."))
        } else {
            self.get(requested).ok_or_else(|| {
                anyhow::anyhow!("unknown model `{requested}`; see `palepale models`")
            })
        }
    }

    pub fn get(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|m| m.id == id && m.available)
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
    fn bun_decimal_rates_parse_without_changing_the_public_prices() {
        let pricing: Pricing = serde_json::from_value(serde_json::json!({
            "input_per_1m_usd":"0.218400000", "cached_input_per_1m_usd":"0.010920000", "output_per_1m_usd":"1.092000000"
        })).unwrap();
        assert_eq!(pricing.input_per_1m_usd, 0.2184);
        assert_eq!(pricing.output_per_1m_usd, 1.092);
        for invalid in ["NaN", "inf", "-1", "not a rate"] {
            assert!(
                serde_json::from_value::<Pricing>(
                    serde_json::json!({"input_per_1m_usd":invalid,"output_per_1m_usd":"1"})
                )
                .is_err()
            );
        }
    }

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
        value["data"][0]
            .as_object_mut()
            .unwrap()
            .remove("input_modalities");
        value["data"][0]
            .as_object_mut()
            .unwrap()
            .remove("capability_source");
        let legacy: Model = serde_json::from_value(value["data"][0].clone()).unwrap();
        let cached: Model = serde_json::from_value(serde_json::to_value(&legacy).unwrap()).unwrap();
        assert!(cached.reasoning_efforts.is_none());
        assert!(cached.input_modalities.is_none() && cached.capability_source.is_none());
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

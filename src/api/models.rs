use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;

use crate::api::Call;
use crate::app::App;
use crate::io;
use crate::limits;

#[cfg(test)]
const FIXTURE: &str = include_str!("../../tests/fixtures/api/models.json");

static ALIASES: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/model-aliases.toml"))
        .expect("model aliases are checked by tests")
});

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Presentation {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub badges: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub presentation: Option<Presentation>,
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
    pub fn name(&self) -> &str {
        if let Some(presentation) = &self.presentation
            && !presentation.name.trim().is_empty()
        {
            return &presentation.name;
        }
        if self.display_name.trim().is_empty() {
            &self.id
        } else {
            &self.display_name
        }
    }

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
        // A backend's exact ID takes precedence over a historical alias.
        self.models
            .iter()
            .find(|m| m.id == id)
            .or_else(|| {
                let current = ALIASES.get(id)?;
                self.models.iter().find(|m| &m.id == current)
            })
            .filter(|m| m.available)
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
    let catalog = load_catalog(app, refresh).await;
    if let Some(saved) = crate::prefs::load(&app.paths).model
        && let Some(model) = catalog.get(&saved)
        && model.id != saved
    {
        crate::prefs::remember_model(&app.paths, &model.id);
    }
    catalog
}

async fn load_catalog(app: &App, refresh: bool) -> Catalog {
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
    let mut listing: Listing =
        serde_json::from_slice(&resp.body).map_err(|e| format!("unreadable model list: {e}"))?;
    // An empty live catalog is authoritative: do not revive unavailable cached models.
    listing.data.retain(|model| model.available);
    Ok(listing.data)
}

fn read_cache(path: &std::path::Path, origin: &str) -> Option<Cached> {
    let text = io::fs::read_string(path).ok().flatten()?;
    serde_json::from_str::<Cached>(&text)
        .ok()
        .filter(|cache| cache.api_origin.as_deref() == Some(origin))
}

fn is_fresh(fetched_at: Timestamp, now: Timestamp, ttl: SignedDuration) -> bool {
    let age = now.duration_since(fetched_at);
    age >= SignedDuration::ZERO && age < ttl
}

fn from_cache(cache: &Cached, note: Option<String>) -> Catalog {
    Catalog {
        models: cache
            .data
            .iter()
            .filter(|model| model.available)
            .cloned()
            .collect(),
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
    fn historical_aliases_require_available_targets_and_preserve_exact_ids() {
        for (old, current) in ALIASES.iter() {
            let mut model = fixture().remove(0);
            model.id.clone_from(current);
            let mut catalog = Catalog {
                models: vec![model],
                source: Source::Live,
                fetched_at: None,
                note: None,
            };
            assert_eq!(catalog.selected(old).unwrap().id, *current);
            catalog.models[0].available = false;
            assert!(catalog.get(old).is_none());
            catalog.models[0].available = true;
            let mut exact = catalog.models[0].clone();
            exact.id.clone_from(old);
            catalog.models.push(exact);
            assert_eq!(catalog.selected(old).unwrap().id, *old);
            catalog.models[1].available = false;
            assert!(catalog.get(old).is_none());
        }
    }

    #[test]
    fn names_use_public_presentation_then_legacy_name_then_id() {
        let mut model = fixture().remove(0);
        assert_eq!(model.name(), model.display_name);
        model.display_name.clear();
        assert_eq!(model.name(), model.id);
        model.presentation = Some(Presentation {
            name: "Public name".into(),
            badges: vec!["Uncensored".into()],
        });
        let cached: Model = serde_json::from_value(serde_json::to_value(&model).unwrap()).unwrap();
        assert_eq!(cached.name(), "Public name");
        model.presentation.as_mut().unwrap().name.clear();
        assert_eq!(model.name(), model.id);
    }

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
    fn an_empty_catalog_is_preserved_and_cache_is_bound_to_origin() {
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
        assert!(
            read_cache(&path, "http://127.0.0.1:8080")
                .unwrap()
                .data
                .is_empty()
        );
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

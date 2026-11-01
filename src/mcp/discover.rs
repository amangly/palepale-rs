//! External definitions are inspected without connecting or running their commands.
use super::Config;
use crate::{extensions::Scope, io, settings, text::template};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, sync::LazyLock};

#[derive(Deserialize)]
struct Sources {
    source: Vec<Source>,
}
#[derive(Deserialize)]
struct Source {
    name: String,
    scope: Scope,
    paths: Vec<String>,
    format: String,
    at: Vec<String>,
    shape: String,
}
static SOURCES: LazyLock<Sources> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/mcp_sources.toml")).expect("MCP sources")
});

pub struct Found {
    pub name: String,
    pub source: String,
    pub scope: Scope,
    pub config: Config,
    pub fingerprint: String,
}

pub fn found(project: &Path) -> Vec<Found> {
    let mut found = BTreeMap::new();
    for source in &SOURCES.source {
        for path in &source.paths {
            let path = template::fill_with(path, |name| {
                if name == "project" {
                    Some(project.to_string_lossy())
                } else {
                    settings::env(name).map(Into::into)
                }
            });
            if path.contains('{') {
                continue;
            }
            let Some(text) = io::fs::read_string(Path::new(&path)).ok().flatten() else {
                continue;
            };
            let document: Option<Value> = match source.format.as_str() {
                "toml" => toml::from_str::<toml::Table>(&text)
                    .ok()
                    .and_then(|v| serde_json::to_value(v).ok()),
                "jsonc" => serde_json::from_str(&crate::text::jsonc::plain(&text)).ok(),
                _ => serde_json::from_str(&text).ok(),
            };
            let Some(document) = document else {
                continue;
            };
            let map = source.at.iter().try_fold(&document, |v, key| {
                v.get(template::fill(
                    key,
                    &[("project", &project.to_string_lossy())],
                ))
            });
            let Some(map) = map.and_then(Value::as_object) else {
                continue;
            };
            for (name, value) in map {
                if !super::valid_name(name) || found.contains_key(name) {
                    continue;
                }
                let Some(config) = config(value, &source.shape) else {
                    continue;
                };
                let fingerprint = crate::extensions::hash(
                    &serde_json::to_vec(&(&path, name, &config)).expect("config serializes"),
                );
                found.insert(
                    name.clone(),
                    Found {
                        name: name.clone(),
                        source: source.name.clone(),
                        scope: source.scope,
                        config,
                        fingerprint,
                    },
                );
            }
        }
    }
    found.into_values().collect()
}

fn config(value: &Value, shape: &str) -> Option<Config> {
    if value.get("disabled").and_then(Value::as_bool) == Some(true)
        || value.get("enabled").and_then(Value::as_bool) == Some(false)
    {
        return None;
    }
    let mut value = value.clone();
    expand(&mut value)?;
    let object = value.as_object()?;
    // Preserve every native field, including OAuth and environment-backed headers.
    let known = serde_json::to_value(Config::default()).ok()?;
    let native: serde_json::Map<String, Value> = object
        .iter()
        .filter(|(key, value)| {
            known.get(*key).is_some() && !(key.as_str() == "command" && value.is_array())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut config: Config = serde_json::from_value(Value::Object(native)).ok()?;
    if shape == "opencode" {
        if let Some(command) = object.get("command").and_then(Value::as_array) {
            config.command = Some(command.first()?.as_str()?.into());
            config.args = command[1..]
                .iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<_>>()?;
        }
    } else {
        config.command = object
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(args) = object.get("args") {
            config.args = serde_json::from_value(args.clone()).ok()?;
        }
    }
    config.url = ["url", "serverUrl", "httpUrl"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str).map(str::to_owned));
    if let Some(env) = object.get("environment").or_else(|| object.get("env")) {
        config.environment = serde_json::from_value(env.clone()).ok()?;
    }
    if let Some(headers) = object.get("http_headers").or_else(|| object.get("headers")) {
        config.http_headers = serde_json::from_value(headers.clone()).ok()?;
    }
    if let Some(cwd) = object.get("cwd").and_then(Value::as_str) {
        config.cwd = Some(cwd.into());
    }
    if object.get("type").and_then(Value::as_str) == Some("sse") {
        config.transport = super::Transport::Sse;
    }
    for (key, target) in [
        ("env_vars", &mut config.env_vars),
        ("scopes", &mut config.scopes),
    ] {
        if let Some(v) = object.get(key) {
            *target = serde_json::from_value(v.clone()).ok()?;
        }
    }
    config.bearer_token_env_var = object
        .get("bearer_token_env_var")
        .and_then(Value::as_str)
        .map(str::to_owned);
    config.validate().ok()?;
    Some(config)
}

fn expand(value: &mut Value) -> Option<()> {
    match value {
        Value::String(text) => {
            let mut output = String::new();
            let mut rest = text.as_str();
            while let Some(open) = rest.find("${") {
                output.push_str(&rest[..open]);
                let after = &rest[open + 2..];
                let close = after.find('}')?;
                let name = after[..close]
                    .strip_prefix("env:")
                    .unwrap_or(&after[..close]);
                let (name, fallback) = name
                    .split_once(":-")
                    .map_or((name, None), |(n, v)| (n, Some(v)));
                let resolved = settings::env(name).or_else(|| fallback.map(str::to_owned))?;
                output.push_str(&resolved);
                rest = &after[close + 1..];
            }
            output.push_str(rest);
            *text = output;
        }
        Value::Array(values) => {
            for value in values {
                expand(value)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                expand(value)?;
            }
        }
        _ => {}
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn supported_shapes_preserve_arguments_and_headers() {
        let program = config(
            &json!({"command":["runner","--stdio"], "environment":{"TOKEN":"secret"}}),
            "opencode",
        )
        .unwrap();
        assert_eq!(program.command.as_deref(), Some("runner"));
        assert_eq!(program.args, ["--stdio"]);
        assert_eq!(program.environment["TOKEN"], "secret");
        let remote = config(
            &json!({"url":"https://example.com/mcp", "headers":{"Authorization":"secret"}}),
            "common",
        )
        .unwrap();
        assert_eq!(remote.http_headers["Authorization"], "secret");
        assert!(config(&json!({"command":"run", "disabled":true}), "common").is_none());
        assert!(config(&json!({"command":"${input:unknown}"}), "common").is_none());
    }
}

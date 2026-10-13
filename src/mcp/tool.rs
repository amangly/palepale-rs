use std::borrow::Cow;
use std::sync::Arc;

use anyhow::{Context, bail};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use crate::extensions::text;
use crate::io;
use crate::limits;
use crate::mcp::Config;
use crate::permission::gate::Ask;
use crate::text::template::fill;
use crate::tools::{Check, Output, Refusal, Spec, Tool, ToolCx};

pub struct Remote {
    spec: Spec,
    pub server: String,
    pub remote_name: String,
    pub description: String,
    pub alias: String,
    pub schema_bytes: usize,
    generation: u64,
    config: Config,
    validator: jsonschema::Validator,
    connection: Arc<io::mcp::Connection>,
}

fn alias(server: &str, name: &str) -> String {
    let plain = format!("mcp__{server}__{name}");
    if plain.len() <= 64
        && plain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return plain;
    }
    let digest = crate::extensions::hash(plain.as_bytes());
    let clean: String = plain
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(46)
        .collect();
    format!("{clean}__{}", &digest[..16])
}

impl Remote {
    pub fn new(
        server: &str,
        config: &Config,
        generation: u64,
        connection: Arc<io::mcp::Connection>,
        value: Value,
    ) -> anyhow::Result<Self> {
        let name = value["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .context("MCP tool is missing its name")?
            .to_owned();
        let schema = value
            .get("inputSchema")
            .context("MCP tool is missing inputSchema")?
            .clone();
        let schema_bytes = serde_json::to_vec(&value)?.len();
        if schema_bytes > limits::get().extensions.tool_schema_bytes.value {
            bail!("MCP tool {name} exceeds its schema size limit");
        }
        if !schema.is_object() || schema["type"] != "object" {
            bail!("MCP tool {name} needs an object input schema");
        }
        if ["anyOf", "oneOf", "allOf"]
            .iter()
            .any(|key| schema.get(key).is_some())
        {
            bail!("MCP tool {name} uses an unsupported root schema combinator");
        }
        let validator = jsonschema::validator_for(&schema)
            .map_err(|e| anyhow::anyhow!("Invalid schema for {name}: {e}"))?;
        let alias = alias(server, &name);
        let description = value["description"].as_str().unwrap_or(&name).to_owned();
        let spec = Spec {
            name: Cow::Owned(alias.clone()),
            description: Cow::Owned(fill(
                &text().mcp_description,
                &[("server", server), ("description", &description)],
            )),
            parameters: schema,
        };
        Ok(Self {
            spec,
            server: server.into(),
            remote_name: name,
            description,
            alias,
            schema_bytes,
            generation,
            config: config.clone(),
            validator,
            connection,
        })
    }
}

impl Tool for Remote {
    fn spec(&self) -> &Spec {
        &self.spec
    }
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        if !cx.app.mcp.active(&self.server, self.generation) || self.connection.closed() {
            return Err(Refusal::Refused(
                "MCP server changed or disconnected; refresh /mcp".into(),
            ));
        }
        self.validator
            .validate(args)
            .map_err(|e| Refusal::InvalidArgs(e.to_string()))?;
        let target = format!("{}.{}", self.server, self.remote_name);
        let mut asks = vec![Ask::new(crate::tools::permissions::MCP.name, &target)];
        if !self.config.read_only_tools.contains(&self.remote_name) {
            asks.push(Ask::new(crate::tools::permissions::MCP_WRITE.name, &target));
        }
        Ok(Check::new(asks).with_preview(format!(
            "{}\n{}",
            self.config.address(),
            serde_json::to_string_pretty(args).unwrap_or_default()
        )))
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            if !cx.app.mcp.active(&self.server, self.generation) {
                return Output::error("MCP server changed before the call started");
            }
            let timeout = self
                .config
                .tool_timeout_secs
                .unwrap_or(limits::get().extensions.call_timeout_secs.value);
            match self
                .connection
                .clone()
                .call(
                    self.remote_name.clone(),
                    args,
                    std::time::Duration::from_secs(timeout),
                    cx.cancel.clone(),
                )
                .await
            {
                Ok(value) => render(value, cx, &self.server, &self.remote_name),
                Err(e) => Output::error(super::redact(&self.config, &format!("{e:#}"))),
            }
        })
    }
}

fn render(value: Value, cx: &ToolCx<'_>, server: &str, tool: &str) -> Output {
    let mut lines = Vec::new();
    let mut unsupported = false;
    if let Some(content) = value["content"].as_array() {
        for item in content {
            if item["type"] == "text" {
                lines.push(item["text"].as_str().unwrap_or_default().to_owned());
            } else {
                unsupported = true;
                lines.push(fill(
                    &text().unsupported,
                    &[("kind", item["type"].as_str().unwrap_or("unknown"))],
                ));
            }
        }
    }
    if let Some(structured) = value.get("structuredContent") {
        lines.push(serde_json::to_string_pretty(structured).unwrap_or_default());
    }
    if lines.is_empty()
        && value.get("content").is_none()
        && value.get("structuredContent").is_none()
    {
        return Output::error(
            "MCP server returned an unsupported response; interactive server requests are not enabled",
        );
    }
    let content = lines.join("\n");
    let mut shown = content.clone();
    let mut full = None;
    if content.len() > limits::get().tools.max_bytes.value
        || content.lines().count() > limits::get().tools.max_lines.value
    {
        let path = cx.app.paths.tmp_dir().join(format!(
            "mcp-{}-{:016x}.txt",
            crate::extensions::hash(cx.call_id.as_bytes()),
            fastrand::u64(..)
        ));
        let cap = limits::get()
            .tools
            .bash_spill_max_bytes
            .value
            .min(content.len());
        let end = content.floor_char_boundary(cap);
        if io::fs::write_atomic(&path, &content.as_bytes()[..end], io::fs::Access::Private).is_ok()
        {
            full = Some(path.display().to_string());
        }
        shown = content
            .lines()
            .take(limits::get().tools.max_lines.value)
            .collect::<Vec<_>>()
            .join("\n");
        let end = shown.floor_char_boundary(limits::get().tools.max_bytes.value.min(shown.len()));
        shown.truncate(end);
        shown.push_str(&format!(
            "\n[Output shortened. Full output: {}]",
            full.as_deref().unwrap_or("could not save")
        ));
    }
    Output {
        content: shown,
        is_error: value["isError"].as_bool().unwrap_or(false) || unsupported,
        details: Some(json!({"server":server,"tool":tool,"full_output":full})),
    }
}

pub struct Search;
impl Tool for Search {
    fn spec(&self) -> &Spec {
        static SPEC: std::sync::LazyLock<Spec> = std::sync::LazyLock::new(|| {
            Spec::load(
                "mcp_search",
                include_str!("../../data/tools/mcp_search.md"),
                include_str!("../../data/tools/mcp_search.schema.json"),
            )
        });
        &SPEC
    }
    fn check(&self, args: &Value, _: &ToolCx<'_>) -> Result<Check, Refusal> {
        if args["query"].as_str().is_none_or(|s| s.trim().is_empty()) {
            return Err(Refusal::InvalidArgs(
                "query must contain search words".into(),
            ));
        }
        Ok(Check::new(vec![]))
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            Output::ok(
                serde_json::to_string(
                    &cx.app
                        .mcp
                        .search(args["query"].as_str().unwrap_or_default()),
                )
                .unwrap_or_default(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_are_bounded_stable_and_do_not_merge_distinct_names() {
        assert_eq!(alias("docs", "search"), "mcp__docs__search");
        let first = alias(&"long".repeat(20), "a/b");
        assert!(first.len() <= 64);
        assert_ne!(first, alias(&"long".repeat(20), "a_b"));
        assert_eq!(first, alias(&"long".repeat(20), "a/b"));
    }
}

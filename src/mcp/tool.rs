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

pub fn alias(server: &str, name: &str) -> String {
    let plain = format!("mcp__{server}__{name}");
    if plain.len() <= 64
        && !server.contains("__")
        && !name.contains("__")
        && plain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return plain;
    }
    let identity = serde_json::to_vec(&(server, name)).expect("MCP names serialize");
    let digest = crate::extensions::hash(&identity);
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

pub(super) fn render(value: Value, cx: &ToolCx<'_>, server: &str, tool: &str) -> Output {
    let mut lines = Vec::new();
    let mut unsupported = false;
    let mut attachments = Vec::new();
    if let Some(content) = value["content"].as_array() {
        for item in content {
            lines.push(content_with_media(item, &mut unsupported, &mut attachments));
        }
    }
    if let Some(contents) = value["contents"].as_array() {
        for resource in contents {
            lines.push(content_with_media(
                &json!({"type":"resource","resource":resource}),
                &mut unsupported,
                &mut attachments,
            ));
        }
    }
    if let Some(messages) = value["messages"].as_array() {
        for message in messages {
            lines.push(format!(
                "{}: {}",
                message["role"].as_str().unwrap_or("message"),
                content_with_media(&message["content"], &mut unsupported, &mut attachments)
            ));
        }
    }
    if let Some(structured) = value.get("structuredContent") {
        lines.push(serde_json::to_string_pretty(structured).unwrap_or_default());
    }
    if lines.is_empty()
        && value.get("content").is_none()
        && value.get("structuredContent").is_none()
        && value.get("contents").is_none()
        && value.get("messages").is_none()
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
        attachments,
        is_error: value["isError"].as_bool().unwrap_or(false) || unsupported,
        details: Some(json!({"server":server,"tool":tool,"full_output":full})),
    }
}

fn content_with_media(
    item: &Value,
    unsupported: &mut bool,
    attachments: &mut Vec<crate::agent::message::Attachment>,
) -> String {
    use base64::Engine;
    let resource = item["type"] == "resource";
    let content = if resource { &item["resource"] } else { item };
    let data = content[if resource { "blob" } else { "data" }].as_str();
    let mime = content["mimeType"]
        .as_str()
        .unwrap_or("application/octet-stream");
    let supported = matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "application/pdf"
            | "audio/wav"
            | "audio/x-wav"
            | "audio/mp3"
            | "audio/mpeg"
    );
    if let Some(data) = data
        && supported
    {
        let cap = limits::get().extensions.skill_asset_bytes.value;
        let total: usize = attachments.iter().map(|a| a.data.len()).sum();
        if data.len() <= cap.saturating_mul(4) / 3 + 4
            && total.saturating_add(data.len()) <= limits::get().extensions.skill_source_bytes.value
            && base64::engine::general_purpose::STANDARD
                .decode(data)
                .is_ok_and(|bytes| bytes.len() <= cap)
        {
            attachments.push(crate::agent::message::Attachment {
                mime_type: mime.into(),
                data: data.into(),
                filename: content["uri"].as_str().map(str::to_owned),
            });
            return format!(
                "Attached {mime}{}",
                content["uri"]
                    .as_str()
                    .map(|s| format!(": {s}"))
                    .unwrap_or_default()
            );
        }
        *unsupported = true;
        return "MCP attachment is invalid or exceeds its size limit".into();
    }
    content_text(item, unsupported)
}

fn resource_text(resource: &Value, unsupported: &mut bool) -> String {
    let body = resource["text"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            *unsupported = true;
            fill(&text().unsupported, &[("kind", "binary resource")])
        });
    format!("{}\n{body}", resource["uri"].as_str().unwrap_or("Resource"))
}

fn content_text(item: &Value, unsupported: &mut bool) -> String {
    match item["type"].as_str() {
        Some("text") => item["text"].as_str().unwrap_or_default().to_owned(),
        Some("resource") => resource_text(&item["resource"], unsupported),
        Some("resource_link") => format!(
            "{}: {}\n{}",
            item["name"].as_str().unwrap_or("Resource"),
            item["uri"].as_str().unwrap_or_default(),
            item["description"].as_str().unwrap_or_default()
        ),
        kind => {
            *unsupported = true;
            fill(&text().unsupported, &[("kind", kind.unwrap_or("unknown"))])
        }
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
        assert_ne!(alias("a__b", "c"), alias("a", "b__c"));
        assert_ne!(alias("a", "搜"), alias("a", "尋"));
    }

    #[test]
    fn binary_content_is_described_without_exposing_base64_as_model_text() {
        let mut unsupported = false;
        let resource =
            json!({"uri":"memory://image","blob":"PRIVATE_BASE64","mimeType":"image/png"});
        let shown = content_text(
            &json!({"type":"resource","resource":resource}),
            &mut unsupported,
        );
        assert!(unsupported && shown.contains("memory://image"));
        assert!(!shown.contains("PRIVATE_BASE64"));
        let shown = content_text(
            &json!({"type":"image","data":"PRIVATE_BASE64"}),
            &mut unsupported,
        );
        assert!(!shown.contains("PRIVATE_BASE64"));
    }

    #[test]
    fn supported_media_is_attached_and_invalid_base64_is_reported() {
        let mut unsupported = false;
        let mut attachments = vec![];
        let shown = content_with_media(
            &json!({"type":"image","mimeType":"image/png","data":"AQID"}),
            &mut unsupported,
            &mut attachments,
        );
        assert!(!unsupported && !shown.contains("AQID"));
        assert_eq!(attachments.len(), 1);
        content_with_media(
            &json!({"type":"resource","resource":{"uri":"doc.pdf","mimeType":"application/pdf","blob":"AQID"}}),
            &mut unsupported,
            &mut attachments,
        );
        content_with_media(
            &json!({"type":"audio","mimeType":"audio/wav","data":"AQID"}),
            &mut unsupported,
            &mut attachments,
        );
        assert_eq!(attachments.len(), 3);
        content_with_media(
            &json!({"type":"image","mimeType":"image/png","data":"!invalid!"}),
            &mut unsupported,
            &mut attachments,
        );
        assert!(unsupported);
        assert_eq!(attachments.len(), 3);
    }
}

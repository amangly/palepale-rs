use super::{Runtime, State};
use crate::tools::{Check, Output, Refusal, Spec, Tool, ToolCx};
use crate::{io, limits};
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Duration;

struct Endpoint {
    generation: u64,
    config: super::Config,
    connection: Arc<io::mcp::Connection>,
    timeout: u64,
}
struct Catalog {
    spec: Spec,
    prompts: bool,
    runtime: Weak<Runtime>,
    endpoints: BTreeMap<String, Endpoint>,
}

pub fn tools(runtime: &Arc<Runtime>) -> Vec<Arc<dyn Tool>> {
    let catalogs: Vec<Arc<Catalog>> = [false, true]
        .into_iter()
        .filter_map(|prompts| {
            let endpoints: BTreeMap<_, _> = runtime
                .servers
                .read()
                .unwrap()
                .iter()
                .filter(|(_, s)| s.state == State::Connected)
                .filter_map(|(name, s)| {
                    s.connection
                        .as_ref()
                        .filter(|c| c.supports(if prompts { "prompts" } else { "resources" }))
                        .map(|c| {
                            (
                                name.clone(),
                                Endpoint {
                                    generation: s.generation,
                                    config: s.config.clone(),
                                    connection: c.clone(),
                                    timeout: s.config.tool_timeout_secs.unwrap_or(
                                        limits::get().extensions.call_timeout_secs.value,
                                    ),
                                },
                            )
                        })
                })
                .collect();
            if endpoints.is_empty() {
                return None;
            }
            let mut spec = if prompts {
                Spec::load(
                    "mcp_prompts",
                    include_str!("../../data/tools/mcp_prompts.md"),
                    include_str!("../../data/tools/mcp_prompts.schema.json"),
                )
            } else {
                Spec::load(
                    "mcp_resources",
                    include_str!("../../data/tools/mcp_resources.md"),
                    include_str!("../../data/tools/mcp_resources.schema.json"),
                )
            };
            spec.parameters["properties"]["server"]["enum"] =
                json!(endpoints.keys().collect::<Vec<_>>());
            Some(Arc::new(Catalog {
                spec,
                prompts,
                runtime: Arc::downgrade(runtime),
                endpoints,
            }))
        })
        .collect();
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for catalog in catalogs {
        if !catalog.prompts {
            for (name, action, description, schema) in [
                (
                    "list_mcp_resources",
                    "list",
                    include_str!("../../data/tools/list_mcp_resources.md"),
                    include_str!("../../data/tools/list_mcp_resources.schema.json"),
                ),
                (
                    "list_mcp_resource_templates",
                    "templates",
                    include_str!("../../data/tools/list_mcp_resource_templates.md"),
                    include_str!("../../data/tools/list_mcp_resource_templates.schema.json"),
                ),
                (
                    "read_mcp_resource",
                    "read",
                    include_str!("../../data/tools/read_mcp_resource.md"),
                    include_str!("../../data/tools/read_mcp_resource.schema.json"),
                ),
            ] {
                let mut spec = Spec::load(name, description, schema);
                spec.parameters["properties"]["server"]["enum"] =
                    json!(catalog.endpoints.keys().collect::<Vec<_>>());
                tools.push(Arc::new(Resources {
                    spec,
                    catalog: catalog.clone(),
                    action,
                }));
            }
        }
        tools.push(catalog);
    }
    tools
}

struct Resources {
    spec: Spec,
    catalog: Arc<Catalog>,
    action: &'static str,
}

impl Resources {
    fn requests(&self, args: &Value) -> Result<Vec<Value>, Refusal> {
        if let Some(server) = args.get("server") {
            if !server.is_string() {
                return Err(Refusal::InvalidArgs("server must be a string".into()));
            }
            let mut request = args.clone();
            request["action"] = json!(self.action);
            return Ok(vec![request]);
        }
        if self.action == "read" {
            return Err(Refusal::InvalidArgs("server is required".into()));
        }
        Ok(self
            .catalog
            .endpoints
            .keys()
            .map(|server| json!({"server":server,"action":self.action}))
            .collect())
    }
}

impl Tool for Resources {
    fn spec(&self) -> &Spec {
        &self.spec
    }
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let mut asks = Vec::new();
        for request in self.requests(args)? {
            asks.extend(self.catalog.check(&request, cx)?.asks);
        }
        Ok(Check::new(asks).with_preview(args.to_string()))
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let requests = match self.requests(&args) {
                Ok(requests) => requests,
                Err(e) => return Output::error(e.to_string()),
            };
            let mut result = Output::ok("");
            for request in requests {
                let output = self.catalog.run(request.clone(), cx).await;
                if !result.content.is_empty() {
                    result.content.push('\n');
                }
                result.content.push_str(&format!(
                    "{}\n{}",
                    request["server"].as_str().unwrap_or_default(),
                    output.content
                ));
                result.attachments.extend(output.attachments);
                result.is_error |= output.is_error;
                if cx.cancel.is_cancelled() {
                    break;
                }
            }
            result
        })
    }
}

impl Catalog {
    fn endpoint(&self, args: &Value) -> Result<(&str, &Endpoint), Refusal> {
        let name = args["server"]
            .as_str()
            .ok_or_else(|| Refusal::InvalidArgs("server is required".into()))?;
        let (name, endpoint) = self.endpoints.get_key_value(name).ok_or_else(|| {
            Refusal::Refused("Server is unavailable or does not support this capability".into())
        })?;
        if self
            .runtime
            .upgrade()
            .is_none_or(|r| !r.active(name, endpoint.generation))
            || endpoint.connection.closed()
        {
            return Err(Refusal::Refused(
                "MCP server changed; discover its current capabilities again".into(),
            ));
        }
        Ok((name, endpoint))
    }
    fn operation(
        &self,
        args: &Value,
    ) -> Result<(&'static str, Value, Option<&'static str>), Refusal> {
        let required = |key: &str| {
            args[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| Refusal::InvalidArgs(format!("{key} is required")))
        };
        match (self.prompts, args["action"].as_str()) {
            (false, Some("list")) => Ok(("resources/list", json!({}), Some("resources"))),
            (false, Some("templates")) => Ok((
                "resources/templates/list",
                json!({}),
                Some("resourceTemplates"),
            )),
            (false, Some("read")) => Ok(("resources/read", json!({"uri":required("uri")?}), None)),
            (true, Some("list")) => Ok(("prompts/list", json!({}), Some("prompts"))),
            (true, Some("get")) => {
                let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
                if arguments
                    .as_object()
                    .is_none_or(|m| m.values().any(|v| !v.is_string()))
                {
                    return Err(Refusal::InvalidArgs(
                        "Prompt arguments must be strings".into(),
                    ));
                }
                Ok((
                    "prompts/get",
                    json!({"name":required("name")?,"arguments":arguments}),
                    None,
                ))
            }
            _ => Err(Refusal::InvalidArgs("Unsupported catalog action".into())),
        }
    }
}

impl Tool for Catalog {
    fn spec(&self) -> &Spec {
        &self.spec
    }
    fn check(&self, args: &Value, _: &ToolCx<'_>) -> Result<Check, Refusal> {
        let (server, _) = self.endpoint(args)?;
        let (method, _, _) = self.operation(args)?;
        Ok(Check::new(vec![crate::permission::gate::Ask::new(
            "mcp",
            format!("{server}.{method}"),
        )])
        .with_preview(args.to_string()))
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let result = async {
                let (server, endpoint) = self.endpoint(&args)?;
                let (method, mut params, field) = self.operation(&args)?;
                let mut items = vec![];
                let mut cursors = HashSet::new();
                for _ in 0..limits::get().extensions.list_pages.value {
                    self.endpoint(&args)?;
                    let result = endpoint.connection.clone()
                        .request(method, params.clone(), Duration::from_secs(endpoint.timeout), cx.cancel.clone())
                        .await
                        .map_err(|e| Refusal::Refused(super::redact(&endpoint.config, &format!("{e:#}"))))?;
                    let Some(field) = field else {
                        return Ok(super::tool::render(result, cx, server, method));
                    };
                    let page = result[field].as_array().ok_or_else(||Refusal::Refused("Malformed MCP catalog response".into()))?;
                    items.extend(page.iter().cloned());
                    if items.len() > limits::get().extensions.tool_count.value { return Err(Refusal::Refused("MCP catalog exceeded its item limit".into())); }
                    let Some(cursor) = result["nextCursor"].as_str() else {
                        return Ok(super::tool::render(json!({"content":[{"type":"text","text":serde_json::to_string_pretty(&items).unwrap_or_default()}]}), cx, server, method));
                    };
                    if !cursors.insert(cursor.to_owned()) { return Err(Refusal::Refused("MCP catalog repeated its pagination cursor".into())); }
                    params["cursor"] = json!(cursor);
                }
                Err(Refusal::Refused("MCP catalog exceeded its page limit".into()))
            }.await;
            result.unwrap_or_else(|e| Output::error(e.to_string()))
        })
    }
}

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse};
use crate::api::Call;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::LazyLock, time::Duration};

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "websearch",
        include_str!("../../data/tools/websearch.md"),
        include_str!("../../data/tools/websearch.schema.json"),
    )
});
static DATA: LazyLock<Data> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/tools/websearch.toml"))
        .expect("websearch configuration")
});
#[derive(Deserialize)]
struct Data {
    max_query_bytes: usize,
    max_output_tokens: u64,
    timeout_seconds: u64,
    unavailable: String,
    invalid: String,
    cancelled: String,
    sources: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    query: String,
}
pub struct WebSearch;

impl Tool for WebSearch {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        if args.query.trim().is_empty() || args.query.len() > DATA.max_query_bytes {
            return Err(Refusal::InvalidArgs(DATA.invalid.clone()));
        }
        let Some(run) = cx
            .run
            .filter(|run| run.model.capabilities.web_search && !cx.app.api.uses_trial())
        else {
            return Err(Refusal::Refused(DATA.unavailable.clone()));
        };
        Ok(Check::new(vec![crate::permission::gate::Ask::new(
            super::permissions::WEBSEARCH.name,
            &run.model.id,
        )])
        .with_preview(args.query))
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            if let Err(error) = self.check(&args, cx) {
                return Output::error(error.to_string());
            }
            tokio::select! {
                result = search(&args, cx) => match result { Ok(output) => output, Err(error) => Output::error(error.to_string()) },
                () = cx.cancel.cancelled() => Output::error(DATA.cancelled.clone()),
            }
        })
    }
}

async fn search(args: &Value, cx: &ToolCx<'_>) -> anyhow::Result<Output> {
    let run = cx
        .run
        .ok_or_else(|| anyhow::anyhow!(DATA.unavailable.clone()))?;
    let mut body = json!({"model":run.model.id,"messages":[{"role":"user","content":args["query"]}],"web_search_options":{"search_context_size":"low"},"max_tokens":DATA.max_output_tokens.min(run.model.max_output_tokens),"stream":false});
    if run.model.capabilities.reasoning_content {
        body["include_reasoning"] = false.into();
    }
    if let Some(id) = cx.app.api.budget_id().await? {
        body["budget_id"] = id.into();
    }
    let operation = crate::session::entry::new_id();
    // One dispatch, using the parent model and budget. No client tools in native search.
    let response = cx
        .app
        .api
        .call(
            "chat",
            Call {
                body: Some(&body),
                headers: &[("Idempotency-Key", &operation)],
                ..Call::default()
            },
            Duration::from_secs(DATA.timeout_seconds),
        )
        .await?;
    let value: Value = serde_json::from_slice(&response.body)?;
    let mut output = Output::ok(render(&value)?);
    if let Some(raw) = value.get("usage") {
        let tokens = serde_json::from_value(raw.clone())?;
        let count = raw["server_tool_use_details"]["web_search_requests"]
            .as_u64()
            .filter(|n| *n <= 1)
            .ok_or_else(|| anyhow::anyhow!(DATA.invalid.clone()))?;
        let mut usage = crate::api::chat::cost(run.model, &tokens);
        usage.cost_micros = usage.cost_micros.saturating_add(
            (run.model.pricing.web_search_per_request_usd * count as f64 * 1_000_000.0).ceil()
                as u64,
        );
        output.details = Some(json!({"usage": usage}));
    }
    Ok(output)
}

fn render(value: &Value) -> anyhow::Result<String> {
    let message = &value["choices"][0]["message"];
    let mut result = message["content"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!(DATA.invalid.clone()))?
        .to_owned();
    let mut links = Vec::new();
    if let Some(annotations) = message["annotations"].as_array() {
        anyhow::ensure!(annotations.len() <= 40, "{}", DATA.invalid);
        for annotation in annotations {
            let Some(url) = annotation["url_citation"]["url"].as_str() else {
                continue;
            };
            let parsed = url::Url::parse(url)?;
            anyhow::ensure!(
                matches!(parsed.scheme(), "http" | "https")
                    && parsed.username().is_empty()
                    && parsed.password().is_none(),
                "{}",
                DATA.invalid
            );
            let url = parsed.to_string();
            if !links.contains(&url) {
                links.push(url);
            }
        }
    }
    if !links.is_empty() {
        result.push_str(&format!("\n\n{}\n- {}", DATA.sources, links.join("\n- ")));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sources_are_safe_and_private_metadata_stays_out() {
        let value = json!({"choices":[{"message":{"content":"Answer","annotations":[{"url_citation":{"url":"https://example.com/"}},{"url_citation":{"url":"https://example.com/"}}]}}],"provider":"private"});
        assert_eq!(
            render(&value).unwrap(),
            "Answer\n\nSources\n- https://example.com/"
        );
        let bad = json!({"choices":[{"message":{"content":"Answer","annotations":[{"url_citation":{"url":"javascript:alert(1)"}}]}}]});
        assert!(render(&bad).is_err());
    }
}

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse};
use crate::api::Call;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{sync::LazyLock, time::Duration};
static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "decision",
        include_str!("../../data/tools/decision.md"),
        include_str!("../../data/tools/decision.schema.json"),
    )
});
static DATA: LazyLock<Data> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/tools/decision.toml")).expect("decision configuration")
});
#[derive(Deserialize)]
struct Data {
    model: String,
    models: Vec<String>,
    max_input_bytes: usize,
    max_questions: usize,
    max_options: usize,
    min_levels: usize,
    max_levels: usize,
    timeout_seconds: u64,
    invalid: String,
    unavailable: String,
    preview: String,
    cancelled: String,
    response_error: String,
    heading: String,
    usage: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    #[serde(default = "default_model")]
    model: String,
    state: Value,
    questions: Map<String, Value>,
}
fn default_model() -> String {
    DATA.model.clone()
}
pub struct Decision;
impl Tool for Decision {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        if !valid(&args) {
            return Err(Refusal::InvalidArgs(DATA.invalid.clone()));
        }
        if cx.app.api.uses_trial() {
            return Err(Refusal::Refused(DATA.unavailable.clone()));
        }
        Ok(Check {
            asks: vec![crate::permission::gate::Ask::new(
                super::permissions::DECISION.name,
                &args.model,
            )],
            preview: Some(
                DATA.preview
                    .replace("{count}", &args.questions.len().to_string()),
            ),
            confirm: true,
        })
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            if let Err(error) = self.check(&args, cx) {
                return Output::error(error.to_string());
            }
            tokio::select! {
             result=decide(args,cx)=>match result {Ok(output)=>output,Err(error)=>Output::error(error.to_string())},
             ()=cx.cancel.cancelled()=>Output::error(DATA.cancelled.clone()),
            }
        })
    }
}
fn valid(args: &Args) -> bool {
    if !DATA.models.contains(&args.model)
        || !(args.state.is_object()
            || args.state.is_array()
            || args.state.as_str().is_some_and(|s| !s.trim().is_empty()))
        || args.questions.is_empty()
        || args.questions.len() > DATA.max_questions
    {
        return false;
    }
    for (id, q) in &args.questions {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return false;
        }
        let Some(q) = q.as_object() else {
            return false;
        };
        if q.keys()
            .any(|k| !matches!(k.as_str(), "type" | "instructions" | "criteria"))
        {
            return false;
        }
        let Some(i) = q.get("instructions") else {
            return false;
        };
        if !(i.is_object() || i.is_array() || i.as_str().is_some_and(|s| !s.trim().is_empty())) {
            return false;
        }
        let text = |v: &Value| v.as_str().is_some_and(|s| !s.is_empty());
        match q.get("type").and_then(Value::as_str) {
            Some("choice") => {
                if !q
                    .get("criteria")
                    .and_then(Value::as_object)
                    .is_some_and(|c| {
                        !c.is_empty()
                            && c.len() <= DATA.max_options
                            && c.iter().all(|(k, v)| !k.is_empty() && text(v))
                    })
                {
                    return false;
                }
            }
            Some("score") => {
                if !q
                    .get("criteria")
                    .and_then(Value::as_array)
                    .is_some_and(|c| {
                        c.len() >= DATA.min_levels
                            && c.len() <= DATA.max_levels
                            && c.iter().all(text)
                    })
                {
                    return false;
                }
            }
            Some("noul") => {
                if q.get("criteria").is_some_and(|c| {
                    !c.as_object().is_some_and(|c| {
                        c.iter()
                            .all(|(k, v)| matches!(k.as_str(), "true" | "false") && text(v))
                    })
                }) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    serde_json::to_vec(&json!({"state":args.state,"questions":args.questions}))
        .is_ok_and(|v| v.len() <= DATA.max_input_bytes)
}
async fn decide(value: Value, cx: &ToolCx<'_>) -> anyhow::Result<Output> {
    let args: Args = serde_json::from_value(value)?;
    let mut body = json!({"model":args.model,"state":args.state,"questions":args.questions});
    if let Some(id) = cx.app.api.budget_id().await? {
        body["budget_id"] = id.into();
    }
    let response = cx
        .app
        .api
        .call(
            "decision",
            Call {
                body: Some(&body),
                ..Call::default()
            },
            Duration::from_secs(DATA.timeout_seconds),
        )
        .await?;
    let response: Value = serde_json::from_slice(&response.body)?;
    Ok(Output::ok(render(&response, &args)?))
}
fn render(value: &Value, args: &Args) -> anyhow::Result<String> {
    let error = || anyhow::anyhow!(DATA.response_error.clone());
    let input = value["usage"]["input_tokens"].as_u64().ok_or_else(error)?;
    let output = value["usage"]["output_tokens"].as_u64().ok_or_else(error)?;
    let mut result = DATA.heading.clone();
    for (id, q) in &args.questions {
        let answer = &value["answers"][id];
        let kind = q["type"].as_str().ok_or_else(error)?;
        anyhow::ensure!(answer["type"] == kind, "{}", DATA.response_error);
        let v = match kind {
            "choice" => {
                let v = answer["choice"].as_str().ok_or_else(error)?;
                anyhow::ensure!(q["criteria"].get(v).is_some(), "{}", DATA.response_error);
                v.to_owned()
            }
            "noul" | "score" => {
                let v = answer[kind].as_f64().ok_or_else(error)?;
                let max = if kind == "noul" {
                    1.0
                } else {
                    q["criteria"]
                        .as_array()
                        .ok_or_else(error)?
                        .len()
                        .saturating_sub(1) as f64
                };
                anyhow::ensure!(
                    v.is_finite() && v >= 0.0 && v <= max,
                    "{}",
                    DATA.response_error
                );
                v.to_string()
            }
            _ => return Err(error()),
        };
        result.push_str(&format!("\n{id}: {v}"));
        if let Some(c) = answer.get("confidence") {
            let c = c.as_f64().ok_or_else(error)?;
            anyhow::ensure!((0.0..=1.0).contains(&c), "{}", DATA.response_error);
            result.push_str(&format!(" ({c})"));
        }
    }
    result.push_str("\n\n");
    result.push_str(
        &DATA
            .usage
            .replace("{input}", &input.to_string())
            .replace("{output}", &output.to_string()),
    );
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args() -> Args {
        serde_json::from_value(json!({"state":"diff + tests","questions":{"review":{"type":"choice","instructions":"Review","criteria":{"accept":"ready","review":"needs review"}}}})).unwrap()
    }
    #[test]
    fn validates_bounds_and_free_spelling() {
        assert!(valid(&args()));
        let mut a = args();
        a.model = "jev-latest:free".into();
        assert!(valid(&a));
        a.questions.clear();
        assert!(!valid(&a));
    }
    #[test]
    fn returns_only_requested_answers_and_usage() {
        let a = args();
        let v = json!({"answers":{"review":{"type":"choice","choice":"review","confidence":1},"private":{"secret":"hidden"}},"usage":{"input_tokens":346,"output_tokens":31},"provider":"hidden"});
        let text = render(&v, &a).unwrap();
        assert!(text.contains("review: review"));
        assert!(text.contains("346 input, 31 output"));
        assert!(!text.contains("hidden"));
        assert!(render(&json!({"usage":{"input_tokens":1,"output_tokens":1}}), &a).is_err());
    }
    #[test]
    fn confirmation_is_mandatory() {
        let p = super::super::testing::Project::new();
        let value = SPEC.parameters["examples"][0].clone();
        assert!(Decision.check(&value, &p.cx()).unwrap().confirm);
    }
}

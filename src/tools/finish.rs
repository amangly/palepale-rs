//! Completion is accepted by the run loop only after a successful batch and verification.
use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::Value;

use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail};

pub const NAME: &str = "finish";
static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        NAME,
        include_str!("../../data/tools/finish.md"),
        include_str!("../../data/tools/finish.schema.json"),
    )
});
#[derive(Deserialize)]
struct Text {
    blocked_modes: Vec<String>,
    blocked: String,
}
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/tools/finish.toml")).expect("finish configuration")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    summary: Option<String>,
}

pub struct Finish;
impl Tool for Finish {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let _: Args = parse(args)?;
        if TEXT.blocked_modes.contains(&cx.shared.mode().id) {
            return Err(Refusal::Refused(TEXT.blocked.clone()));
        }
        Ok(Check::new(vec![]))
    }
    fn run<'a>(&'a self, args: Value, _cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            match parse_or_fail::<Args>(&args) {
                Ok(args) => Output::ok(args.summary.unwrap_or_default().trim().to_owned()),
                Err(output) => output,
            }
        })
    }
}

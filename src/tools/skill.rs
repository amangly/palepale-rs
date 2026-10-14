use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail, target};
use crate::skills;
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::LazyLock;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "skill",
        include_str!("../../data/tools/skill.md"),
        include_str!("../../data/tools/skill.schema.json"),
    )
});
#[derive(Deserialize)]
struct Args {
    name: String,
}
pub struct Skill;

fn find(cx: &ToolCx<'_>, name: &str) -> Result<skills::Skill, Refusal> {
    skills::discover(cx.app)
        .into_iter()
        .find(|s| s.name == name && s.state == skills::State::Ready && !s.explicit_only)
        .ok_or_else(|| {
            Refusal::Refused(format!(
                "Skill unavailable for automatic use: {name}. Check /skills."
            ))
        })
}

impl Tool for Skill {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let skill = find(cx, &args.name)?;
        let target = target::resolve(&cx.app.paths, &skill.path.to_string_lossy())
            .map_err(Refusal::Refused)?;
        let mut asks = target::asks("read", &target);
        asks.push(crate::permission::gate::Ask::new(
            super::permissions::SKILL.name,
            &args.name,
        ));
        Ok(Check::new(asks).with_preview(target.shown))
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let result = (|| {
                let args: Args = parse_or_fail(&args)?;
                let skill = find(cx, &args.name).map_err(|e| Output::error(e.to_string()))?;
                let content = skills::context(&skill).map_err(|e| Output::error(e.to_string()))?;
                Ok(Output::ok(content)
                    .with_details(json!({"skill":skill.name,"source":skill.path})))
            })();
            result.unwrap_or_else(|e| e)
        })
    }
}

//! The optional local decompiler uses the same path permissions as file reads.
use super::{Check, Output, Refusal, Spec, Tool, ToolCx, parse, parse_or_fail, target, truncate};
use crate::{io, limits, permission::catalog, text::template::fill};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::LazyLock;

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec::load(
        "decompile",
        include_str!("../../data/tools/decompile.md"),
        include_str!("../../data/tools/decompile.schema.json"),
    )
});
#[derive(Deserialize)]
struct Text {
    program: String,
    no_program: String,
    failed: String,
    no_output: String,
    exit_error: String,
    truncated: String,
}
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    toml::from_str(include_str!("../../data/tools/decompile.toml"))
        .expect("decompile configuration")
});
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    binary: String,
    function: Option<String>,
}

impl Args {
    fn argv(&self, binary: String) -> Vec<String> {
        match &self.function {
            Some(function) => vec!["decompile".into(), binary, function.clone()],
            None => vec!["functions".into(), binary],
        }
    }
}

pub struct Decompile;
impl Tool for Decompile {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn check(&self, args: &Value, cx: &ToolCx<'_>) -> Result<Check, Refusal> {
        let args: Args = parse(args)?;
        let target = target::resolve(&cx.app.paths, &args.binary).map_err(Refusal::Refused)?;
        Ok(Check::new(target::asks(catalog::READ.name, &target)))
    }
    fn run<'a>(&'a self, args: Value, cx: &'a ToolCx<'a>) -> BoxFuture<'a, Output> {
        Box::pin(async move {
            let args: Args = match parse_or_fail(&args) {
                Ok(args) => args,
                Err(out) => return out,
            };
            let Some(program) = io::proc::which(&TEXT.program) else {
                return Output::error(TEXT.no_program.clone());
            };
            let target = match target::resolve(&cx.app.paths, &args.binary) {
                Ok(target) => target,
                Err(error) => return Output::error(error),
            };
            let captured = io::exec::exec(&io::exec::Program {
                program: &program,
                args: &args.argv(target.path.to_string_lossy().into_owned()),
                cwd: &cx.app.paths.project,
                env: &[],
                unset: super::shell::unset(),
                timeout: limits::get().tools.decompile_timeout_secs.secs(),
            })
            .await;
            match captured {
                Ok(captured) => render(captured, &target.shown, args.function.is_some()),
                Err(error) => Output::error(fill(
                    &TEXT.failed,
                    &[("binary", &target.shown), ("error", &error.to_string())],
                )),
            }
        })
    }
}

fn render(captured: io::exec::Captured, binary: &str, function: bool) -> Output {
    if !captured.ok() {
        let error = String::from_utf8_lossy(&captured.stderr);
        let error = error.split("\nusage:").next().unwrap_or_default().trim();
        return Output::error(fill(
            &TEXT.failed,
            &[
                ("binary", binary),
                (
                    "error",
                    if error.is_empty() {
                        &TEXT.exit_error
                    } else {
                        error
                    },
                ),
            ],
        ));
    }
    let output = String::from_utf8_lossy(&captured.stdout);
    if output.trim().is_empty() {
        return Output::error(fill(&TEXT.no_output, &[("binary", binary)]));
    }
    let limits = &limits::get().tools;
    let cut = truncate::cut(
        &output,
        truncate::Keep::Head,
        limits.max_lines.value,
        limits.max_bytes.value,
    );
    let suffix = if cut.truncated() {
        fill(&TEXT.truncated, &[("lines", &cut.kept_lines.to_string())])
    } else {
        String::new()
    };
    Output::ok(format!("{}{suffix}", cut.text))
        .with_details(json!({"mode": if function { "decompile" } else { "functions" }}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompiling_outside_the_project_asks_for_external_read_access() {
        let project = crate::tools::testing::Project::new();
        let check = Decompile
            .check(&json!({"binary":"../outside-program"}), &project.cx())
            .unwrap();
        assert_eq!(check.asks.len(), 2);
        assert_eq!(check.asks[0].permission, catalog::READ.name);
        assert_eq!(check.asks[1].permission, catalog::EXTERNAL_DIRECTORY.name);
    }
    #[test]
    fn filenames_and_functions_are_separate_process_arguments() {
        let args = Args {
            binary: "ignored".into(),
            function: Some("main; echo unsafe".into()),
        };
        assert_eq!(
            args.argv("/project/a file".into()),
            ["decompile", "/project/a file", "main; echo unsafe"]
        );
        let output = render(
            io::exec::Captured {
                code: Some(0),
                stdout: b"int main() { return 0; }".to_vec(),
                stderr: vec![],
            },
            "a file",
            true,
        );
        assert!(!output.is_error);
        assert_eq!(output.details.unwrap()["mode"], "decompile");
        let output = render(
            io::exec::Captured {
                code: Some(1),
                stdout: vec![],
                stderr: b"bad format\nusage: long help".to_vec(),
            },
            "a file",
            false,
        );
        assert!(output.is_error);
        assert!(output.content.contains("bad format"));
        assert!(!output.content.contains("long help"));
    }
}

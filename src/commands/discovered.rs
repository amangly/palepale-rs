//! Skills and MCP prompts share the terminal's slash menu. Built-in names always win.
use crate::{
    app::App,
    commands::{self, Effect, Outcome, Scope, Spec},
    skills,
    tools::ToolCx,
};
use serde_json::{Value, json};

#[derive(Clone)]
pub enum Source {
    Skill(String),
    Prompt {
        server: String,
        name: String,
        arguments: Vec<(String, bool)>,
    },
}

#[derive(Clone)]
pub struct Discovered {
    pub spec: Spec,
    pub source: Source,
}

fn spec(name: String, summary: String) -> Spec {
    Spec {
        name,
        summary,
        aliases: vec![],
        usage: String::new(),
        hidden: false,
        requires_backend: true,
        scope: vec![Scope::Session],
        secret: vec![],
    }
}

pub async fn list(app: &App) -> Vec<Discovered> {
    let mut found = Vec::new();
    for skill in skills::load(app).await {
        if skill.state == skills::State::Ready
            && skill.invocable
            && skill.slash
            && commands::find(&skill.name).is_none()
        {
            found.push(Discovered {
                spec: spec(skill.name.clone(), skill.description),
                source: Source::Skill(skill.name),
            });
        }
    }
    for (server, prompt) in app.mcp.prompts().await {
        let Some(name) = prompt["name"].as_str() else {
            continue;
        };
        let alias = format!(
            "{}:{}",
            server,
            name.chars()
                .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                })
                .collect::<String>()
        );
        if commands::find(&alias).is_some() || found.iter().any(|d| d.spec.name == alias) {
            continue;
        }
        let arguments = prompt["arguments"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| {
                a["name"]
                    .as_str()
                    .map(|n| (n.to_owned(), a["required"].as_bool().unwrap_or(false)))
            })
            .collect();
        found.push(Discovered {
            spec: spec(
                alias,
                prompt["description"].as_str().unwrap_or(name).to_owned(),
            ),
            source: Source::Prompt {
                server,
                name: name.to_owned(),
                arguments,
            },
        });
    }
    found.sort_by(|a, b| a.spec.name.cmp(&b.spec.name));
    found
}

impl Discovered {
    pub async fn run(&self, args: &[String]) -> anyhow::Result<Outcome> {
        let text = match &self.source {
            Source::Skill(name) => format!("${name} {}", args.join(" ")),
            Source::Prompt {
                server,
                name,
                arguments,
            } => {
                if args.len() > arguments.len()
                    || arguments.iter().enumerate().any(|(i, (_, required))| {
                        *required && args.get(i).is_none_or(|s| s.is_empty())
                    })
                {
                    anyhow::bail!(
                        "usage: /{} {}",
                        self.spec.name,
                        arguments
                            .iter()
                            .map(|(n, r)| if *r {
                                format!("<{n}>")
                            } else {
                                format!("[{n}]")
                            })
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
                let values: serde_json::Map<String, Value> = arguments
                    .iter()
                    .zip(args)
                    .map(|((n, _), v)| (n.clone(), json!(v)))
                    .collect();
                let request =
                    json!({"server":server,"action":"get","name":name,"arguments":values});
                return Ok(Outcome {
                    text: String::new(),
                    effects: vec![Effect::McpPrompt(request.to_string())],
                });
            }
        };
        Ok(Outcome {
            text: String::new(),
            effects: vec![Effect::Send(text)],
        })
    }
}

pub async fn prompt(args: Value, cx: &ToolCx<'_>) -> crate::tools::Output {
    let registry = crate::tools::Registry::builtin(cx.app).snapshot();
    let Some(tool) = registry.get("mcp_prompts") else {
        return crate::tools::Output::error("MCP prompt server disconnected");
    };
    let check = match tool.check(&args, cx) {
        Ok(check) => check,
        Err(e) => return crate::tools::Output::error(e.to_string()),
    };
    let policy = crate::permission::gate::Policy {
        rules: &cx.app.settings.permission,
        mode: cx.shared.mode(),
        plan_file: cx.plan_file,
    };
    match policy.decide(&check.asks) {
        crate::permission::gate::Decision::Allow => {}
        crate::permission::gate::Decision::Deny(reason) => {
            return crate::tools::Output::error(reason);
        }
        crate::permission::gate::Decision::Ask => {
            let approved = cx
                .ask(crate::agent::prompt::Prompt::Permission {
                    tool: "mcp_prompts".into(),
                    asks: check.asks,
                    preview: check.preview,
                    confirm: check.confirm,
                })
                .await;
            if approved != Some(crate::agent::prompt::Reply::Yes) {
                return crate::tools::Output::error("MCP prompt read was not approved");
            }
        }
    }
    tool.run(args, cx).await
}

pub async fn expand_prompts(
    cx: &crate::agent::run::RunCtx<'_>,
    shared: &crate::agent::state::Shared,
    cancel: &tokio_util::sync::CancellationToken,
    parts: &mut Vec<crate::agent::message::Part>,
) -> bool {
    let mut expanded = Vec::new();
    let mut okay = true;
    for part in std::mem::take(parts) {
        if let crate::agent::message::Part::McpPrompt { request } = part {
            cx.app.mcp.start_enabled(cx.app).await;
            (cx.emit)(crate::agent::event::AgentEvent::ToolStart {
                call_id: "mcp-prompt".into(),
                name: "mcp_prompts".into(),
                arguments: request.to_string(),
            });
            let tcx = ToolCx {
                run: Some(cx),
                app: cx.app,
                shared,
                cwd: shared.cwd().unwrap_or_else(|| cx.app.paths.project.clone()),
                emit: cx.emit,
                call_id: "mcp-prompt",
                plan_file: &cx.plan_file,
                cancel: cancel.clone(),
            };
            let output = prompt(request, &tcx).await;
            (cx.emit)(crate::agent::event::AgentEvent::ToolEnd {
                result: crate::agent::message::ToolResult {
                    call_id: "mcp-prompt".into(),
                    name: "mcp_prompts".into(),
                    content: output.content.clone(),
                    attachments: output.attachments.clone(),
                    is_error: output.is_error,
                    details: output.details.clone(),
                    asks: vec![],
                    refused: None,
                },
            });
            okay &= !output.is_error;
            expanded.push(crate::agent::message::Part::Text {
                text: output.content,
            });
            expanded.extend(
                output
                    .attachments
                    .into_iter()
                    .map(crate::agent::message::Part::Attachment),
            );
        } else {
            expanded.push(part);
        }
    }
    *parts = expanded;
    okay
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn slash_controls_hide_only_the_alias_and_builtin_names_win() {
        let project = crate::tools::testing::Project::new();
        for (name, metadata) in [
            ("native-slash", "metadata:\n  opencode/slash: false"),
            ("native-manual", "disable-model-invocation: true"),
            ("help", ""),
        ] {
            crate::io::fs::write_atomic(
                &project
                    .app
                    .paths
                    .project
                    .join(format!(".agents/skills/{name}/SKILL.md")),
                format!(
                    "---\nname: {name}\ndescription: Workflow {name}\n{metadata}\n---\nInstructions"
                )
                .as_bytes(),
                crate::io::fs::Access::Private,
            )
            .unwrap();
        }
        crate::trust::trust(&project.app.paths).unwrap();
        let commands = list(&project.app).await;
        assert!(
            !commands
                .iter()
                .any(|d| d.spec.name == "help" || d.spec.name == "native-slash")
        );
        let manual = commands
            .iter()
            .find(|d| d.spec.name == "native-manual")
            .unwrap();
        assert_eq!(
            manual.run(&["inspect".into()]).await.unwrap().effects,
            [Effect::Send("$native-manual inspect".into())]
        );
        assert!(
            skills::discover(&project.app)
                .iter()
                .any(|s| s.name == "native-slash" && s.invocable)
        );
    }
}

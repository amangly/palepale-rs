use std::sync::LazyLock;

use anyhow::{anyhow, ensure};
use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Effect, Outcome, Spec};
use crate::api::agent_profiles;
use crate::text::template::fill;

const SRC: &str = include_str!("../../data/commands/godmode.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SRC));
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SRC)
        .expect("data/commands/godmode.toml is checked by tests")
        .text
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Text {
    pub input_color: String,
    pub title: String,
    pub off: String,
    pub off_summary: String,
    pub status: String,
    pub badge: String,
    pub label: String,
    pub unknown: String,
    pub no_profiles: String,
    pub unavailable: String,
    pub custom: String,
    pub custom_summary: String,
    pub custom_prompt_id: String,
    pub custom_opened: String,
    pub custom_applied: String,
    pub custom_invalid: String,
    pub custom_apply: String,
    pub custom_cancel: String,
    pub custom_max_bytes: usize,
}

pub fn text() -> &'static Text {
    &TEXT
}

pub struct Godmode;

fn custom_file(app: &crate::app::App) -> std::path::PathBuf {
    app.paths
        .home
        .join("prompts")
        .join(format!("{}.md", text().custom_prompt_id))
}

pub fn custom_body(app: &crate::app::App) -> anyhow::Result<String> {
    let body = crate::io::fs::read_limited(&custom_file(app), text().custom_max_bytes)?
        .ok_or_else(|| anyhow!(text().custom_invalid.clone()))?;
    let body = body.strip_prefix('\u{feff}').unwrap_or(&body);
    ensure!(!body.trim().is_empty(), text().custom_invalid.clone());
    Ok(body.to_owned())
}

pub fn open_custom(app: &crate::app::App) -> anyhow::Result<String> {
    let path = custom_file(app);
    if crate::io::fs::read_prefix(&path, 0)?.is_none() {
        crate::io::fs::write_atomic(&path, b"", crate::io::fs::Access::Private)?;
    }
    crate::io::proc::open_text_file(&path)?;
    Ok(fill(
        &text().custom_opened,
        &[("path", &path.display().to_string())],
    ))
}

impl Command for Godmode {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let session = cx.session.as_ref().ok_or_else(|| cx.usage(&SPEC))?;
            match args {
                [arg] if arg == "custom" => Ok(Outcome {
                    effects: vec![Effect::EditCustomPrompt],
                    ..Outcome::default()
                }),
                [arg, action] if arg == "custom" && action == "apply" => {
                    custom_body(cx.app)?;
                    Ok(Outcome {
                        text: text().custom_applied.clone(),
                        effects: vec![
                            Effect::SetAgentProfile(None),
                            Effect::SetSystemPrompt(text().custom_prompt_id.clone()),
                        ],
                    })
                }
                [arg] if arg == "status" => Ok(fill(
                    &text().status,
                    &[(
                        "profile",
                        if session.system_prompt == text().custom_prompt_id {
                            &text().custom
                        } else {
                            session.agent_profile.as_deref().unwrap_or(&text().off)
                        },
                    )],
                )
                .into()),
                [arg] if arg == "off" => Ok(Outcome {
                    effects: vec![Effect::SetAgentProfile(None)],
                    ..Outcome::default()
                }),
                [] | [_] => {
                    let profiles = match agent_profiles::load(&cx.app.api, &session.model).await {
                        Ok(profiles) => profiles,
                        Err(_) if args.is_empty() => {
                            return Ok(Outcome {
                                text: text().unavailable.clone(),
                                effects: vec![Effect::PickAgentProfile(vec![])],
                            });
                        }
                        Err(error) => return Err(anyhow!(error)),
                    };
                    let effect = match args {
                        [] => Effect::PickAgentProfile(profiles),
                        [id] => {
                            ensure!(
                                profiles.iter().any(|profile| profile.id == *id),
                                fill(&text().unknown, &[("profile", id)])
                            );
                            Effect::SetAgentProfile(Some(id.clone()))
                        }
                        _ => unreachable!(),
                    };
                    Ok(Outcome {
                        effects: vec![effect],
                        ..Outcome::default()
                    })
                }
                _ => Err(cx.usage(&SPEC)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn custom_prompts_are_local_validated_and_do_not_change_the_default() {
        let root = tempfile::tempdir().unwrap();
        let app = crate::app::App::for_tests(root.path());
        let cx = Ctx {
            app: &app,
            scope: super::super::Scope::Session,
            session: Some(super::super::Session {
                model: "demo".into(),
                mode: "AUTO".into(),
                agent_profile: Some("on".into()),
                system_prompt: "default".into(),
                context_pct: None,
            }),
        };
        let args = ["custom".into(), "apply".into()];
        assert!(Godmode.run(&cx, &args).await.is_err());
        for body in ["", "   ", &"x".repeat(text().custom_max_bytes + 1)] {
            crate::io::fs::write_atomic(
                &custom_file(&app),
                body.as_bytes(),
                crate::io::fs::Access::Private,
            )
            .unwrap();
            assert!(Godmode.run(&cx, &args).await.is_err());
        }
        crate::io::fs::write_atomic(
            &custom_file(&app),
            "\u{feff}CUSTOM_TEST_PROMPT".as_bytes(),
            crate::io::fs::Access::Private,
        )
        .unwrap();
        crate::trust::trust(&app.paths).unwrap();
        crate::io::fs::write_atomic(
            &app.paths
                .project
                .join(".palepale/prompts/godmode-custom.md"),
            b"PROJECT_OVERRIDE",
            crate::io::fs::Access::Shared,
        )
        .unwrap();
        let result = Godmode.run(&cx, &args).await.unwrap();
        assert_eq!(
            result.effects,
            [
                Effect::SetAgentProfile(None),
                Effect::SetSystemPrompt(text().custom_prompt_id.clone())
            ]
        );
        assert_eq!(
            super::super::prompt::body(&app, &text().custom_prompt_id)
                .unwrap()
                .as_deref(),
            Some("CUSTOM_TEST_PROMPT")
        );
        assert!(
            crate::io::fs::read_limited(&app.paths.user_config(), 32768)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn status_and_off_work_without_fetching_the_catalog() {
        let root = tempfile::tempdir().unwrap();
        let app = crate::app::App::for_tests(root.path());
        let cx = Ctx {
            app: &app,
            scope: super::super::Scope::Session,
            session: Some(super::super::Session {
                model: "demo".into(),
                mode: "AUTO".into(),
                agent_profile: Some("deep".into()),
                system_prompt: "default".into(),
                context_pct: None,
            }),
        };
        let status = Godmode.run(&cx, &["status".into()]).await.unwrap();
        assert_eq!(status.text, "Coding profile: deep");
        assert!(status.effects.is_empty());
        assert_eq!(
            Godmode.run(&cx, &["off".into()]).await.unwrap().effects,
            [Effect::SetAgentProfile(None)]
        );
        assert!(
            Godmode
                .run(&cx, &["deep".into(), "extra".into()])
                .await
                .is_err()
        );
    }
}

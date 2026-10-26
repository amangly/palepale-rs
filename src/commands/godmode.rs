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
}

pub fn text() -> &'static Text {
    &TEXT
}

pub struct Godmode;

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
                [arg] if arg == "status" => Ok(fill(
                    &text().status,
                    &[(
                        "profile",
                        session.agent_profile.as_deref().unwrap_or(&text().off),
                    )],
                )
                .into()),
                [arg] if arg == "off" => Ok(Outcome {
                    effects: vec![Effect::SetAgentProfile(None)],
                    ..Outcome::default()
                }),
                [] | [_] => {
                    let profiles = agent_profiles::load(&cx.app.api, &session.model)
                        .await
                        .map_err(|error| {
                            if error.info.status == Some(404) {
                                anyhow!(text().unavailable.clone())
                            } else {
                                anyhow!(error)
                            }
                        })?;
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

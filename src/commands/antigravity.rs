use super::{Command, Ctx, Outcome, Spec};
use crate::{
    api::{Call, account, endpoints},
    auth, io, settings,
};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::{path::PathBuf, sync::LazyLock};

const SOURCE: &str = include_str!("../../data/commands/antigravity.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SOURCE));

#[derive(Deserialize)]
struct Text {
    ready: String,
    disconnected: String,
}

#[derive(Deserialize)]
struct Config {
    text: Text,
}

static TEXT: LazyLock<Config> = LazyLock::new(|| toml::from_str(SOURCE).expect("antigravity text"));

pub struct Antigravity;

impl Command for Antigravity {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            anyhow::ensure!(
                args.first().is_some_and(|s| s == "--run")
                    || (args.len() <= 1
                        && args
                            .first()
                            .is_none_or(|s| s == "--logout" || !s.starts_with('-'))),
                "Usage: palepale {}",
                SPEC.usage
            );
            let root = io::antigravity::settings_dir(
                settings::env("ANTIGRAVITY_CONFIG_DIR").map(PathBuf::from),
                settings::env("ANTIGRAVITY_IDE_SERVER_ROOT").map(PathBuf::from),
                io::fs::home_dir(),
            )?;
            if args.first().is_some_and(|arg| arg == "--run") {
                let forwarded = if args.get(1).is_some_and(|arg| arg == "--") {
                    &args[2..]
                } else {
                    &args[1..]
                };
                io::antigravity::launch(&root, &endpoints::origin("api"), forwarded).await?;
                return Ok(Outcome::default());
            }
            if args.first().is_some_and(|arg| arg == "--logout") {
                io::antigravity::disconnect(&root)?;
                return Ok(TEXT.text.disconnected.clone().into());
            }
            if !cx.app.api.has_key() {
                io::term::out(&format!("{}\n", super::login::pair_here(cx.app).await?));
            }
            account::me(&cx.app.api).await?;
            let key = auth::load(&cx.app.paths)?
                .ok_or_else(|| anyhow::anyhow!("Sign in with palepale login first"))?;
            let catalog = cx
                .app
                .api
                .call(
                    "client_config",
                    Call::default(),
                    std::time::Duration::from_secs(io::clients::config().setup_timeout_secs),
                )
                .await?;
            let model = io::antigravity::install(
                &root,
                &catalog.body,
                &key.secret,
                &endpoints::origin("api"),
                (args.first().map(String::as_str), &cx.app.settings.model),
            )?;
            Ok(TEXT.text.ready.replace("{model}", &model).into())
        })
    }
}

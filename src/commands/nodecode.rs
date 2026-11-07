use super::{Command, Ctx, Outcome, Spec};
use crate::{
    api::{Call, account, endpoints},
    auth, io, settings,
    text::template::fill,
};
use futures_util::future::BoxFuture;
use std::{path::PathBuf, sync::LazyLock};

const SOURCE: &str = include_str!("../../data/commands/nodecode.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SOURCE));
pub struct NodeCode;
impl Command for NodeCode {
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
                args.len() <= 1
                    && args
                        .first()
                        .is_none_or(|s| s == "--logout" || !s.starts_with('-')),
                "Usage: palepale {}",
                SPEC.usage
            );
            let root = match settings::env("NODECODE_HOME") {
                Some(path) => PathBuf::from(path),
                None => io::nodecode::active_home(
                    &io::fs::home_dir()
                        .ok_or_else(|| anyhow::anyhow!("No user home directory"))?
                        .join(".nodecode"),
                )?,
            };
            anyhow::ensure!(root.is_absolute(), "NODECODE_HOME must be an absolute path");
            let text: toml::Value = toml::from_str(SOURCE)?;
            if args.first().is_some_and(|arg| arg == "--logout") {
                io::nodecode::disconnect(&root)?;
                return Ok(text["text"]["disconnected"]
                    .as_str()
                    .expect("disconnect text")
                    .into());
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
            let model = io::nodecode::install(
                &root,
                &catalog.body,
                &key.secret,
                &endpoints::origin("api"),
                (args.first().map(String::as_str), &cx.app.settings.model),
            )?;
            Ok(fill(
                text["text"]["ready"].as_str().expect("setup text"),
                &[("model", &model)],
            )
            .into())
        })
    }
}

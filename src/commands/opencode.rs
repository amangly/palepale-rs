use super::{Command, Ctx, Outcome, Spec};
use crate::{
    api::{Call, account, endpoints},
    auth, io, limits, settings,
};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::sync::LazyLock;

const SOURCE: &str = include_str!("../../data/commands/opencode.toml");
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
static TEXT: LazyLock<Config> = LazyLock::new(|| toml::from_str(SOURCE).expect("opencode text"));
pub struct OpenCode;
impl Command for OpenCode {
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
                args.is_empty() || args == ["--logout"],
                "Usage: palepale {}",
                SPEC.usage
            );
            let root = settings::env("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or(
                    io::fs::home_dir()
                        .ok_or_else(|| anyhow::anyhow!("No user home directory"))?
                        .join(".config"),
                )
                .join("opencode");
            anyhow::ensure!(
                root.is_absolute(),
                "XDG_CONFIG_HOME must be an absolute path"
            );
            if !args.is_empty() {
                io::opencode::disconnect(&root)?;
                return Ok(TEXT.text.disconnected.clone().into());
            }
            if !cx.app.api.has_key() {
                io::term::out(&format!("{}\n", super::login::pair_here(cx.app).await?));
            }
            account::me(&cx.app.api).await?;
            let key = auth::load(&cx.app.paths)?
                .ok_or_else(|| anyhow::anyhow!("Sign in with palepale login first"))?;
            let config = cx
                .app
                .api
                .call(
                    "opencode_config",
                    Call::default(),
                    limits::get().opencode.setup_timeout_secs.secs(),
                )
                .await?;
            let plugin = cx
                .app
                .api
                .call(
                    "opencode_plugin",
                    Call::default(),
                    limits::get().opencode.setup_timeout_secs.secs(),
                )
                .await?;
            io::opencode::install(
                &root,
                &config.body,
                &plugin.body,
                &key.secret,
                &endpoints::origin("api"),
            )?;
            Ok(TEXT.text.ready.clone().into())
        })
    }
}

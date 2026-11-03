use super::{Command, Ctx, Outcome, Spec};
use crate::{
    api::{Call, account, endpoints},
    auth, io, settings,
    text::template::fill,
};
use futures_util::future::BoxFuture;
use std::{path::PathBuf, sync::LazyLock};

pub struct Connect {
    source: &'static str,
    spec: &'static LazyLock<Spec>,
    kind: io::clients::Kind,
}
const CODEX_SOURCE: &str = include_str!("../../data/commands/codex.toml");
const CLAUDE_SOURCE: &str = include_str!("../../data/commands/claude.toml");
static CODEX_SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(CODEX_SOURCE));
static CLAUDE_SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(CLAUDE_SOURCE));
pub static CODEX: Connect = Connect {
    source: CODEX_SOURCE,
    spec: &CODEX_SPEC,
    kind: io::clients::Kind::Codex,
};
pub static CLAUDE: Connect = Connect {
    source: CLAUDE_SOURCE,
    spec: &CLAUDE_SPEC,
    kind: io::clients::Kind::Claude,
};

impl Command for Connect {
    fn spec(&self) -> &Spec {
        self.spec
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
                self.spec.usage
            );
            let (variable, folder) = match self.kind {
                io::clients::Kind::Codex => ("CODEX_HOME", ".codex"),
                io::clients::Kind::Claude => ("CLAUDE_CONFIG_DIR", ".claude"),
            };
            let root = if let Some(value) = settings::env(variable) {
                PathBuf::from(value)
            } else {
                io::fs::home_dir()
                    .ok_or_else(|| anyhow::anyhow!("No user home directory"))?
                    .join(folder)
            };
            anyhow::ensure!(root.is_absolute(), "{variable} must be an absolute path");
            let text: toml::Value = toml::from_str(self.source)?;
            if args.first().is_some_and(|arg| arg == "--logout") {
                io::clients::disconnect(self.kind, &root)?;
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
            let config = cx
                .app
                .api
                .call(
                    "client_config",
                    Call::default(),
                    std::time::Duration::from_secs(io::clients::config().setup_timeout_secs),
                )
                .await?;
            let model = io::clients::install(
                self.kind,
                &root,
                &config.body,
                &key.secret,
                &endpoints::origin("api"),
                (args.first().map(String::as_str), &cx.app.settings.model),
            )?;
            Ok(fill(
                text["text"]["ready"].as_str().expect("client setup text"),
                &[("model", &model)],
            )
            .into())
        })
    }
}

pub struct ClientKey;
static KEY_SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/client-key.toml")));

pub fn credential_helper(words: &[String]) -> Option<anyhow::Result<String>> {
    (words.first().is_some_and(|word| word == &KEY_SPEC.name)).then(|| read_key(&words[1..]))
}

fn read_key(args: &[String]) -> anyhow::Result<String> {
    let [path] = args else {
        anyhow::bail!("Usage: palepale {}", KEY_SPEC.usage);
    };
    io::clients::credential(&PathBuf::from(path))
}
impl Command for ClientKey {
    fn spec(&self) -> &Spec {
        &KEY_SPEC
    }
    fn run<'a>(
        &'a self,
        _: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move { Ok(read_key(args)?.into()) })
    }
}

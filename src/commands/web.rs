use futures_util::future::BoxFuture;
use std::sync::LazyLock;
use super::{Command, Ctx, Outcome, Scope, Spec};
use crate::{api::account, auth, io::proc::open_url, settings, text::template::fill};
use serde::Deserialize;

const SOURCE: &str = include_str!("../../data/commands/web.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SOURCE));
#[derive(Deserialize)] struct Text { opened: String }
static TEXT: LazyLock<Text> = LazyLock::new(|| { #[derive(Deserialize)] struct File { text: Text } toml::from_str::<File>(SOURCE).expect("web text").text });
pub struct Web;
impl Command for Web {
    fn spec(&self) -> &Spec { &SPEC }
    fn run<'a>(&'a self, cx: &'a Ctx<'a>, args: &'a [String]) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            cx.no_args(&SPEC,args)?;
            anyhow::ensure!(cx.scope == Scope::Cli, "palepale web only runs from the CLI");
            let code = account::browser_handoff(&auth::owner_api(cx.app)?).await?;
            let url = format!("https://palepale.tech/login#handoff={code}");
            if settings::env("PALEPALE_NO_BROWSER").is_none() { let _ = open_url(&url); }
            Ok(fill(&TEXT.opened, &[]).into())
        })
    }
}

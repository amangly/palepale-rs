use super::{Command, Ctx, Outcome, Spec};
use crate::{
    api::{account, signup},
    auth::{self, KeyKind},
    text::template::fill,
};
use anyhow::{Context, bail};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::sync::LazyLock;
const SOURCE: &str = include_str!("../../data/commands/signup.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SOURCE));
#[derive(Deserialize)]
struct Text {
    existing: String,
    owner_exists: String,
    recovery: String,
    done: String,
    label: String,
}
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SOURCE).expect("signup text").text
});
pub struct Signup;
impl Command for Signup {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let app = cx.app;
            let _lock = auth::owner_lock(&app.paths)?;
            let force = match args {
                [] => false,
                [flag] if flag == "--force" => true,
                _ => return Err(cx.usage(&SPEC)),
            };
            if auth::owner_stored(&app.paths)? {
                bail!(TEXT.owner_exists.clone());
            }
            if !force && auth::stored(&app.paths)?.is_some() {
                bail!(TEXT.existing.clone());
            }
            let account = signup::register(&app.api).await?;
            // Persist the only recovery credential before requesting a client key.
            auth::save_owner(&app.paths, &account.owner_credential)?;
            let api = app.api.with_key(account.owner_credential);
            let key = account::create_key(&api, &TEXT.label)
                .await
                .context(TEXT.recovery.clone())?;
            auth::save(&app.paths, &key, KeyKind::Device).context(TEXT.recovery.clone())?;
            app.api.set_key(Some(key));
            Ok(Outcome::from(format!(
                "{}{}",
                fill(
                    &TEXT.done,
                    &[("path", &auth::owner_path(&app.paths).display().to_string())]
                ),
                super::login::env_note()
            )))
        })
    }
}

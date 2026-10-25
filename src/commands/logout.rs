use std::sync::LazyLock;

use futures_util::future::BoxFuture;
use serde::Deserialize;

use super::{Command, Ctx, Outcome, Spec};
use crate::auth;

const SOURCE: &str = include_str!("../../data/commands/logout.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SOURCE));
#[derive(Deserialize)]
struct Text {
    removed: String,
    absent: String,
    owner_retained: String,
}
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SOURCE).expect("logout text").text
});

pub struct Logout;

impl Command for Logout {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            cx.no_args(&SPEC, args)?;
            let removed = auth::remove(&cx.app.paths)?;
            // A running session stops using the key at once, unless the environment supplies one.
            let left = auth::load(&cx.app.paths)?.map(|key| key.secret);
            cx.app.api.set_key(left);
            let mut out = if removed {
                TEXT.removed.clone()
            } else {
                TEXT.absent.clone()
            };
            if auth::owner_stored(&cx.app.paths)? {
                out.push_str(&TEXT.owner_retained);
            }
            out.push_str(super::login::env_note());
            Ok(out.into())
        })
    }
}

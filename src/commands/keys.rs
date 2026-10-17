use super::{Command, Ctx, Outcome, Spec};
use crate::{api::account, text::template::fill};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::sync::LazyLock;
const SOURCE: &str = include_str!("../../data/commands/keys.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SOURCE));
#[derive(Deserialize)]
struct Text {
    empty: String,
    row: String,
    current: String,
    revoked: String,
    active: String,
    limit: String,
    confirm: String,
    done: String,
    bad_ref: String,
}
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SOURCE).expect("keys text").text
});
pub struct Keys;
impl Command for Keys {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            if args.is_empty() || args == ["list"] {
                let keys = account::keys(&cx.app.api).await?;
                let mut rows: Vec<_> = keys
                    .iter()
                    .map(|key| {
                        fill(
                            &TEXT.row,
                            &[
                                ("ref", &key.reference),
                                ("name", &key.name),
                                ("hint", &key.hint),
                                ("created", &key.created_at),
                                ("expires", &key.expires_at),
                                (
                                    "state",
                                    if key.current {
                                        &TEXT.current
                                    } else if key.revoked_at.is_some() {
                                        &TEXT.revoked
                                    } else {
                                        &TEXT.active
                                    },
                                ),
                            ],
                        )
                    })
                    .collect();
                if rows.is_empty() {
                    rows.push(TEXT.empty.clone());
                }
                rows.push(TEXT.limit.clone());
                return Ok(rows.join("\n").into());
            }
            if args.first().is_some_and(|arg| arg == "revoke") {
                if args.len() != 3 || args[2] != "--yes" {
                    anyhow::bail!(TEXT.confirm.clone());
                }
                let reference = &args[1];
                if reference.len() != 36
                    || !reference.bytes().enumerate().all(|(i, b)| {
                        if [8, 13, 18, 23].contains(&i) {
                            b == b'-'
                        } else {
                            b.is_ascii_hexdigit()
                        }
                    })
                {
                    anyhow::bail!(TEXT.bad_ref.clone());
                }
                account::revoke_key(&cx.app.api, reference).await?;
                return Ok(fill(&TEXT.done, &[("ref", reference)]).into());
            }
            Err(cx.usage(&SPEC))
        })
    }
}

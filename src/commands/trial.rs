use super::{Command, Ctx, Outcome, Scope, Spec};
use crate::{
    api::{Call, account},
    auth,
    io::proc::open_url,
    settings,
    text::template::fill,
};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::sync::LazyLock;
const SOURCE: &str = include_str!("../../data/commands/trial.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SOURCE));
#[derive(Deserialize)]
struct Text {
    model: String,
    opened: String,
    status: String,
}
static TEXT: LazyLock<Text> = LazyLock::new(|| {
    #[derive(Deserialize)]
    struct File {
        text: Text,
    }
    toml::from_str::<File>(SOURCE).expect("trial text").text
});
pub fn model() -> &'static str {
    &TEXT.model
}
pub struct Trial;
impl Command for Trial {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            anyhow::ensure!(cx.scope == Scope::Cli, "trial requires the CLI");
            if args == ["request"] || args == ["redeem"] {
                let action = if args == ["request"] {
                    "trial_request"
                } else {
                    "trial_redeem"
                };
                let body = serde_json::json!({"action":action});
                let response = cx
                    .app
                    .api
                    .call(
                        "trial_flow",
                        Call {
                            body: Some(&body),
                            ..Call::default()
                        },
                        crate::limits::get().http.request_timeout_ms.ms(),
                    )
                    .await?;
                #[derive(Deserialize)]
                struct Flow {
                    flow: String,
                }
                let flow: Flow = serde_json::from_slice(&response.body)?;
                anyhow::ensure!(
                    flow.flow.len() == 43
                        && flow
                            .flow
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
                    "Invalid trial flow"
                );
                let code = account::browser_handoff(&auth::owner_api(cx.app)?).await?;
                let url = format!(
                    "https://palepale.tech/login#handoff={code}&trial={}&action={action}",
                    flow.flow
                );
                if settings::env("PALEPALE_NO_BROWSER").is_none() {
                    let _ = open_url(&url);
                }
                return Ok(TEXT.opened.clone().into());
            }
            cx.no_args(&SPEC, args)?;
            let response = cx
                .app
                .api
                .call(
                    "trial",
                    Call::default(),
                    crate::limits::get().http.request_timeout_ms.ms(),
                )
                .await?;
            #[derive(Deserialize)]
            struct Wallet {
                available: String,
            }
            #[derive(Deserialize)]
            struct Application {
                state: String,
            }
            #[derive(Deserialize)]
            struct Status {
                model: String,
                available: bool,
                wallet: Wallet,
                application: Option<Application>,
            }
            let status: Status = serde_json::from_slice(&response.body)?;
            Ok(fill(
                &TEXT.status,
                &[
                    (
                        "state",
                        status
                            .application
                            .as_ref()
                            .map_or("not requested", |a| a.state.as_str()),
                    ),
                    ("balance", &status.wallet.available),
                    ("model", &status.model),
                    ("available", if status.available { "yes" } else { "no" }),
                ],
            )
            .into())
        })
    }
}

use super::{Command, Ctx, Outcome, Scope, Spec};
use crate::{
    api::{Api, Call, account},
    auth,
    io::{clock, proc::open_url, term},
    settings,
    text::template::fill,
};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use std::{sync::LazyLock, time::Duration};
const SOURCE: &str = include_str!("../../data/commands/trial.toml");
static SPEC: LazyLock<Spec> = LazyLock::new(|| Spec::parse(SOURCE));
#[derive(Deserialize)]
struct Text {
    model: String,
    opened: String,
    status: String,
    code_prompt: String,
    invalid_code: String,
    cancelled: String,
    waiting: String,
    redeemed: String,
    expired: String,
}
#[derive(Deserialize)]
struct Workflow {
    poll_ms: u64,
    timeout_secs: u64,
}
#[derive(Deserialize)]
struct Config {
    text: Text,
    workflow: Workflow,
}
static CONFIG: LazyLock<Config> = LazyLock::new(|| toml::from_str(SOURCE).expect("trial settings"));
pub fn model() -> &'static str {
    &CONFIG.text.model
}

async fn start(cx: &Ctx<'_>, action: &str) -> anyhow::Result<String> {
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
    let handoff = match auth::owner_api(cx.app) {
        Ok(owner) => account::browser_handoff(&owner).await.ok(),
        Err(_) => None,
    };
    let prefix = handoff
        .map(|code| format!("handoff={code}&"))
        .unwrap_or_default();
    let url = format!(
        "{}/login#{prefix}trial={}&action={action}",
        crate::api::endpoints::origin("chat"),
        flow.flow
    );
    if settings::env("PALEPALE_NO_BROWSER").is_none() {
        let _ = open_url(&url);
    }
    Ok(flow.flow)
}
fn code() -> anyhow::Result<String> {
    let code = if term::stdin_is_terminal() {
        term::read_secret(&CONFIG.text.code_prompt)?
    } else {
        term::read_line()?
    };
    let code = code.ok_or_else(|| anyhow::anyhow!(CONFIG.text.cancelled.clone()))?;
    let code = code.trim().to_owned();
    anyhow::ensure!(
        code.len() == 46
            && code.starts_with("pt_")
            && code[3..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        CONFIG.text.invalid_code.clone()
    );
    Ok(code)
}
async fn redeem(api: &Api, flow: &str, code: &str) -> anyhow::Result<String> {
    let body = serde_json::json!({"flow":flow,"code":code});
    let wait = async {
        loop {
            let response = api
                .call(
                    "trial_redeem",
                    Call {
                        body: Some(&body),
                        ..Call::default()
                    },
                    crate::limits::get().http.request_timeout_ms.ms(),
                )
                .await?;
            #[derive(Deserialize)]
            struct Redemption {
                #[serde(default)]
                pending: bool,
                #[serde(default)]
                ok: bool,
                amount: Option<String>,
            }
            let result: Redemption = serde_json::from_slice(&response.body)?;
            if result.ok && result.amount.as_deref() == Some("2000000") {
                return Ok(CONFIG.text.redeemed.clone());
            }
            anyhow::ensure!(result.pending, "Invalid trial redemption response");
            clock::sleep(Duration::from_millis(CONFIG.workflow.poll_ms)).await;
        }
    };
    let bounded = clock::timeout(Duration::from_secs(CONFIG.workflow.timeout_secs), wait);
    tokio::select! {
     result=bounded=>result.map_err(|_|anyhow::anyhow!(CONFIG.text.expired.clone()))?,
     ()=term::interrupted()=>anyhow::bail!(CONFIG.text.cancelled.clone()),
    }
}
async fn status(api: &Api) -> anyhow::Result<String> {
    let response = api
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
        &CONFIG.text.status,
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
    ))
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
            if args == ["request"] {
                start(cx, "trial_request").await?;
                return Ok(CONFIG.text.opened.clone().into());
            }
            if args == ["redeem"] {
                let code = code()?;
                let flow = start(cx, "trial_redeem").await?;
                term::out(&format!("{}\n", CONFIG.text.waiting));
                return Ok(redeem(&cx.app.api, &flow, &code).await?.into());
            }
            cx.no_args(&SPEC, args)?;
            Ok(status(&cx.app.api).await?.into())
        })
    }
}

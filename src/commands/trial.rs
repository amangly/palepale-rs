use super::{Command, Ctx, Effect, Outcome, Scope, Spec};
use crate::{
    api::{Api, Call},
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
    login: String,
    ticket: String,
    unavailable: String,
    closed: String,
    invalid_status: String,
}
#[derive(Deserialize)]
struct Workflow {
    poll_ms: u64,
    timeout_secs: u64,
    discord_url: String,
    verify_path: String,
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
    let url = format!(
        "{}{}#trial={}&action={action}",
        crate::api::endpoints::origin("chat"),
        CONFIG.workflow.verify_path,
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
#[derive(Deserialize)]
struct Wallet {
    available: String,
}
#[derive(Deserialize)]
struct Application {
    id: Option<String>,
    state: String,
}
#[derive(Deserialize)]
struct Status {
    model: String,
    available: bool,
    wallet: Wallet,
    application: Option<Application>,
}
async fn read_status(api: &Api) -> anyhow::Result<Status> {
    let response = api
        .call(
            "trial",
            Call::default(),
            crate::limits::get().http.request_timeout_ms.ms(),
        )
        .await?;
    let status: Status = serde_json::from_slice(&response.body)?;
    anyhow::ensure!(
        status.model == CONFIG.text.model,
        CONFIG.text.invalid_status.clone()
    );
    Ok(status)
}
async fn status(api: &Api) -> anyhow::Result<String> {
    let status = read_status(api).await?;
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
            (
                "balance",
                &crate::text::units::usd_micros(status.wallet.available.parse::<u64>()?),
            ),
            ("model", &status.model),
            ("available", if status.available { "yes" } else { "no" }),
        ],
    ))
}
async fn wait_for_request(api: &Api) -> anyhow::Result<Status> {
    let wait = async {
        loop {
            let current = read_status(api).await?;
            if current.application.is_some() {
                return Ok(current);
            }
            clock::sleep(Duration::from_millis(CONFIG.workflow.poll_ms)).await;
        }
    };
    tokio::select! {
        result=clock::timeout(Duration::from_secs(CONFIG.workflow.timeout_secs),wait)=>result.map_err(|_|anyhow::anyhow!(CONFIG.text.expired.clone()))?,
        ()=term::interrupted()=>anyhow::bail!(CONFIG.text.cancelled.clone()),
    }
}
async fn workflow(cx: &Ctx<'_>) -> anyhow::Result<Outcome> {
    if !cx.app.api.has_key() {
        term::out(&format!("{}\n", CONFIG.text.login));
        super::login::Login.run(cx, &[]).await?;
    }
    let mut current = match read_status(&cx.app.api).await {
        Err(error)
            if error
                .downcast_ref::<crate::api::error::ApiError>()
                .is_some_and(|e| e.info.status == Some(401)) =>
        {
            term::out(&format!("{}\n", CONFIG.text.login));
            super::login::Login.run(cx, &[]).await?;
            read_status(&cx.app.api).await?
        }
        result => result?,
    };
    if current.application.is_none() {
        start(cx, "trial_request").await?;
        term::out(&format!("{}\n", CONFIG.text.opened));
        current = wait_for_request(&cx.app.api).await?;
    }
    let application = current.application.as_ref().expect("request exists");
    anyhow::ensure!(
        !["rejected", "revoked"].contains(&application.state.as_str()),
        CONFIG.text.closed.clone()
    );
    if application.state != "redeemed" {
        if application.state == "pending" {
            let reference = application
                .id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!(CONFIG.text.invalid_status.clone()))?;
            term::out(&format!(
                "{}\n",
                fill(
                    &CONFIG.text.ticket,
                    &[
                        ("reference", reference),
                        ("url", &CONFIG.workflow.discord_url)
                    ]
                )
            ));
            super::login::open_browser(&CONFIG.workflow.discord_url);
        }
        let code = code()?;
        let flow = start(cx, "trial_redeem").await?;
        term::out(&format!("{}\n", CONFIG.text.waiting));
        redeem(&cx.app.api, &flow, &code).await?;
        current = read_status(&cx.app.api).await?;
    }
    anyhow::ensure!(
        current
            .application
            .as_ref()
            .is_some_and(|app| app.state == "redeemed"),
        CONFIG.text.invalid_status.clone()
    );
    anyhow::ensure!(
        current.available && current.wallet.available.parse::<u64>()? > 0,
        CONFIG.text.unavailable.clone()
    );
    Ok(Outcome {
        text: CONFIG.text.redeemed.clone(),
        effects: vec![Effect::StartTrial],
    })
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
            if args == ["status"] {
                return Ok(status(&cx.app.api).await?.into());
            }
            cx.no_args(&SPEC, args)?;
            workflow(cx).await
        })
    }
}

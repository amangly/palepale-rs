use super::{Command, Ctx, Outcome, Spec};
use crate::{
    discord::{Config, bot::Bot, data},
    io, settings,
};
use anyhow::Context;
use std::sync::{Arc, LazyLock};
use tokio_util::sync::CancellationToken;

static SPEC: LazyLock<Spec> =
    LazyLock::new(|| Spec::parse(include_str!("../../data/commands/discord.toml")));
pub struct Discord;
impl Command for Discord {
    fn spec(&self) -> &Spec {
        &SPEC
    }
    fn run<'a>(
        &'a self,
        cx: &'a Ctx<'a>,
        args: &'a [String],
    ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Outcome>> {
        Box::pin(async move {
            let (action, rest) = args
                .split_first()
                .map_or(("status", &[][..]), |(a, r)| (a.as_str(), r));
            if action == "setup" {
                return setup(cx, rest).await;
            }
            cx.no_args(&SPEC, rest)?;
            match action {
                "run" => run(cx).await,
                "status" => {
                    let Some(config) = load(cx)? else {
                        return Ok("Discord is not configured. Run palepale discord setup.".into());
                    };
                    let running = io::fs::lock(&lock_path(cx)).is_err();
                    Ok(format!(
                        "Discord: {} · owner: {} · model: {}\nConfig: {}",
                        if running {
                            "running or locked"
                        } else {
                            "stopped"
                        },
                        config.owner,
                        if config.model.is_empty() {
                            "CLI default"
                        } else {
                            &config.model
                        },
                        cx.app.paths.discord_config().display()
                    )
                    .into())
                }
                "logout" => {
                    let _lock = lock(cx)?;
                    io::fs::remove(&cx.app.paths.discord_token())?;
                    io::fs::remove(&cx.app.paths.discord_config())?;
                    Ok("Discord disconnected locally. Your Palepale login stays active. Clear DISCORD_BOT_TOKEN if set. The Discord bot and command remain installed in your server.".into())
                }
                "help" | "--help" => Ok(include_str!("../../data/discord-help.md").into()),
                _ => Err(cx.usage(&SPEC)),
            }
        })
    }
}
fn lock_path(cx: &Ctx<'_>) -> std::path::PathBuf {
    cx.app.paths.home.join("discord.lock")
}
fn lock(cx: &Ctx<'_>) -> anyhow::Result<io::fs::FileLock> {
    io::fs::lock(&lock_path(cx)).context("Stop palepale discord run before changing its setup")
}
fn load(cx: &Ctx<'_>) -> anyhow::Result<Option<Config>> {
    io::fs::read_limited(&cx.app.paths.discord_config(), 65536)?
        .map(|text| {
            toml::from_str(&text).map_err(|_| {
                anyhow::anyhow!(
                    "Invalid discord.toml. Check its fields or run discord logout to remove it"
                )
            })
        })
        .transpose()
}

async fn setup(cx: &Ctx<'_>, args: &[String]) -> anyhow::Result<Outcome> {
    anyhow::ensure!(
        settings::env("PALEPALE_OFFLINE").is_none(),
        "Discord setup needs network access"
    );
    let _lock = lock(cx)?;
    let mut config = load(cx)?.unwrap_or_else(|| data().defaults.clone());
    let mut args = args.iter();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--owner" => {
                config.owner = args
                    .next()
                    .context("--owner needs a Discord user ID")?
                    .clone()
            }
            "--allow-user" => config.allow_users.push(
                args.next()
                    .context("--allow-user needs a Discord user ID")?
                    .clone(),
            ),
            "--allow-role" => config.allow_roles.push(
                args.next()
                    .context("--allow-role needs a Discord role ID")?
                    .clone(),
            ),
            "--free-channel" => config.free_response_channels.push(
                args.next()
                    .context("--free-channel needs a Discord channel ID")?
                    .clone(),
            ),
            "--thread" => config.auto_thread = true,
            _ => return Err(cx.usage(&SPEC)),
        }
    }
    if config.owner.is_empty() && io::term::stdin_is_terminal() {
        io::term::out("Discord owner user ID: ");
        config.owner = io::term::read_line()?.unwrap_or_default().trim().to_owned();
    }
    if !cx.app.settings.model.is_empty() {
        config.model.clone_from(&cx.app.settings.model);
    }
    config.validate()?;
    let token = if let Some(token) = settings::env("DISCORD_BOT_TOKEN") {
        token
    } else if io::term::stdin_is_terminal() {
        io::term::read_secret("Discord bot token: ")?.context("Setup cancelled")?
    } else {
        anyhow::bail!("Set DISCORD_BOT_TOKEN or run setup in a terminal");
    };
    let token = token.trim().to_owned();
    let discord = io::discord::Discord::new(token.clone())?;
    let application = discord.identity().await?;
    discord.register(&application).await?;
    io::discord::save_token(&cx.app.paths.discord_token(), &token)?;
    io::fs::write_atomic(
        &cx.app.paths.discord_config(),
        toml::to_string_pretty(&config)?.as_bytes(),
        io::fs::Access::Private,
    )?;
    Ok(format!("Discord configured. Enable Message Content Intent and invite the bot with bot and applications.commands scopes.\nRun palepale discord run.\nInvite: https://discord.com/oauth2/authorize?client_id={application}&scope=bot%20applications.commands&permissions=309237713920").into())
}

async fn run(cx: &Ctx<'_>) -> anyhow::Result<Outcome> {
    let _lock = lock(cx)?;
    anyhow::ensure!(
        cx.app.api.is_configured() && settings::env("PALEPALE_OFFLINE").is_none(),
        "Discord run needs network access"
    );
    anyhow::ensure!(cx.app.api.has_key(), "Sign in with palepale login first");
    anyhow::ensure!(
        !cx.app.api.uses_trial(),
        "Discord uses paid account access. Run without --trial"
    );
    let mut config = load(cx)?.context("Run palepale discord setup first")?;
    config.validate()?;
    let catalog = crate::api::models::load(cx.app, true).await;
    let model = catalog.selected(if config.model.is_empty() {
        &cx.app.settings.model
    } else {
        &config.model
    })?;
    anyhow::ensure!(
        model.capabilities.streaming,
        "Choose a streaming text model with palepale --model ID discord setup"
    );
    config.model = model.id.clone();
    let discord =
        io::discord::Discord::new(io::discord::read_token(&cx.app.paths.discord_token())?)?;
    let bot_id = discord.identity().await?;
    let cancel = CancellationToken::new();
    let (events, receiver) = tokio::sync::mpsc::channel(data().event_queue);
    let bot = Arc::new(Bot::new(
        cx.app.api.clone(),
        discord.clone(),
        config,
        catalog,
        bot_id,
        cancel.clone(),
    ));
    io::term::out("Discord bot starting. Press Ctrl-C to stop.\n");
    let serving = bot.serve(receiver);
    let gateway = discord.gateway(events, cancel.clone());
    tokio::pin!(serving, gateway);
    let (result, serving_done) = tokio::select! {
        result = &mut gateway => (result, false),
        () = &mut serving => (Ok(()), true),
        () = io::term::interrupted() => (Ok(()), false),
    };
    cancel.cancel();
    // The server aborts its bounded tasks and drops every paid response stream on shutdown.
    if !serving_done {
        serving.await;
    }
    result?;
    Ok("Discord bot stopped.".into())
}

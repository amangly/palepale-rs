# Connect coding clients

Palepale can configure Codex and Claude Code to use your Palepale account in API
mode. Install the client first, then run:

```sh
palepale codex
palepale claude
```

These commands work from PowerShell, Bash and Zsh. They sign in to Palepale when
needed, check account access and fetch the gateway's supported models. Setup
does not send an inference request. The gateway must expose `/docs/clients.json`
and the corresponding protocol before setup can succeed.

The commands use your current Palepale model when available, otherwise the first
compatible model. To choose one, run `palepale codex <model-id>` or
`palepale claude <model-id>`. Restart the target app afterward. Requests use your
Palepale account's paid API access and spending limits. Trial credit stays in
the Palepale CLI. The other app's cost display can be an estimate; Palepale's
account usage records determine the charge.

## Configuration

Codex uses `CODEX_HOME`, or `~/.codex`. Setup merges `config.toml`, selects the
Palepale provider and saves `palepale-models.json`. Other providers, project trust,
approval settings and saved subscription credentials remain. An active default
profile and model-specific overrides are cleared so they cannot keep the old
provider or context limits active. Named profiles remain available.

Thinking appears automatically when the model returns it. Rerun `palepale codex`
after updating to replace older settings that hid reasoning summaries.

Claude Code uses `CLAUDE_CONFIG_DIR`, or `~/.claude`. Setup merges `settings.json`
and lists every compatible Palepale model in `/model`, without duplicate family
entries. This requires Claude Code 2.1.242 or later. Background family aliases use
the setup model; subagents inherit the active model. Setup clears competing
provider credentials in the settings environment, uses context/output limits
that fit every listed model, and disables extended-thinking and deferred-tool
features that the adapter does not support. Existing permissions, hooks and
unrelated settings remain. Managed or project settings can take precedence;
check `/status` if the client still selects another provider.

Both integrations store an inference credential in `palepale.key` beside the
client's config. Codex's credential command and Claude Code's `apiKeyHelper` read
it through the installed Palepale executable. Keys are absent from the main
settings, model catalog and setup output. Keep Palepale installed at that path;
reconnect after moving it or replacing a revoked key. The credential helper
works without loading project configuration or contacting the API.

Every changed existing file gets a private sibling `*.palepale-*.bak` backup.
Reconnecting with unchanged settings makes no new backups. Rerun setup to refresh
the model list.

## Cline CLI and VS Code

Install the latest Cline CLI or VS Code extension, then close both clients and run:

```sh
palepale cline
```

Restart Cline. In VS Code, select **Palepale** from the provider list. For the CLI,
run `cline` in your project folder. To select a model during setup, run
`palepale cline <model-id>`. Rerun setup and restart to refresh the model list.

Both clients share `~/.cline/data/settings/providers.json` and `models.json`.
`CLINE_DIR` changes the Cline root; `CLINE_DATA_DIR` overrides its data directory.
Use the same directory for setup and Cline. Palepale merges only its own provider
and models. Other providers remain. The inference key stays in Cline's private
provider settings; disconnect removes it. Reconnecting does not create key backups.

Streaming and local tools use Cline's normal workflow. Image-capable models can
receive attachments in VS Code or `cline "Describe @./screenshot.png"` in the CLI.
Models use their default reasoning effort. Cline's price display is an estimate;
Palepale's usage records determine the charge.

Local verification uses Cline CLI 3.0.74 and VS Code extension 4.1.26 with a fake
gateway, including an image, a local file tool, its result and the final reply.
The extension test selects Palepale through its saved model-picker state.
No supplier request is sent.
[Cline configuration](https://docs.cline.bot/getting-started/config).

## Antigravity CLI and IDE

Install Antigravity, then run:

```sh
palepale antigravity
```

The command adds the gateway and compatible model list to Antigravity's user
settings, and selects the current Palepale model when available.
It configures the OpenAI-compatible gateway at `https://api.palepale.tech` and
keeps the inference key in the private gateway settings file. Existing settings
remain in place. Use `palepale antigravity <model-id>` to choose a model.
Open a Palepale IDE window with `palepale antigravity --ide`. It uses the profile
at `~/.antigravity/palepale-ide` so another open IDE window cannot reuse its model
session. Complete the IDE's first-run setup if prompted. A custom install needs
`antigravity-ide` on PATH. Windows also checks the standard installation folders.
Use `palepale antigravity --run` for the standalone CLI. Pass extra arguments after
`--`, for example `palepale antigravity --run -- --print "Hello"`.
Both launchers pass gateway settings to the app process. Your shell profile stays private. Rerun setup to refresh the model list.

Connect the desktop app on your computer. For Antigravity CLI on a VPS, run
setup as the same SSH user. `ANTIGRAVITY_IDE_SERVER_ROOT` can point to the
absolute `.antigravity-ide-server` path when the remote home cannot be inferred.
That directory contains runtime files; setup writes `.antigravity/settings.json`
and `.gemini/antigravity-cli/settings.json` in its parent directory.

This requires an Antigravity build with OpenAI gateway mode. Administrator
settings and `AGY_LLM_GATEWAY_*` variables override user settings. JetBrains and
Zed's ACP server requires the Gemini wire protocol; setup leaves ACP unchanged.
See [Antigravity gateway settings](https://docs.cloud.google.com/gemini/enterprise/docs/ai-developer-tools/deploy-enterprise-gateway).

Verified locally with Antigravity CLI 1.5.1: model listing, streaming, a local
file read and tool-result replay through a fake gateway using Palepale request
validation. Antigravity IDE 2.6.0 was also checked with the ordinary IDE open:
the separate profile listed two fixture models, selected a different model and
rendered its authenticated gateway reply. These checks use a local fake gateway.
The model picker in a Remote SSH window still needs runtime verification.

Disconnect with:

```sh
palepale antigravity --logout
```

## Discord bot

Run a Discord chat bot with your Palepale account. Keep the CLI running on your
computer or VPS; the host account pays for every reply.

```sh
palepale discord setup --owner YOUR_DISCORD_USER_ID
palepale discord run
```

Create a bot in the [Discord Developer Portal](https://discord.com/developers/applications).
Enable **Message Content Intent**. Leave **Interactions Endpoint URL** empty;
commands arrive through the Gateway. Setup asks for the token without echoing
it, registers `/palepale`, and prints an invite link. It also accepts
`DISCORD_BOT_TOKEN`. Never put the token in a command argument.

Only the owner has access at first. Add `--allow-user ID` or `--allow-role ID`
during setup to grant access. Role access applies inside servers, not DMs.
Approved users can DM the bot or mention it in a channel. `--free-channel ID`
removes the mention requirement there; it does not grant access. `--thread`
starts a thread for channel mentions. Follow-up messages in that thread need no mention.

| Discord command | Action |
| --- | --- |
| `/palepale ask` | Send a message |
| `/palepale model` | List models or select one |
| `/palepale reset` | Clear your conversation |
| `/palepale stop` | Cancel your active reply |
| `/palepale usage` | Show verified session tokens |
| `/palepale help` | Show commands |

Conversations are separate for each user and channel or thread. History stays
in memory until reset or exit; the oldest idle histories are removed when the
session limit is reached. Changing models starts a fresh conversation.
Replies update as text arrives and split at Discord's message limit. Attachments,
local tools and moderation actions are not supported in this version.

Choose the default model with `palepale --model MODEL_ID discord setup`.
Use `palepale --budget 2 discord run` to apply a $2 run budget through the gateway.
Stop does not undo usage already processed; check Palepale Usage for charges.
Trials are not supported by the bot.

`palepale discord status` shows local configuration and whether another process
holds the bot lock. `palepale discord logout` removes local setup without changing
your Palepale login or removing the Discord bot. Clear `DISCORD_BOT_TOKEN` too if set.
Configuration lives in `~/.palepale/discord.toml` (or `PALEPALE_HOME`): user/role
access, mention-free channels, threading, concurrent replies and history limits.
Restart the bot after editing it. Only one bot process can use the same home.

## Disconnect

Close the client, then run its command:

```sh
palepale opencode --logout
palepale codex --logout
palepale claude --logout
palepale cline --logout
palepale antigravity --logout
```

Disconnect works offline. It removes the app's Palepale key and integration.
Codex and Claude Code restore settings saved by this setup version, preserving
later edits. Older connections are cleared; their original settings remain in
the sibling backups. Other providers, saved subscription logins and your Palepale
login stay available. Restart the client afterward. Already-running clients may
retain a cached key until closed; disconnect does not revoke the shared key.

## Supported behavior

The gateway adapts Responses for Codex and Messages for Claude Code to Palepale's
existing completion service. Text streams incrementally. User images, function
tools, matching tool results, cancellation and usage accounting use the same
admission and billing rules as the native CLI. Codex function namespaces retain
their identities across turns. Desktop code mode supports free-form source and
its matching tool results. Code executes in the client. Tool results currently
support text only.

The Responses path is stateless: clients replay history. Stored responses,
`previous_response_id`, hosted tools, strict function-schema guarantees and
verbosity controls are unsupported. Setup disables Codex web search and
WebSockets. Prompt-cache affinity hints are accepted without guaranteeing a cache.

The Messages path supports custom tools and token counting. It does not implement
signed thinking, deferred tool discovery or Anthropic cache-placement guarantees.
Setup lets the model use its default effort. Unsupported requests fail explicitly
before inference. Anthropic does not officially support non-Claude models in
Claude Code; Palepale's adapter compatibility is tested separately.

Local fixture verification used Codex CLI 0.166.0 and Claude Code 2.1.317 on
Windows, including a tool call, its result and the final reply. These checks use
fake models and do not establish live provider quality or production deployment.
Rerun setup to refresh Codex's model catalog after model availability changes.

References: [Codex gateway setup](https://learn.chatgpt.com/docs/enterprise/connect-to-a-gateway),
[Codex protocol](https://learn.chatgpt.com/docs/enterprise/gateway-compatibility),
[Claude Code gateway setup](https://code.claude.com/docs/en/llm-gateway-connect),
[Claude Code protocol](https://code.claude.com/docs/en/llm-gateway-protocol).

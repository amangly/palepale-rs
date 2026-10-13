# Palepale-rs

Use your [palepale.tech](https://palepale.tech) account to run a coding agent in your terminal. Read code, edit files, run commands, and switch models in your project.

## Get started

Download the CLI for Windows x64, macOS Apple Silicon, or Linux x64 from the [Palepale download page](https://palepale.tech/docs/cli) or [GitHub Releases](https://github.com/amangly/palepale-rs/releases/latest). Rename it to `palepale.exe` on Windows or `palepale` on macOS/Linux.

Run it from your project folder:

```powershell
.\palepale.exe
```

On macOS/Linux, run `chmod +x palepale` once, then `./palepale`.

The first launch opens palepale.tech in your browser. Sign in, match the code shown in your terminal, and choose **Connect CLI**. Later launches use your saved login.

Run `palepale install` to add the CLI to your user PATH. Before installation, use `./palepale install` or `.\palepale.exe install`.

After installing, open a new terminal in your project folder and run `palepale`. Describe what you want to build or fix, then use `/models` to choose a model.

## Use

| Control | Action |
| --- | --- |
| Enter | Send a message |
| Shift+Enter or Ctrl+J | Insert a new line |
| Shift+Tab | Switch MANUAL, PLAN, and AUTO modes |
| `/models` | Refresh the list and pick a model |
| `/effort` | Choose reasoning effort for the current model |
| `/status` | Open session details, balance, and account usage |
| `/mcp` | Add and manage MCP servers and inspect their tools |
| `/skills` | Search, preview, and select reusable workflows |
| `/help` | List commands |
| Escape | Interrupt the current turn |
| Ctrl+D | Quit with an empty composer |

MANUAL follows permission rules. PLAN restricts writes while preparing a plan. AUTO accepts actions automatically, including actions denied by permission rules.

Opening `/models` fetches the latest catalog. If the server is unavailable, it shows the cached list.

The opening screen reveals the Palepale logo with a brief Matrix effect from [ttfx](https://github.com/omacom/ttfx). The SVG becomes colored block pixels, keeping its shape and green gradient in Windows Terminal and other terminals. Typing skips the animation and keeps your input. `PALEPALE_NO_ANIMATION=1` skips it too. `NO_COLOR` shows the same shape without color.

`/effort` shows the levels reported by the model catalog. Choose `default` to reset it, or set a level directly with `/effort low`.

Use `/logout` then `/login` to connect another account. Changing the browser account does not switch an already paired CLI.

## MCP and skills

`/mcp` opens a compact server list. Press `a` to add an HTTPS URL or local program, enter its name and connection details, then connect. Local arguments use a JSON array, so paths with spaces stay intact. Tab switches user/project scope in the form. In server details, `e` enables or disables, `r` reconnects, `l` starts OAuth sign-in, `o` signs out, and `d` removes the definition. The sign-in card shows a clickable browser link; press `c` to copy it if the browser did not open.

Server definitions use the existing user or project configuration:

```toml
[mcp_servers.docs]
url = "https://developers.openai.com/mcp"

[mcp_servers.local]
command = "/path/to/your/mcp-server"
args = ["--stdio"]
env_vars = ["SERVICE_TOKEN"]
read_only_tools = ["search"]
```

Use `bearer_token_env_var = "SERVICE_TOKEN"` for an HTTP bearer token. OAuth credentials are saved privately, separately from the Palepale account. Project servers require project trust and a review of each changed definition before starting. MCP calls follow MANUAL/AUTO permissions; PLAN blocks tools unless their names are explicitly listed in `read_only_tools`. Server annotations do not grant access. Connections run in the background, and large tool catalogs use local on-demand discovery.

`/skills` searches standard `SKILL.md` folders in the project's `.agents/skills` directories and `~/.agents/skills`. Press `a` to register another folder, including existing Claude Code or Codex skill folders. Enter opens a preview; Enter again inserts `$skill-name ` into your draft. Add the task and send when ready. Use `e` to enable/disable a skill and `r` to rescan.

A skill starts with YAML metadata, followed by its instructions:

```markdown
---
name: review
description: Review a change for bugs and missing checks.
---
Read the diff, inspect affected code, and report concrete issues.
```

Keep it at `.agents/skills/review/SKILL.md`, with the name matching its folder. Enabled skills can be selected automatically from their descriptions; full instructions and references load only when needed. `disable-model-invocation: true` requires explicit selection, and `user-invocable: false` permits automatic use only. Previews never execute scripts, and skill instructions retain the current permissions. Untrusted project skills stay inactive until the project is trusted.

`palepale mcp list` and `palepale skills list` also work outside the TUI. This release supports MCP tools over stdio and Streamable HTTP, with JSON/SSE responses and OAuth. Resources, prompts, images, sampling, and server-driven interactive workflows are not enabled.

## Updates and settings

Run `palepale` to check for updates before opening the session. When a newer release is available, Palepale shows download progress, verifies `SHA256SUMS`, installs it, and opens the updated CLI in the same terminal. Rapid restarts share a check for one minute. An unavailable update service or failed checksum leaves the installed version usable. Press Ctrl+C during the check or download to continue with it.

`/status` opens a compact card with Session and Usage tabs. Shared charts show context, input/output tokens, and account budget use. Cached input and reasoning totals sit below the token bars. Session cost uses listed model rates; Usage shows confirmed account charges and available and reserved balances. Tab switches tabs, up/down scroll, `r` refreshes, and Escape closes the card. Missing usage data is shown as unavailable.

Settings live in `~/.palepale/config.toml`. Set `auto_update = false` to disable automatic updates.

## Development

See [Contributing](CONTRIBUTING.md) for builds, checks, configuration, and the source layout. Maintainers can find publishing instructions in [Releases](docs/releases.md).

## License

Licensed under [Apache-2.0](LICENSE-APACHE). Copyright 2026 amangly. See [NOTICE](NOTICE).

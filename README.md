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

New sessions start in AUTO, which accepts actions automatically, including actions denied by permission rules. Use `/mode`, Shift+Tab, or `--mode` to choose MANUAL or PLAN. MANUAL follows permission rules. PLAN restricts writes while preparing a plan. An explicit mode in your configuration overrides the startup default.

Opening `/models` fetches the latest catalog. If the server is unavailable, it shows the cached list.

The opening screen reveals the Palepale logo with a brief Matrix effect from [ttfx](https://github.com/omacom/ttfx). The SVG becomes colored block pixels, keeping its shape and green gradient in Windows Terminal and other terminals. Typing skips the animation and keeps your input. `PALEPALE_NO_ANIMATION=1` skips it too. `NO_COLOR` shows the same shape without color.

`/effort` shows the levels reported by the model catalog. Choose `default` to reset it, or set a level directly with `/effort low`.

Use `/logout` then `/login` to connect another account. Changing the browser account does not switch an already paired CLI.

## MCP and skills

`/mcp` opens a compact server list. Press `a` to add an HTTPS URL or local program, enter its name and connection details, then connect. Local arguments use a JSON array, so paths with spaces stay intact. Tab switches user/project scope in the form. In server details, `e` enables or disables, `r` reconnects, `l` starts OAuth sign-in, `o` signs out, and `d` removes the definition. The sign-in card shows a clickable browser link; press `c` to copy it if the browser did not open.

Setup requests in a Palepale conversation target Palepale. For a local server, run `palepale mcp add docs -- /path/to/mcp-server --stdio`; for a remote server, run `palepale mcp add docs --url https://example.com/mcp`. Definitions go in `~/.palepale/config.toml` (or `PALEPALE_HOME/config.toml` when set). Add `--scope project` before `--` to use `.palepale/config.toml` in the current folder. Existing entries are preserved; duplicate names in the selected scope are refused.

Run `palepale mcp check docs` to connect and list the tools. Use `palepale mcp auth docs` to sign in, `logout` to sign out, `enable` or `disable` to change startup, and `remove` to remove that user entry. These commands also work as `/mcp ...` in a session. Quote paths and arguments containing spaces. Project checks keep the trust and definition-review requirements from `/mcp`. Definitions reload between model requests, so a server added during a conversation becomes available there. Other coding clients keep their own settings.

Server definitions use the existing user or project configuration:

```toml
[mcp_servers.docs]
url = "https://developers.openai.com/mcp"

[mcp_servers.local]
command = "/path/to/your/mcp-server"
args = ["--stdio"]
cwd = "."
env_vars = ["SERVICE_TOKEN"]
read_only_tools = ["search"]
```

Use `bearer_token_env_var = "SERVICE_TOKEN"` for an HTTP bearer token, or `env_http_headers = { "X-API-Key" = "SERVICE_TOKEN" }` for a custom header. OAuth credentials are saved privately, separately from the Palepale account. Project servers require project trust and a review of each changed definition before starting. MCP calls follow MANUAL/AUTO permissions; PLAN blocks tool calls unless their names are explicitly listed in `read_only_tools`. Server annotations do not grant access. Connections run in the background, and large tool catalogs use local on-demand discovery. The agent can also list and read resources, inspect resource templates, and load prompts through `mcp_resources` and `mcp_prompts`.

`/skills` searches `SKILL.md` files in `.palepale/skills`, `.agents/skills`, `.claude/skills`, and `.opencode/skills` or `.opencode/skill`, including grouped subfolders. It checks the project up to its repository root and the matching personal folders. Press `a` to register another folder. Enter opens a preview; Enter again inserts `$skill-name ` into your draft. Add the task and send when ready. Use `e` to enable/disable a skill and `r` to rescan.

A skill starts with YAML metadata, followed by its instructions:

```markdown
---
name: review
description: Review a change for bugs and missing checks.
---
Read the diff, inspect affected code, and report concrete issues.
```

Keep it at `.agents/skills/review/SKILL.md`, with the name matching its folder. The agent sees enabled skills' names and descriptions, then calls the native `skill` tool to load instructions, their base directory, and up to ten supporting filenames. References load separately when needed. Use `/review task` or `$review task` to invoke it explicitly. `disable-model-invocation: true` requires explicit selection; `metadata: { opencode/autoinvoke: false }` has the same effect. `user-invocable: false` permits automatic use only, and `slash: false` hides its slash alias. Previews never execute scripts, and skill instructions retain the current permissions. Untrusted project skills stay inactive until the project is trusted.

Register an existing folder with `palepale skills add <folder>` or an index source with `palepale skills add https://example.com/skills/`. Verify with `palepale skills check <name>`. Remote sources download `index.json` and the listed files into a versioned cache; a failed refresh keeps the previous complete version. Use `skills refresh` to check immediately.

`palepale mcp list` and `palepale skills list` also work outside the TUI. MCP supports stdio, Streamable HTTP, and legacy HTTP+SSE. Connected prompts appear as `/server:prompt`, with positional arguments. Supported images, WAV/MP3 audio, and PDF resources reach attachment-capable models. See [MCP and skills](docs/mcp-and-skills.md) for configuration, permissions, and the pinned OpenCode comparison.

## Updates and settings

Run `palepale` to check for updates before opening the session. When a newer release is available, Palepale shows download progress, verifies `SHA256SUMS`, installs it, and opens the updated CLI in the same terminal. Rapid restarts share a check for one minute. An unavailable update service or failed checksum leaves the installed version usable. Press Ctrl+C during the check or download to continue with it.

`/status` opens a compact card with Session and Usage tabs. Shared charts show context, input/output tokens, and account budget use. Cached input and reasoning totals sit below the token bars. Session cost uses listed model rates; Usage shows confirmed account charges and available and reserved balances. Tab switches tabs, up/down scroll, `r` refreshes, and Escape closes the card. Missing usage data is shown as unavailable.

Settings live in `~/.palepale/config.toml`. Set `auto_update = false` to disable automatic updates.

## Development

See [Contributing](CONTRIBUTING.md) for builds, checks, configuration, and the source layout. Maintainers can find publishing instructions in [Releases](docs/releases.md).

## License

Licensed under [Apache-2.0](LICENSE-APACHE). Copyright 2026 amangly. See [NOTICE](NOTICE).

# Palepale

<p align="center"><img src="data/palepale-logo.svg" alt="Palepale logo" width="160"></p>

Palepale is a terminal coding agent. It reads code, edits files, runs commands, and streams model responses in your project folder.

## Install

Download the latest build for [Windows x64](https://api.palepale.tech/cli/latest/palepale-x86_64-pc-windows-msvc.exe), [macOS Apple Silicon](https://api.palepale.tech/cli/latest/palepale-aarch64-apple-darwin), or [Linux x64](https://api.palepale.tech/cli/latest/palepale-x86_64-unknown-linux-musl).

Run it from your project folder:

```powershell
.\palepale.exe
```

On macOS or Linux:

```bash
chmod +x palepale
./palepale
```

On the first run, enter your account number or pair the terminal at [palepale.tech/pair](https://palepale.tech/pair).

## Before and after `/godmode`

Run `/godmode` and choose a server profile. The selected profile stays visible and applies to the next requests.

<table>
  <tr>
    <th>Before</th>
    <th>After</th>
  </tr>
  <tr>
    <td><img src="docs/images/palepale-session.png" alt="Palepale before godmode"></td>
    <td><img src="docs/images/palepale-godmode.png" alt="Palepale after choosing godmode"></td>
  </tr>
</table>

The server keeps profile instructions private. The CLI receives the profile ID, name, and description.

## Controls

| Command | Action |
| --- | --- |
| `/models` | Choose a model |
| `/sessions` | Resume a saved conversation |
| `/resume` | Alias for `/sessions` |
| `/undo` | Take back the last prompt and restore its file checkpoint |
| `/retry` | Take back the last prompt and send it again |
| `/compact [focus]` | Summarize older turns while retaining recent context |
| `/prompt` | List local system prompts and choose a default |
| `/godmode` | Choose a coding profile |
| `/godmode off` | Clear the profile |
| `/status` | View session and usage |
| `/keys` | List account keys; revoke another device with `/keys revoke <ref> --yes` |
| `/mcp` | Manage MCP servers |
| `/skills` | Browse reusable skills |
| `/help` | Show commands |
| `Escape` | Stop the current turn |
| `Ctrl+D` | Quit |

New sessions start in AUTO. Use `/mode` or `Shift+Tab` to switch to MANUAL or PLAN.

Undo checkpoints cover files inside a Git work tree and respect its ignore rules. Files edited
after a completed turn are kept. An interrupted turn without a final checkpoint rewinds the
conversation only. Snapshots live under the CLI home, and concurrent sessions cannot share an
active snapshot index.

Long conversations compact automatically near the context limit. The original messages stay
in the session file. The `task` tool can delegate work to an `explore` agent with read-only
tools or a `general` agent with the parent's tool set and permission mode. Child tasks have
bounded depth, concurrency, and turns; stopping the parent cancels its children.

Run `palepale mcp discover` to list MCP definitions from supported local clients. Import one
with `palepale mcp import <name> --fingerprint <fingerprint>`, optionally adding `--scope project`.
Imports start disabled; review the configuration in `/mcp` before enabling it. Discovery does
not start servers or print environment/header values.

Use `palepale --continue` to reopen the latest project conversation, or
`palepale --resume <id>` for a specific one. Local prompt files and `/godmode`
profiles have separate selections. See [sessions and prompts](docs/reliable-sessions.md)
for recovery, instruction scopes, and model compatibility checks.

## MCP and skills

Use `/mcp` to add local or remote MCP servers and `/skills` to find project workflows. See [MCP and skills](docs/mcp-and-skills.md) for configuration and permissions.

## Updates and development

Running `palepale` checks for updates, verifies the release checksum, and installs a newer CLI when one is available. Set `auto_update = false` in `~/.palepale/config.toml` to disable it.

See [Contributing](CONTRIBUTING.md) for local builds and checks. The server defaults to `https://api.palepale.tech`; local development can point the CLI at a local API with `PALEPALE_API_URL`.

## License

Licensed under [Apache-2.0](LICENSE-APACHE). Copyright 2026 amangly. See [NOTICE](NOTICE).

# Palepale

<p align="center"><img src="data/palepale-logo.svg" alt="Palepale logo" width="160"></p>

Palepale is a terminal coding agent. It reads code, edits files, runs commands, and streams model responses in your project folder.

## Install

Download the latest build for [Windows x64](https://github.com/amangly/palepale-rs/releases/latest), [macOS Apple Silicon](https://github.com/amangly/palepale-rs/releases/latest), or [Linux x64](https://github.com/amangly/palepale-rs/releases/latest).

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
| `/godmode` | Choose a coding profile |
| `/godmode off` | Clear the profile |
| `/status` | View session and usage |
| `/mcp` | Manage MCP servers |
| `/skills` | Browse reusable skills |
| `/help` | Show commands |
| `Escape` | Stop the current turn |
| `Ctrl+D` | Quit |

New sessions start in AUTO. Use `/mode` or `Shift+Tab` to switch to MANUAL or PLAN.

## MCP and skills

Use `/mcp` to add local or remote MCP servers and `/skills` to find project workflows. See [MCP and skills](docs/mcp-and-skills.md) for configuration and permissions.

## Updates and development

Running `palepale` checks for updates, verifies the release checksum, and installs a newer CLI when one is available. Set `auto_update = false` in `~/.palepale/config.toml` to disable it.

See [Contributing](CONTRIBUTING.md) for local builds and checks. The server defaults to `https://api.palepale.tech`; local development can point the CLI at a local API with `PALEPALE_API_URL`.

## License

Licensed under [Apache-2.0](LICENSE-APACHE). Copyright 2026 amangly. See [NOTICE](NOTICE).

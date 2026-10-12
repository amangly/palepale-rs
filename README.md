# Palepale CLI

A Rust coding agent for your terminal. Read code, edit files, run commands, and switch models in one workspace.

## Get started

Download the binary for Windows x64, macOS ARM64, or Linux x64 from [Releases](https://github.com/amangly/palepale-releases/releases/latest). Rename it to `palepale.exe` on Windows or `palepale` on macOS/Linux.

Run it from your project folder:

```powershell
.\palepale.exe
```

On macOS/Linux, run `chmod +x palepale` once, then `./palepale`.

The first launch opens browser pairing. Sign in to your Palepale account, match the code, and choose **Connect CLI**. Later launches reuse the saved device key.

Run `palepale install` to add the CLI to your user PATH. Before installation, use `./palepale install` or `.\palepale.exe install`.

## Use

| Control | Action |
| --- | --- |
| Enter | Send a message |
| Shift+Enter or Ctrl+J | Insert a new line |
| Shift+Tab | Switch MANUAL, PLAN, and AUTO modes |
| `/models` | Pick a model |
| `/status` | Check your account and balance |
| `/help` | List commands |
| Escape | Interrupt the current turn |
| Ctrl+D | Quit with an empty composer |

MANUAL follows permission rules. PLAN restricts writes while preparing a plan. AUTO accepts actions automatically, including actions denied by permission rules.

Use `/logout` then `/login` to connect another account. Changing the browser account does not switch an already paired CLI.

## Updates and settings

Release builds check for updates at startup, at most once a day. Run `palepale update` to check now, then restart. Downloads are checked against `SHA256SUMS`; updates preserve your saved account, settings, and conversations.

Settings live in `~/.palepale/config.toml`. Set `auto_update = false` to disable automatic updates. `PALEPALE_HOME` changes the state folder; `PALEPALE_OFFLINE=1` starts without network access.

## Development

See [Contributing](CONTRIBUTING.md) for builds, checks, configuration, and the source layout. Maintainers can find publishing instructions in [Releases](docs/releases.md).

## License

Licensed under [Apache-2.0](LICENSE-APACHE). Copyright 2026 amangly. See [NOTICE](NOTICE).

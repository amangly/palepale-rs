# Palepale CLI

Use your [palepale.tech](https://palepale.tech) account to run a coding agent in your terminal. Read code, edit files, run commands, and switch models in your project.

## Get started

Download the CLI for Windows x64, macOS Apple Silicon, or Linux x64 from the [Palepale download page](https://palepale.tech/docs/cli) or [GitHub Releases](https://github.com/amangly/palepale-cli/releases/latest). Rename it to `palepale.exe` on Windows or `palepale` on macOS/Linux.

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
| `/status` | Check your account and balance |
| `/help` | List commands |
| Escape | Interrupt the current turn |
| Ctrl+D | Quit with an empty composer |

MANUAL follows permission rules. PLAN restricts writes while preparing a plan. AUTO accepts actions automatically, including actions denied by permission rules.

Opening `/models` fetches the latest catalog. If the server is unavailable, it shows the cached list.

`/effort` shows the levels reported by the model catalog. Choose `default` to reset it, or set a level directly with `/effort low`.

Use `/logout` then `/login` to connect another account. Changing the browser account does not switch an already paired CLI.

## Updates and settings

Release builds check for updates at startup, at most once a day. Run `palepale update` to check now, then restart. Downloads are checked against `SHA256SUMS`; updates preserve your saved account, settings, and conversations.

Settings live in `~/.palepale/config.toml`. Set `auto_update = false` to disable automatic updates.

## Development

See [Contributing](CONTRIBUTING.md) for builds, checks, configuration, and the source layout. Maintainers can find publishing instructions in [Releases](docs/releases.md).

## License

Licensed under [Apache-2.0](LICENSE-APACHE). Copyright 2026 amangly. See [NOTICE](NOTICE).

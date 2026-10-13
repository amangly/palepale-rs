# Palepale CLI

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
| `/help` | List commands |
| Escape | Interrupt the current turn |
| Ctrl+D | Quit with an empty composer |

MANUAL follows permission rules. PLAN restricts writes while preparing a plan. AUTO accepts actions automatically, including actions denied by permission rules.

Opening `/models` fetches the latest catalog. If the server is unavailable, it shows the cached list.

The opening screen reveals the Palepale logo with a brief Matrix effect from [ttfx](https://github.com/omacom/ttfx). The SVG becomes colored block pixels, keeping its shape and green gradient in Windows Terminal and other terminals. Typing skips the animation and keeps your input. `PALEPALE_NO_ANIMATION=1` skips it too. `NO_COLOR` shows the same shape without color.

`/effort` shows the levels reported by the model catalog. Choose `default` to reset it, or set a level directly with `/effort low`.

Use `/logout` then `/login` to connect another account. Changing the browser account does not switch an already paired CLI.

## Updates and settings

Run `palepale` to check for updates before opening the session. When a newer release is available, Palepale shows download progress, verifies `SHA256SUMS`, installs it, and opens the updated CLI in the same terminal. Rapid restarts share a check for one minute. An unavailable update service or failed checksum leaves the installed version usable. Press Ctrl+C during the check or download to continue with it.

`/status` opens a compact card with Session and Usage tabs. Shared charts show context, input/output tokens, and account budget use. Cached input and reasoning totals sit below the token bars. Session cost uses listed model rates; Usage shows confirmed account charges and available and reserved balances. Tab switches tabs, up/down scroll, `r` refreshes, and Escape closes the card. Missing usage data is shown as unavailable.

Settings live in `~/.palepale/config.toml`. Set `auto_update = false` to disable automatic updates.

## Development

See [Contributing](CONTRIBUTING.md) for builds, checks, configuration, and the source layout. Maintainers can find publishing instructions in [Releases](docs/releases.md).

## License

Licensed under [Apache-2.0](LICENSE-APACHE). Copyright 2026 amangly. See [NOTICE](NOTICE).

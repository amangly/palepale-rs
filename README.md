# Palepale

<p align="center">
  <img src="data/palepale-logo.svg" alt="Palepale logo" width="160">
</p>

<p align="center">
  <a href="https://github.com/amangly/palepale-rs/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/amangly/palepale-rs/ci.yml?branch=main&label=CI" alt="CI"></a>
  <a href="https://github.com/amangly/palepale-rs/releases"><img src="https://img.shields.io/github/v/release/amangly/palepale-rs?display_name=tag&sort=semver" alt="Release"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.98%2B-orange?logo=rust&logoColor=white" alt="Rust 1.98+"></a>
  <a href="https://tokio.rs/"><img src="https://img.shields.io/badge/runtime-Tokio-2d3748?logo=rust&logoColor=white" alt="Tokio runtime"></a>
  <a href="docs/mcp-and-skills.md"><img src="https://img.shields.io/badge/MCP-supported-6f42c1" alt="MCP supported"></a>
  <a href="#install"><img src="https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-4c8bf5" alt="Windows, macOS, and Linux"></a>
  <a href="LICENSE-APACHE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="Apache-2.0 license"></a>
</p>

Palepale is a terminal coding agent. It reads code, edits files, runs commands, and streams model responses in your project folder.

## Install

On macOS with Apple Silicon (M-series) or Linux x64:

```sh
curl -fsSL https://raw.githubusercontent.com/amangly/palepale-rs/main/data/install.sh | sh
```

On Windows x64, run in PowerShell:

```powershell
irm https://raw.githubusercontent.com/amangly/palepale-rs/main/data/install.ps1 | iex
```

The installers download the matching release, verify its SHA-256 checksum, and run
`palepale install` for your user. No administrator access is needed. The shell
installer adds `~/.local/bin` to your shell profile; Windows adds its install folder
to your user PATH. Open a new terminal, enter your project folder, and run:

```bash
palepale
```

macOS installation selects the Apple Silicon build even under Rosetta. Intel Macs,
Linux ARM, and Windows ARM do not have release builds yet. You can also download
the binaries directly for [Windows x64](https://api.palepale.tech/cli/latest/palepale-x86_64-pc-windows-msvc.exe),
[macOS Apple Silicon](https://api.palepale.tech/cli/latest/palepale-aarch64-apple-darwin),
or [Linux x64](https://api.palepale.tech/cli/latest/palepale-x86_64-unknown-linux-musl).

Use `/login` to pair the device or enter an inference key. You can also set `PALEPALE_API_KEY` to a `pk_` key. For a new account, run `palepale signup`; see [account setup](docs/bun-gateway.md) for owner credentials and recovery.

## Start a session

Type a task and press Enter. Use `/models` to choose a model and `/help` to see commands. New sessions start in AUTO, which runs tools without asking. Use `/mode manual` for approval prompts or `/mode plan` to investigate before editing.

Press Escape to stop a turn and Ctrl+D to quit. Resume the latest conversation with `palepale --continue`.

The [command reference](docs/commands.md) covers slash commands, shortcuts, and session controls.

## Godmode

Use `/godmode` to toggle the server-managed coding profile for the selected model. The profile stays on the backend; the CLI only sends the toggle.

https://github.com/user-attachments/assets/b2fdc082-ca27-4072-a5a2-e3253d2cdf64
## Guides

- [Commands and keyboard shortcuts](docs/commands.md)
- [Sessions, recovery, and project instructions](docs/reliable-sessions.md)
- [MCP servers and skills](docs/mcp-and-skills.md)
- [Accounts and Bun gateway compatibility](docs/bun-gateway.md)
- [Local builds and development](CONTRIBUTING.md)
- [Publishing releases](docs/releases.md)

Palepale checks for updates at interactive startup and verifies downloads before installing them. Set `auto_update = false` in `~/.palepale/config.toml` to disable this. The default API is `https://api.palepale.tech`; use `PALEPALE_API_URL` for another backend.

## License

Licensed under [Apache-2.0](LICENSE-APACHE). Copyright 2026 amangly. See [NOTICE](NOTICE).

## Promotional trial

Run `palepale trial`. It connects your account if needed, verifies the request
in your browser, and opens Discord with your application reference shown in the terminal.
After approval, enter the code at its hidden prompt. The CLI verifies, redeems,
and starts DeepSeek with trial funding. Browser verification follows the CLI
account even when a different account is signed in on the website.
Never put redemption codes or owner credentials in shell arguments.

The grant is bound to the originating inference credential and a signing key kept
in protected local state. Copying the API key alone cannot spend trial credit.
Deleting the signing state requires support review; a new key cannot replace the
registered one automatically. Software signing does not attest the CLI binary or
prevent a client with access to the full local state from acting as that device.

After redemption, `palepale --trial` selects the eligible DeepSeek model and
sends signed requests with explicit trial funding. Paid balance is never an
automatic fallback. The server checks model eligibility, device authorization,
remaining credit and shared daily capacity for every request.

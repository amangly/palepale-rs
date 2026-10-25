# Palepale

<p align="center"><img src="data/palepale-logo.svg" alt="Palepale logo" width="160"></p>

[![CI](https://img.shields.io/github/actions/workflow/status/amangly/palepale-rs/ci.yml?branch=main&label=CI)](https://github.com/amangly/palepale-rs/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/amangly/palepale-rs?display_name=tag&sort=semver)](https://github.com/amangly/palepale-rs/releases)
[![Rust](https://img.shields.io/badge/Rust-1.98%2B-orange?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Tokio](https://img.shields.io/badge/runtime-Tokio-2d3748?logo=rust&logoColor=white)](https://tokio.rs/)
[![MCP](https://img.shields.io/badge/MCP-supported-6f42c1)](docs/mcp-and-skills.md)
[![Platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-4c8bf5)](#install)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE-APACHE)

Palepale is a terminal coding agent. It reads code, edits files, runs commands, and streams model responses in your project folder.

## Install

Download the latest build for [Windows x64](https://api.palepale.tech/cli/latest/palepale-x86_64-pc-windows-msvc.exe), [macOS Apple Silicon](https://api.palepale.tech/cli/latest/palepale-aarch64-apple-darwin), or [Linux x64](https://api.palepale.tech/cli/latest/palepale-x86_64-unknown-linux-musl).

On Windows, rename the downloaded file to `palepale.exe` and run it from your project folder:

```powershell
.\palepale.exe
```

On macOS or Linux, rename the downloaded file to `palepale`, then run:

```bash
chmod +x palepale
./palepale
```

Use `/login` to pair the device or enter an inference key. You can also set `PALEPALE_API_KEY` to a `pk_` key. For a new account, run `palepale signup`; see [account setup](docs/bun-gateway.md) for owner credentials and recovery.

## Start a session

Type a task and press Enter. Use `/models` to choose a model and `/help` to see commands. New sessions start in AUTO, which runs tools without asking. Use `/mode manual` for approval prompts or `/mode plan` to investigate before editing.

Press Escape to stop a turn and Ctrl+D to quit. Resume the latest conversation with `palepale --continue`.

The [command reference](docs/commands.md) covers slash commands, shortcuts, and session controls.

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

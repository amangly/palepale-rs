# Releases

Source, binaries, checksums, `LICENSE-APACHE`, and `NOTICE` live in `amangly/palepale-rs`.

## Publishing

1. Set the package version in `Cargo.toml` and `Cargo.lock`. Each release needs a new version.
2. After an explicit release request, merge the reviewed `main` commit into `prod` and push `prod`.

The workflow uses GitHub's built-in token. Only the publishing job has Contents write permission; no personal release token is needed.

The release workflow runs lint, unit tests, and end-to-end checks. It builds Windows x64, macOS ARM64, and static Linux x64 binaries, checks the packaged updater and terminal startup on host runners, then uploads a draft with `SHA256SUMS` and license notices. It publishes the draft after every upload succeeds. Main pushes run CI without publishing a release.

Tags use `v<package-version>`, starting with `v0.1.0`. Existing tags cannot be reused. Published binaries show the same tag in `palepale --version` and the welcome screen. Optimized local builds use build number zero; debug builds do not update.

The repository is configured in `data/release.toml`; the API feed is in `data/endpoints.toml`.

## Updating

Release builds check before interactive startup and share a recent check for one minute. Download progress appears in the terminal. A successful checksum verification and replacement automatically reopen the CLI with the original arguments, environment, and working directory. Only a newer numeric version is installed; missing assets, invalid tags, or checksum failures keep the existing executable. Release-feed checks time out after five seconds, and Ctrl+C skips a pending check or download.

Earlier `0.2.0-*` builds use a separate release repository and will not install `0.1.0` automatically. Download the new binary from this repository and replace the executable once. Keep the CLI home folder to retain account state, settings, and sessions. Later updates use this repository.

Startup updates finish before a conversation begins. The optional `palepale update` command still installs an update without starting a session; its next launch uses the new binary. Account state, configuration, and sessions remain in the CLI home. The installation folder must be writable. Results are saved in `~/.palepale/update.json`.

`auto_update = false`, `PALEPALE_NO_UPDATE=1`, and offline mode disable automatic checks. Manual updates remain available outside offline mode. `PALEPALE_RELEASES_URL` overrides the feed for local testing.

SHA-256 verifies downloads against the release checksum file; it is not an independent publisher signature.

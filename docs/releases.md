# Releases

Source lives in `amangly/palepale-cli`. Binaries, checksums, `LICENSE-APACHE`, and `NOTICE` are published to `amangly/palepale-releases`.

## Publishing

1. Create a fine-grained GitHub token restricted to `amangly/palepale-releases`, with **Contents: read and write**.
2. Store it as the source repository's Actions secret `PALEPALE_RELEASE_TOKEN`. Never put its value in source or customer configuration.
3. After an explicit release request, merge the reviewed `main` commit into `prod` and push `prod`.

The release workflow runs lint, unit tests, and end-to-end checks. It builds Windows x64, macOS ARM64, and static Linux x64 binaries, checks the packaged updater and terminal startup on host runners, then uploads a draft with `SHA256SUMS` and license notices. It publishes the draft after every upload succeeds. Main pushes run CI without publishing a release.

Tags use `v<package-version>-<workflow-run-number>`. Published binaries show the same tag in `palepale --version` and the welcome screen. Optimized local builds use build number zero; debug builds do not update.

The download repository is configured in `data/release.toml`; the API feed is in `data/endpoints.toml`. Keep the website address out of the download repository's homepage, README, and release notes.

## Updating

Release builds check at interactive startup, at most once every 24 hours. `palepale update` checks immediately. Only a newer numeric version is installed; missing assets, invalid tags, or checksum failures keep the existing executable.

The current session continues after an update; the next launch uses the new binary. Account state, configuration, and sessions remain in the CLI home. The installation folder must be writable. Results are saved in `~/.palepale/update.json`.

`auto_update = false`, `PALEPALE_NO_UPDATE=1`, and offline mode disable automatic checks. Manual updates remain available outside offline mode. `PALEPALE_RELEASES_URL` overrides the feed for local testing.

SHA-256 verifies downloads against the release checksum file; it is not an independent publisher signature.

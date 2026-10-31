# Releases

## v0.1.33 — 2026-10-31

Adds CLI-started trial requests and redemption with browser verification. Promotional
credit is restricted to DeepSeek and the originating CLI credential. Use
`palepale trial` for current credit and `palepale --trial` for explicit trial
funding. Exhaustion never retries against paid balance.


## v0.1.32 — 2026-10-31

The agent can finish a successful tool batch without a separate closing request.
Completion still waits for configured verification and queued work; the summary
is displayed and saved only after success.

Reads accept per-file ranges, and searches accept multiple roots with one shared
result limit. Plan review shows the full plan and sends user edits back to the
model. Successful decompiler answers are cached privately. Repeated-call checks
now detect interleaved stalls and reset when output changes.

## v0.1.31 — 2026-10-31

Godmode hides any leading bracketed profile label, including long names and Unicode,
using a shared regex instead of a fixed name list. Filtering handles streamed tags
and preserves Markdown links and bracketed text later in a reply.

## v0.1.30 — 2026-10-31

The CLI adds `palepale web`, which creates a short-lived one-use browser handoff
from the owner credential saved by `palepale signup`. The browser receives its
normal HttpOnly session cookie; owner credentials never enter the URL or browser
storage. The handoff is headless-safe when `PALEPALE_NO_BROWSER=1` is set.

## v0.1.29 — 2026-10-28

The model picker adds All, Uncensored and Cheapest filters, switched with Tab.
Cheapest sorts by the combined price of 1M input and 1M output tokens. Model names
come from the gateway's public presentation, with legacy names and IDs as fallbacks.

The three initial uncensored model IDs ending in `-standard` resolve to their
current `-zero-refusal` IDs when those models are available. Saved preferences
migrate after catalog discovery; old session selections and explicit model flags
also resolve. Exact IDs supplied by another backend still take precedence.

Released commit `f1b800b` through [release run 40719775660](https://github.com/amangly/palepale-rs/actions/runs/40719775660).
Local lint, 481 unit tests, one fake-MCP test, 101 end-to-end tests and four battery
tests passed; two opt-in battery tests were skipped. All release-platform checks
passed. The downloaded Windows package also passed the renamed-selection and
filter terminal tests.

The Azure feed and latest downloads now serve v0.1.29. All three binaries matched
the release checksums, and the Linux version was checked before switching the feed.
The installed Windows updater completed the v0.1.28 to v0.1.29 upgrade; its SHA-256 is
`16a34bafec9e8eec2a407ed9913bd59d2e03f5dc9c78467b3d93cf33cdcad538`.

## v0.1.26 — 2026-10-27 (Windows)

Built from clean source commit `38610ac` and published directly, without CI/CD. Main activity loading uses the same braille sequence as tool cards. Godmode colors only input borders red; typed text, prompt and cursor retain their normal colors. The unrelated local forum change is excluded.

Local lint, 468 unit tests, one fake-MCP test, 97 end-to-end tests and four battery tests passed; two opt-in battery tests were skipped. The tagged release binary also passed packaged checksum/update, startup replacement and three godmode terminal checks.

GitHub and public-server downloads match SHA-256 `4f7ec0917363009c947a581a2d4596a9e738e5be7065c10a2669aa754cca5b8c`. The release includes SHA256SUMS, LICENSE-APACHE and NOTICE. The updater feed advertises only the new Windows asset. macOS/Linux remain on v0.1.25; their existing latest download URLs still work. Their updaters retain the installed binary when the new release lacks a matching asset.

On the host, `/srv/palepale-cli/latest` points to `current-windows-v0.1.26`, which links the Windows files to `v0.1.26` and the two other binaries to `v0.1.25`. Its checksum manifest covers these current downloads. The immutable `v0.1.26` directory and release feed contain Windows-only assets. A later complete release can replace the latest symlink normally.

## v0.1.25 — 2026-10-27

Released commit `e9de99e7a9c3acc11e8d21a0805c61e34390a345`. Godmode uses a red input field and hides leading persona tags. Profile discovery is scoped to the selected model. Interrupted replies retain their text and show one warning with the request ID; unconfirmed tool calls remain unexecuted and requests are not automatically repeated.

[Release run 40397370577](https://github.com/amangly/palepale-rs/actions/runs/40397370577) passed platform lint, unit and end-to-end tests, builds, and packaged updater checks. Local validation passed 469 unit tests, the fake MCP test, 97 end-to-end tests and four battery tests; two opt-in battery tests were skipped. The downloaded Windows release also passed three godmode terminal tests and the interrupted-tool-call test.

The backend verified the release checksums and Linux version before switching `/srv/palepale-cli/latest` to v0.1.25. All six public downloads matched GitHub's SHA-256 digests. The installed Windows updater completed the v0.1.24 to v0.1.25 upgrade and reported the new version. Matching backend commit `bb94fd4` is deployed. These changes clarify interrupted output; they do not establish that every provider interruption is resolved or reconcile older usage holds.

## v0.1.24 — 2026-10-26

Released commit `a19bba13fc35491663981f8dbcc57160005bbaeb` through the `prod` workflow. The CLI authenticates profile discovery and sends `/godmode` selections as `server_profile`, matching the deployed Bun gateway. Profile instructions remain on the server; account instructions are managed on the website.

[Release run 40308737668](https://github.com/amangly/palepale-rs/actions/runs/40308737668) passed lint, unit tests, end-to-end tests, platform builds and packaged updater checks. The backend verified the published checksums and Linux version before atomically switching `/srv/palepale-cli/latest` from v0.1.23 to v0.1.24. Public downloads of all six assets matched GitHub's SHA-256 digests; the feed and latest download URLs were checked, and the downloaded Windows binary reported `palepale 0.1.24 (v0.1.24)`.

Matching backend commit `85f0e55` and website commit `5254b55` are deployed. Live account instruction save/reload/clear checks passed without payments or inference. Run `palepale update` to install the release.

Source and build artifacts live in the public `amangly/palepale-rs` repository. Public binaries, checksums, `LICENSE-APACHE`, and `NOTICE` are served at `https://api.palepale.tech/cli/latest/`.

## Publishing

1. Set the package version in `Cargo.toml` and `Cargo.lock`. Each release needs a new version.
2. After an explicit release request, merge the reviewed `main` commit into `prod` and push `prod`.
3. Publish the verified assets to the current backend host and atomically switch the public feed. Check the server repository's `docs/SSH-VPS.md` for the current host. The old `deploy/publish-cli.py` and Azure Run Command scripts target a retired VM. A GitHub release alone does not update customers.

The workflow uses GitHub's built-in token. Only the publishing job has Contents write permission; no personal release token is needed.

The release workflow runs lint, unit tests, and end-to-end checks. It builds Windows x64, macOS ARM64, and static Linux x64 binaries, checks the packaged updater and terminal startup on host runners, then uploads a draft with `SHA256SUMS` and license notices. It publishes the draft after every upload succeeds. Main pushes run CI without publishing a release.

Tags use `v<package-version>`, starting with `v0.1.0`. Existing tags cannot be reused. Published binaries show the same tag in `palepale --version` and the welcome screen. Optimized local builds use build number zero; debug builds do not update.

The repository is configured in `data/release.toml`; the API feed is in `data/endpoints.toml`.

## Updating

Release builds check before interactive startup and share a recent check for one minute. Download progress appears in the terminal. A successful checksum verification and replacement automatically reopen the CLI with the original arguments, environment, and working directory. Only a newer numeric version is installed; missing assets, invalid tags, or checksum failures keep the existing executable. Release-feed checks time out after five seconds, and Ctrl+C skips a pending check or download.

Earlier `0.2.0-*` builds use a separate release repository and will not install `0.1.0` automatically. Download the new binary from the server and replace the executable once. Keep the CLI home folder to retain account state, settings, and sessions. Later updates use the server.

Builds through v0.1.18 use GitHub by default. To migrate without replacing the binary manually, set `PALEPALE_RELEASES_URL=https://api.palepale.tech/cli` and run `palepale update`. Remove the override after installing v0.1.19 or later.

Startup updates finish before a conversation begins. The optional `palepale update` command still installs an update without starting a session; its next launch uses the new binary. Account state, configuration, and sessions remain in the CLI home. The installation folder must be writable. Results are saved in `~/.palepale/update.json`.

`auto_update = false`, `PALEPALE_NO_UPDATE=1`, and offline mode disable automatic checks. Manual updates remain available outside offline mode. `PALEPALE_RELEASES_URL` overrides the feed for local testing.

SHA-256 verifies downloads against the release checksum file; it is not an independent publisher signature.

## Release signing setup

The updater supports Minisign signatures over `SHA256SUMS`. Signing is not enabled until
`signing.keys` in `data/release.toml` contains a public key. With an empty key list, updates
still check SHA-256 checksums but do not authenticate the publisher.

Generate the release key on a trusted machine with `minisign -G -W -p release.pub -s release.key`.
Keep `release.key` out of the repository. This creates an unencrypted key for unattended CI;
store its complete contents in the GitHub Actions secret `PALEPALE_SIGNING_KEY`. Put the
base64 public-key line from `release.pub` first in `signing.keys` and commit that configuration.
The publishing workflow refuses a mismatch between the public-key configuration and secret.

The workflow signs the manifest with the trusted comment `palepale release <tag>` and verifies
the result before publishing. Clients with a configured key reject missing signatures,
altered manifests, unknown keys, and signatures for another release tag. CI secret access
therefore grants release-signing authority.

For rotation, ship a release accepting both keys before switching the signing secret. Keep
the active signing key first in the list so CI can verify its output. Remove the old public
key only after clients have had time to install a release that trusts the new key. Existing
unsigned clients cannot authenticate the first transition to a signed build; distribute its
checksum and public key through a trusted channel.

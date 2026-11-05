# Releases

## v0.1.50 — 2026-11-06

Connect Antigravity with `palepale antigravity`, then start the CLI with
`palepale antigravity --run`. Use `--logout` to disconnect.
See [client setup](clients.md#antigravity-cli-and-ide) for model selection and SSH setup.

## v0.1.49 — 2026-11-05

Codex setup now shows model-returned thinking automatically. Run `palepale codex`
again after updating, then restart Codex. Thinking appears when the model returns it.

## v0.1.48 — 2026-11-04

Connect Cline CLI and its VS Code extension with `palepale cline`. Setup adds
Palepale's models, prices and image capabilities while keeping other providers.
Restart Cline after setup. In VS Code, select Palepale in the provider list.

Use `palepale cline <model-id>` to choose a model or `palepale cline --logout`
to disconnect both clients. Disconnect works offline and keeps your Palepale login.

## v0.1.47 — 2026-11-04

Image-capable models can now open local PNG, JPEG, GIF and WebP files with
`view_image`. Give the file path in your prompt. Images follow read permissions
and stay in the session so the model can use them on later turns.

Run `palepale models --refresh` to load current capabilities. For OpenCode, run
`palepale opencode` again and restart it.

## v0.1.46 — 2026-11-04

The Uncensored filter now includes models with the Zero Refusal badge, including
Palepale. Open `/models` and press Tab to switch filters.

## v0.1.45 — 2026-11-04

Claude Code now lists every compatible Palepale model in `/model`. Rerun
`palepale claude` after updating, then restart Claude Code.

Disconnect an app with `palepale opencode --logout`, `palepale codex --logout`,
or `palepale claude --logout`. These commands work offline and keep your
Palepale login and other providers.

The gateway now accepts desktop code-mode tools, including a normal message
with the free-form `exec` tool enabled.

## v0.1.44 — 2026-11-04

Run `palepale codex` or `palepale claude` to connect an installed client to your
Palepale account in API mode. Add a model ID to select one explicitly, then restart
the client. Setup backs up changed settings and stores the inference key privately.

The gateway supports stateless Responses and Messages requests, streamed text,
function tools and text tool results through the existing billing and spending
limits. See [client setup](clients.md) for supported features and requirements.

## v0.1.43 — 2026-11-04

Removed the built-in forum integration, including `/forums`, forum tools,
login screens, and automatic instructions to search forums first. Existing
local forum state is ignored and left untouched.

## v0.1.42 — 2026-11-03

The command menu opens when you type `/` after draft text, such as `asdasd /`.
Choose a command with Up/Down, then Tab or Enter to run it and keep the
surrounding draft. Escape closes the menu and keeps the typed text.

The `@` picker includes hidden and ignored files throughout the current
directory. Up/Down scrolls through every discovered match. Git internals and
directory symlinks are omitted; existing discovery limits and read permissions
still apply.

## v0.1.41 — 2026-11-02

The `/` command menu now works while the agent is replying. Type `/` at the
start of the prompt, use Tab to complete a command, and Enter to run it.

Type `$` anywhere in a message to find an installed skill. Tab or Enter inserts
the selected name, including skills without a slash command. Escape closes the
menu and keeps your draft.

Use `@` for files inside folders, such as `@src/auth/login.rs`. Paths with spaces
are quoted when selected. File and skill selection preserve the surrounding
text; press Enter again to send the message.

## v0.1.40 — 2026-11-02

Type `@` to find project files, folders, and agents. Use Up/Down to select a
result and Tab or Enter to insert it. Enter sends the message after selection;
Tab still steers a running turn when the picker is closed.

File mentions add read results to the conversation, with normal permissions and
size limits. Folder mentions add a directory listing. Use quotes for paths with
spaces, and `@@name` for a literal `@name`. Saved sessions retain the read results.

Use `@explore` to inspect code or `@general` to delegate a task. Children inherit
the current mode and spending budget; PLAN restrictions and configured checks
still apply. Example: `@explore explain authentication in @src/auth.rs`.

## v0.1.39 — 2026-11-02

The agent can search when it needs current information. It uses the selected
model's built-in search and returns source links. No command or model switch is
needed. Search and token charges share your run's spending limit. The tool is
unavailable in trial mode and on models without search support.

Drag to select transcript text, then copy it. Selection follows scrolling and
resizing; Escape clears it. The terminal's native selection remains available.

## Publishing

1. Set the package version in `Cargo.toml` and `Cargo.lock`. Each release needs a new version.
2. After an explicit release request, merge the reviewed `main` commit into `prod` and push `prod`.
3. Promote the verified assets to the public update feed using the private release runbook, then verify public download checksums. A GitHub release alone does not update customers.

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

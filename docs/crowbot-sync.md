# Crowbot sync — 2026-10-31

Compared Palepale `1f072a5` with Crowbot main at
[`1a47b44`](https://github.com/emlinhax/crowbot-cli/commit/1a47b44cfa),
published on 2026-10-29. The source changes are adapted to Palepale's I/O,
permission and model interfaces. Upstream attribution remains in `NOTICE`.

| Upstream change | Palepale status |
| --- | --- |
| [Decompiler cache](https://github.com/emlinhax/crowbot-cli/commit/940c8821f6) | Ported. Successful answers are private files under `~/.palepale/cache/decompile/`, keyed by program, binary and adjacent PDB metadata, arguments and working directory. Missing metadata, failures, empty answers and inputs changed during execution skip caching. |
| [Shared SHA-256 formatting](https://github.com/emlinhax/crowbot-cli/commit/b61d5b72db) | Ported. Cache names, extension hashes, snapshot paths and updater checks share one helper without changing existing digests. |
| [Full plan display and edited-plan feedback](https://github.com/emlinhax/crowbot-cli/commit/4bc27c36c7) | Ported. The full plan appears in the scrollable conversation. The card names its file; edits made during review reach the model. Relative plan paths use normal tool resolution. |
| [Repeated PLAN instructions](https://github.com/emlinhax/crowbot-cli/commit/dedccaa1cc) | Ported. Later user messages repeat PLAN instructions, which now name `question` and `plan_exit`. |
| [Interleaved repeated-call guard](https://github.com/emlinhax/crowbot-cli/commit/56c27fd) | Ported. Alternating identical calls count toward the existing guard; changing output counts as progress. A new user message resets the window. |
| [Per-file read ranges and multi-root grep](https://github.com/emlinhax/crowbot-cli/commit/34345ca) | Ported. Existing string arguments still work. Per-file options override shared defaults. Search roots share one result cap, overlapping files appear once, and every root is checked for permissions. |
| [Model presets](https://github.com/emlinhax/crowbot-cli/commit/0ece3cdb78) | Separate work. Routing per request needs account signals, picker support and defined behavior with Palepale's model-scoped godmode profiles and saved selections. |
| [Open an editor from the plan card](https://github.com/emlinhax/crowbot-cli/commit/d44c07a64c) | Separate work. The named file can be edited in another editor during review; the card does not yet suspend the terminal to launch one. |
| [Send unfinished PLAN runs back once](https://github.com/emlinhax/crowbot-cli/commit/5ff2dc060f) | Separate work. This adds an automatic paid request. This batch improves instructions without adding that request. |
| [Remove the main turn limit](https://github.com/emlinhax/crowbot-cli/commit/51cf4fa89f) | Retained Palepale's configured bound; removing a spend limit is a separate policy change. |
| [Guarded finish tool](https://github.com/emlinhax/crowbot-cli/commit/e42de31) | Separate work. Requires defined behavior for failed batches, queued messages and configured verification. |
| [Test portability](https://github.com/emlinhax/crowbot-cli/commit/a7ec4b9cc0), [PTY readiness](https://github.com/emlinhax/crowbot-cli/commit/1a47b44cfa), [account-number matching](https://github.com/emlinhax/crowbot-cli/commit/ec8f981a1c) | Not copied wholesale: the harnesses differ. New path assertions in this batch compare resolved paths. |

Older additions such as tool trees, command previews, optional verification,
first-turn orientation, persistent command directories and null-array stream
decoding are already represented in Palepale.

The cache uses path, size and modification time rather than hashing whole binaries.
External PDB locations and cache pruning remain known limits. The session file
format and stored model selections are unchanged. `plan_exit` prompt events gain
an additive `file` field.

Validation uses local fixtures. No paid inference or live benchmark was run.
Local validation passed lint, 492 unit tests, one fake-MCP test, 101 end-to-end
tests and four battery tests. Two opt-in battery tests were skipped.

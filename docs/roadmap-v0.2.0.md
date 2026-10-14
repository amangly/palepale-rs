# v0.2.0: reliable coding sessions

Resume a coding task, attach the files it needs, continue past the context limit, and review or reverse a completed turn.

[GitHub milestone](https://github.com/amangly/palepale-rs/milestone/1) · [Tracking issue #9](https://github.com/amangly/palepale-rs/issues/9)

Planned against Palepale [v0.1.11](https://github.com/amangly/palepale-rs/releases/tag/v0.1.11), commit `a1b5906`. The [OpenCode v2 docs](https://opencode.ai/v2/docs) were reviewed on 14 October 2026. This document plans the next release; the linked issues contain its acceptance criteria. No delivery date is set.

## Current baseline

Palepale already has a native Rust agent loop, streaming tools, model and effort selection, AUTO/MANUAL/PLAN modes, account usage charts, startup updates, and MCP/skill managers. Keep those workflows.

The next release addresses these gaps:

| Area | v0.1.11 behavior | Next behavior |
| --- | --- | --- |
| Sessions | Appends JSONL; each launch starts a new transcript | Resume the active history with a single writer |
| Context | Repairs tool pairs; no budget or compaction | Budget requests and summarize older complete turns |
| Instructions | Loads a supported file from the launch directory | Apply global, project, and nested rules with refresh |
| Input | Typed attachments from MCP results | Attach files, line ranges, and images directly |
| Commands | Built-ins, skills, and MCP prompts | Add project/user Markdown prompt templates |
| Changes | Tool-level diff display | Review the session diff and undo/redo a turn |
| Compatibility | Comparison against OpenCode v1.19.0 | Dated v2 behavior matrix and supported import mappings |

See [the existing MCP/skills comparison](mcp-and-skills.md) for shipped behavior. Its v1 comparison does not establish v2 parity.

## Build order

Start with session persistence. Instruction scope, model capability metadata, and the v2 audit can proceed independently. All work remains in the existing Rust executable.

| Stage | Work | Priority | Depends on | Reference |
| --- | --- | --- | --- | --- |
| Foundations | [#1](https://github.com/amangly/palepale-rs/issues/1) Session picker, --continue/--resume, active heads, writer locks | P0 | None | [CLI commands](https://opencode.ai/v2/docs/cli/commands) |
| Foundations | [#2](https://github.com/amangly/palepale-rs/issues/2) Global/nested AGENTS.md, precedence, refresh | P0 | None | [Instructions](https://opencode.ai/v2/docs/instructions) |
| Foundations | [#3](https://github.com/amangly/palepale-rs/issues/3) Model modalities, limits, and request preflight | P0 | None | [Models](https://opencode.ai/v2/docs/models) |
| Foundations | [#4](https://github.com/amangly/palepale-rs/issues/4) v2 MCP/skill audit and import rules | P1 | None | [MCP](https://opencode.ai/v2/docs/mcp-servers), [skills](https://opencode.ai/v2/docs/skills) |
| Continue the task | [#5](https://github.com/amangly/palepale-rs/issues/5) Manual and automatic context compaction | P0 | [#1](https://github.com/amangly/palepale-rs/issues/1), [#3](https://github.com/amangly/palepale-rs/issues/3) | [Compaction](https://opencode.ai/v2/docs/compaction) |
| Continue the task | [#6](https://github.com/amangly/palepale-rs/issues/6) @file picker, ranges, images, --file | P1 | [#3](https://github.com/amangly/palepale-rs/issues/3), [#2](https://github.com/amangly/palepale-rs/issues/2) | [Attachments](https://opencode.ai/v2/docs/attachments), [TUI](https://opencode.ai/v2/docs/cli/tui) |
| Continue the task | [#7](https://github.com/amangly/palepale-rs/issues/7) Markdown slash commands and arguments | P1 | [#2](https://github.com/amangly/palepale-rs/issues/2) | [Commands](https://opencode.ai/v2/docs/commands) |
| Review and recover | [#8](https://github.com/amangly/palepale-rs/issues/8) Session diff and checkpoint undo/redo | P1 | [#1](https://github.com/amangly/palepale-rs/issues/1) | [Snapshots](https://opencode.ai/v2/docs/snapshots) |

P0 defines the persistence and request boundaries. P1 completes this milestone's input, review, and compatibility work; it is still required before release.

### Implementation choices

- Extend the session format additively. Resume follows an explicit head, restores model/effort/mode, and never repeats a completed external action. Compaction preserves the original transcript.
- Use the configured Palepale home, including PALEPALE_HOME, for user commands and instructions. Preserve the current CLAUDE.md fallback where AGENTS.md is absent. Nested rules stay within their directory scope.
- Reuse the existing attachment representation. Capability checks happen before a model request, and an incompatible model leaves the draft intact.
- Begin custom commands with prompt templates and argument expansion. Shell substitution and agent overrides wait for their execution boundaries.
- Capture checkpoints at turn boundaries in Git projects. Preserve the existing index and dirty baseline; detect newer editor changes before restoration. External actions have no filesystem rollback.
- Keep Palepale TOML as the native configuration. The v2 audit must distinguish accepted imports, unsupported fields, and deliberate differences. Existing Palepale skill IDs remain compatible.

Put runtime I/O in `src/io/`; put prompts, schemas, configuration, limits, and copy in `data/`. Reuse the command registry and TUI components. New sessions retain AUTO by default; instruction files and templates follow the existing trust and mode rules. Setup requests continue to target Palepale.

## Release checks

The tracking issue owns the full checklist. The release must pass one fixture-based walkthrough: load scoped instructions, select a model, attach a file, invoke a command, edit, interrupt, resume, compact, restart, inspect the diff, undo, redo, and branch from an earlier turn.

Also require:

- Old sessions/configuration remain readable and MCP/skills regressions pass.
- Lint, unit, local integration, and packaged startup checks pass across Windows x64, Linux x64, and macOS arm64 CI.
- Keyboard navigation, cancellation, 64-column layout, and NO_COLOR work in the new screens.
- Existing account pairing, streaming, permissions, headless JSON, and startup updates keep working.

Development uses local provider and MCP fixtures. Live paid model tests require an explicit request. Release notes must list implemented behavior, limits, and remaining differences.

## Ordered follow-up work

These are planning candidates after v0.2.0, not additional acceptance criteria for it.

| Order | Candidate | First useful scope | Reference |
| --- | --- | --- | --- |
| 1 | Named agent profiles and delegated tasks | Native Rust profiles, a bounded child-session lifecycle, explicit tool limits, cancellation, and returned results. Begin with read-only explore/review work. | [Agents](https://opencode.ai/v2/docs/agents), [tools](https://opencode.ai/v2/docs/tools) |
| 2 | Background shell jobs and formatters | Working directories, owned job IDs, bounded output, completion events, and opt-in formatters already installed locally | [Tools](https://opencode.ai/v2/docs/tools), [formatters](https://opencode.ai/v2/docs/formatters) |
| 3 | External references and web search | Read-only source roots with clear provenance; optional native search adapters with cited results | [References](https://opencode.ai/v2/docs/references), [websearch](https://opencode.ai/v2/docs/websearch) |
| 4 | Broader providers and integrations | Decide from concrete demand, preserving the Palepale account path and one request interface | [Providers](https://opencode.ai/v2/docs/providers) |

Keep the compact Matrix TUI and native Rust distribution. OpenCode's shared server, JavaScript plugin runtime, and Code Mode are separate architecture decisions outside this release. Any future MCP protocol revision needs review against the protocol specification before implementation.

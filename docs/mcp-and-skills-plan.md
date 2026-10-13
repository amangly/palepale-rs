# `/mcp` and `/skills`

Proposal, 14 October 2026.

Add two compact managers to Palepale's existing slash menu. `/mcp` connects external tools; `/skills` finds and selects reusable workflows. Both run inside the Rust CLI and use its current agent, permission prompts, and terminal components. This feature needs no backend or AWS changes.

Claude Code and Codex both expose `/mcp`. Codex also uses `/skills` and `$skill-name` for explicit skill selection. These are familiar starting points for Palepale's interface. [Claude Code MCP](https://code.claude.com/docs/en/mcp), [Codex MCP](https://developers.openai.com/codex/mcp), [Codex skills](https://developers.openai.com/codex/skills).

## The two screens

Use one bordered list with a detail view, matching the current model picker. Target a 64-column card that fits a 24-row terminal and shrinks with the window. Keep the Matrix green selection, muted secondary text, and short keyboard footer. Show state in words as well as color.

| Command | List rows | Enter opens | Main actions |
| --- | --- | --- | --- |
| `/mcp` | Name, connection state, tool count, user/project scope | Address or command, auth state, available tools, last error | Add, connect/retry, sign in, enable/disable, remove |
| `/skills` | Name, one-line description, user/project scope | Instructions preview, source folder, enabled state | Use, add folder, enable/disable, refresh |

Both lists support search, arrows, Enter, and Esc. Opening either manager preserves the draft. Connection, login, and discovery jobs run beside keyboard input; closing a card prevents late results from reopening it.

### `/mcp` setup

1. Choose **Add**, then **Remote URL** or **Local command**.
2. Enter a name and the URL, or the program and its arguments. Default to user scope; offer project scope for shared configuration.
3. Preview the source and connection details, save, then connect in the background.
4. Show `connecting`, `connected`, `sign in`, `off`, or `error`. A failed server leaves the rest of Palepale usable.

The detail view exposes **Sign in** only when needed. Use browser OAuth, keep a copyable sign-in URL visible, and report browser-launch failure in the card. Logout removes that server's stored credentials. Bearer authentication refers to an environment variable by name, so tokens do not enter command history or project files.

### `/skills` selection

Selecting **Use** inserts `$skill-name ` into the composer and returns to the draft. The next send invokes the skill with the accompanying request. Selection alone does not send a message. Enabled skills can also be chosen by the agent when their descriptions match the task.

**Add folder** registers an existing skill folder or skills directory. This allows reuse of Claude Code and Codex skill folders without moving or rewriting their files. Installation from GitHub can follow later.

## Skills behavior

Use the open Agent Skills format: a folder containing `SKILL.md`, YAML `name` and `description`, and optional references, scripts, and assets. Validate metadata per skill and show an invalid entry's reason without breaking the list. [Agent Skills specification](https://agentskills.io/specification).

Discover project skills in `.agents/skills` from the launch directory up to the repository root, plus personal skills in `~/.agents/skills`. Include explicitly registered folders. Deduplicate canonical paths. A trusted project skill takes precedence over a personal skill with the same name; the picker shows the shadowed source. Explicit invocation must report a missing or disabled skill clearly.

Load names, brief descriptions, and source paths into a bounded catalog. Load the full instructions only on explicit invocation or when the agent selects the skill. This follows Codex's progressive loading pattern. [Codex skill loading](https://developers.openai.com/codex/skills).

For explicit invocation, resolve the selected skill and check its read permission before adding its instructions to the turn. For implicit use, the agent reads the listed `SKILL.md` through the existing read tool. Attribute loaded instructions to their source and preserve the original invocation in the transcript. Supporting files load only when needed, through normal file tools and path checks.

Treat invocation arguments as text. Scanning or previewing a skill never runs its scripts. Skill metadata cannot grant permissions, change the active mode, or start an MCP server. Unsupported Claude-specific execution extensions receive a clear compatibility message rather than being silently executed. Untrusted project skills remain visible but inactive until the project is trusted.

Refresh discovery when opening `/skills` and before a new turn. Start with file stamps and explicit refresh; a filesystem watcher is unnecessary for this release.

## MCP behavior

Use the official Rust SDK, `rmcp`, with client features only. Start with **3.5.1**, released 5 October 2026. Confirm its Windows build and required lifecycle APIs in the first implementation step, then pin the dependency and lockfile. Its declared Rust minimum is below Palepale's pinned compiler, and its reqwest dependency uses the same `0.13` series as Palepale. [SDK release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.5.1), [SDK source](https://github.com/modelcontextprotocol/rust-sdk).

Support local **stdio** and remote **Streamable HTTP**. Use SDK negotiation for the current `2026-07-28` discovery lifecycle and compatible legacy servers, including `2025-11-25`. Test both explicitly; calling the SDK's older `serve()` helper alone would leave the modern discovery path unused. [SDK client lifecycle](https://github.com/modelcontextprotocol/rust-sdk#client-lifecycle-modes), [MCP transports](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports).

The first release supports tool listing, pagination, calls, text results, structured JSON, cancellation, and tool-list refresh. Show unsupported result types explicitly. Advertise only implemented client capabilities. MCP resources, prompts, images, sampling, elicitation, and long-running task workflows are separate follow-ups.

Keep each server's lifecycle in one runtime: connect, discover, call, disconnect, and cancel. Start enabled, trusted servers asynchronously. Give startup and calls separate timeouts. Reconnection can retry discovery, but must not automatically repeat a tool call after a timeout or disconnect: its remote action may already have completed. Serialize calls per server initially; retain native tools' current concurrency.

Map tool names to stable provider-compatible aliases such as `mcp__docs__search`, with deterministic collision handling. Keep the original server/tool identity in the dispatch map. Validate schemas before advertising them and isolate an invalid tool so it cannot break every model request. Preserve `isError` and structured results, bound visible output, and reuse the existing full-output spill mechanism.

Keep small tool catalogs directly available. When their schemas exceed a configurable context budget, expose a local `mcp_search` tool that searches cached names and descriptions and makes selected schemas available on the next model request. Use ordinary function calling so this works across Palepale's supported providers. This adopts the context-saving purpose of Claude Code's tool search without requiring its provider-specific tool-reference format. [Claude Code tool search](https://code.claude.com/docs/en/mcp#scale-with-mcp-tool-search).

## Configuration and permissions

Store server definitions in `[mcp_servers.<name>]` inside the existing user or project `config.toml`. A definition contains either a URL or a program plus argument array, an enabled flag, environment-variable references, and optional timeout overrides. A project definition replaces a same-name user definition as a whole, preventing mixed command/URL settings from recursive merging.

Use the existing project-trust mechanism. Review new or changed project server definitions before they start, and remember the reviewed definition's fingerprint in private local state. Launch local programs through `src/io/` with program and arguments kept separate, tested Windows launcher handling, and an explicit environment allowlist. Palepale API credentials are not inherited by default.

Route every MCP call through the normal permission gate with its server, tool, and argument preview. MANUAL asks by default; AUTO retains its existing behavior. PLAN permits only tools explicitly classified as read-only in local policy. Server annotations are hints and cannot make that classification authoritative.

Frame server instructions and tool results as content from that server. They cannot override Palepale's permission rules or grant access to another server.

Keep OAuth credentials in a separate private store, protected with the existing secret-storage facilities and keyed to the server's canonical resource and authorization identity. Use SDK-backed OAuth discovery and PKCE, validate state and issuer, bind tokens to their intended resource, and redact secrets from logs and transcripts. Never forward the Palepale account key to an MCP endpoint. [MCP authorization](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization).

## Changes in this repository

| Area | Change |
| --- | --- |
| `src/commands/`, `data/commands/` | Add `mcp` and `skills` commands and effects; include them in help and completion. |
| `src/tui/` | Add the two cards using existing list, preview, form, and background-job patterns. |
| `src/skills/` | Parse metadata, resolve precedence, build the catalog, and expand explicit invocations. |
| `src/mcp/` | Hold server state and catalog data; adapt server tools to the existing `Tool` interface. |
| `src/io/` | Own SDK transports, subprocesses, filesystem reads, browser callbacks, and credential I/O. |
| `src/tools/mod.rs` | Allow owned tool names/descriptions and registry snapshots. Keep native description templating separate from remote text. |
| `src/agent/run.rs` | Refresh the tool snapshot between model requests. It is currently captured once per whole turn. Pin dispatch to the snapshot that supplied each call. |
| `src/agent/system_prompt.rs`, `src/session/` | Add the bounded skill catalog and source-attributed activation context using existing message parts. Preserve the session format and stable base prompt. |
| `src/settings.rs`, `src/paths.rs`, `src/trust.rs`, `data/` | Add typed settings, private-state locations, trust fingerprints, prompts, and limits. |

Apply configuration and catalog changes between model requests. Disabling a server removes its tools and cancels pending server work; a stale call must never be redirected to a replacement definition. Keep enough source identity in the existing transcript fields to explain what ran when a session is reopened.

## Build order and completion checks

1. **Skills and slash UI.** Build discovery, validation, the picker, preview, `$name` invocation, and explicit/implicit loading. Verify a local fixture skill can guide an edit with the normal permissions.
2. **MCP tools.** Prove the pinned SDK's modern and legacy stdio/HTTP paths with local fixtures, then add server state, configuration, the manager, and real agent tool calls.
3. **Authentication and context limits.** Finish OAuth login/logout, credential refresh, environment-token support, and budget-triggered local tool search. Verify selecting a skill can lead to an authorized MCP tool call in the same conversation.
4. **Verification and docs.** Cover failures and keyboard behavior, run lint, unit tests, and the local end-to-end suite, then document setup. Both commands must be usable before releasing the feature.

Use fixture servers and skills for all automated tests. Cover protocol negotiation, pagination, malformed schemas, duplicate names, disconnects, cancellation, ordered calls, credential redaction, project trust, PLAN restrictions, metadata budgets, skill path resolution, and reloads. Include a timeout after a simulated remote write and assert that Palepale sends no duplicate call.

Run terminal checks at 64×24 and a narrower width. Verify search, empty/error states, long names, sign-in fallback links, draft preservation, and closing a card during connection. Reopen a saved session containing both a skill invocation and an MCP call to check transcript compatibility.

This planning change adds only this document. Implementation starts with `/skills` discovery and the two slash-command entries. Server hosting, plugin marketplaces, automatic package installation, and custom workflow execution engines stay outside the first release.

# MCP and skills

Palepale connects MCP servers and loads skills inside its Rust agent loop. `/mcp` manages connections; `/skills` previews and selects workflows. Setup requests inside a conversation target Palepale's own configuration.

## MCP configuration

Definitions live in `~/.palepale/config.toml` or the project's `.palepale/config.toml`. `PALEPALE_HOME` changes the personal state directory. Each server has either a local command or a remote URL.

```toml
[mcp_servers.local-docs]
command = "npx"
args = ["-y", "your-mcp-package"]
cwd = "."
environment = { LOG_LEVEL = "error" }
env_vars = ["SERVICE_TOKEN"]
read_only_tools = ["search"]

[mcp_servers.remote-docs]
url = "https://example.com/mcp"
transport = "auto"
env_http_headers = { "X-API-Key" = "SERVICE_TOKEN" }
oauth = false
startup_timeout_secs = 20
tool_timeout_secs = 60
```

`cwd` resolves relative to the project. Local servers inherit a small platform environment plus variables explicitly listed in `env_vars`; `environment` adds literal values. Palepale's own credential variables are excluded. For HTTP, `http_headers` stores literal header values; `env_http_headers` reads them from named environment variables. Prefer references for tokens. Transport and session headers are managed by Palepale.

`transport = "auto"` negotiates Streamable HTTP, then tries legacy HTTP+SSE after a connection failure. Select `http` or `sse` to use one transport. Tool calls are never replayed during fallback or session expiration. Matching progress notifications extend a call's idle deadline, up to a total of one hour. Cancellation and timeouts send cancellation notifications; a remote action may already have completed.

Common commands work both outside the TUI and as `/mcp ...` inside it:

```sh
palepale mcp add docs --url https://example.com/mcp
palepale mcp add local --cwd . --env LOG_LEVEL=error -- npx -y your-mcp-package
palepale mcp check docs
palepale mcp auth docs
palepale mcp logout docs
palepale mcp disable docs
palepale mcp enable docs
palepale mcp remove docs
```

Use `--scope project` when adding or removing project definitions. Project servers need trust and a review of each changed definition before they start. Adding a server preserves unrelated settings. Server definitions reload between model requests. Tool-list notifications refresh the existing connection and invalidate outdated tool definitions.

OAuth uses the server's discovery metadata, a browser, PKCE, and a local callback. If the browser does not open, copy the link from the sign-in card. Credentials use Palepale's private credential storage. Optional fields are `client_id`, `client_secret_env_var`, `scopes`, and `oauth_callback_port`. `oauth = false` disables OAuth. `bearer_token_env_var` supplies a bearer token instead.

The model sees connected servers and their capabilities. Small tool catalogs load directly; larger catalogs expose `mcp_search` to select tool definitions. Servers that advertise resources or prompts expose these native tools:

| Tool | Actions |
| --- | --- |
| `mcp_resources` | List resources, list templates, read a URI |
| `mcp_prompts` | List prompts, get one with string arguments |

The familiar `list_mcp_resources`, `list_mcp_resource_templates`, and `read_mcp_resource` tools are also available. Lists accept an optional server; reads require a server and URI. Connected prompts appear in the slash menu as `/server:prompt`. Arguments follow the prompt's declared order; quote arguments containing spaces. The prompt is fetched through the normal permission gate before the model turn begins.

Text, structured tool results, embedded text resources, resource links, and text prompt messages reach the model. Oversized output is shortened and saved to a local spill file. PNG, JPEG, GIF, WebP, WAV/MP3, and PDF content arrives as typed attachments. Each attachment is capped at 16 MiB; invalid or unsupported content receives an explicit message. Attachments survive session replay and follow the completed tool-result batch in the model request. The selected model and backend must support the attachment type.

MANUAL follows `mcp` permission rules. PLAN also requires tool names in `read_only_tools`; resource and prompt reads use `mcp` rules. Server annotations do not grant permissions. AUTO accepts actions automatically, including actions denied by configured rules.

## Skills

Skills follow the [Agent Skills format](https://agentskills.io/specification). Each folder has `SKILL.md` with YAML `name` and `description`, followed by instructions. The name must match the folder and use lowercase letters, digits, and single hyphens.

Discovery checks these folders, including grouped subfolders:

| Scope | Locations |
| --- | --- |
| Project, up to repository root | `.palepale/skills`, `.agents/skills`, `.claude/skills`, `.opencode/skills`, `.opencode/skill` |
| Personal | `~/.palepale/skills`, `~/.agents/skills`, `~/.claude/skills`, `~/.config/opencode/skills`, `~/.config/opencode/skill` |
| Registered | `[skills] paths = ["/path/to/skills"]` |
| Remote index | `[skills] urls = ["https://example.com/skills/"]` |

`PALEPALE_HOME/skills` is also discovered when the state directory is overridden. Use `skills add <folder-or-url>` to register a source and `skills check <name>` to verify its instructions. `skills remove` unregisters a source while preserving its files. `skills enable`, `disable`, and `refresh` also work from the CLI and slash commands.

Remote sources serve `index.json` with a `skills` array. Each entry contains `name`, `files` (including `SKILL.md`), and an optional `version`. Listed files download into separate cache directories per source and version. The manifest switches after every required file finishes; failed downloads keep the previous complete set. Unchanged versions reuse their files. Index checks share a one-minute cache; `skills refresh` bypasses it. Offline runs use cached files. Downloads stay on the source origin, enforce file and total-byte limits, and reject absolute or traversing file paths. Raw Markdown and repository URLs should be installed into local skill folders instead.

Canonical paths prevent duplicates and link cycles. Discovery is capped at 512 definitions and eight folder levels. Trusted project skills take precedence over personal skills with the same name; other entries remain visible with their state. Disabled definitions reserve their name. Invalid and untrusted definitions cannot be invoked.

The model receives a bounded name-and-description catalog, then calls `skill` with an exact name. The result contains the instructions, base directory, and up to ten supporting filenames. Supporting file contents load through ordinary file tools when needed. `$skill-name task` and `/skill-name task` select a workflow explicitly. Built-in slash commands take precedence. Previews and invocation do not run scripts.

Metadata controls invocation:

| Field | Effect |
| --- | --- |
| `disable-model-invocation: true` | Require explicit selection |
| `user-invocable: false` | Allow automatic selection only |
| `metadata: { opencode/autoinvoke: false }` | Require explicit selection |
| `metadata: { palepale/autoinvoke: false }` | Require explicit selection |
| `slash: false`, or `metadata: { opencode/slash: false }` | Hide the slash alias; explicit `$name` selection remains available |
| `license`, `compatibility`, other metadata | Accepted as descriptive information |
| `context`, `agent`, `hooks` | Rejected as unsupported execution fields |

Skill loading checks `skill` name permissions and ordinary file-read permissions. In MANUAL or PLAN, a rule such as the following hides matching names from the automatic catalog and denies loading:

```toml
[[permission]]
permission = "skill"
pattern = "private-*"
action = "deny"
```

AUTO retains its normal behavior of accepting actions. Skill instructions and metadata do not change the active mode or grant permissions. New files appear between model requests; no restart is required.

## OpenCode comparison

Behavior was checked against [OpenCode v1.19.0, commit 733b2c6](https://github.com/anomalyco/opencode/tree/733b2c627e5aec39cb33cc22373bdce33053ef2c), using its [MCP implementation](https://github.com/anomalyco/opencode/blob/733b2c627e5aec39cb33cc22373bdce33053ef2c/packages/opencode/src/mcp/index.ts), [catalog code](https://github.com/anomalyco/opencode/blob/733b2c627e5aec39cb33cc22373bdce33053ef2c/packages/opencode/src/mcp/catalog.ts), and [skill tool](https://github.com/anomalyco/opencode/blob/733b2c627e5aec39cb33cc22373bdce33053ef2c/packages/opencode/src/tool/skill.ts). Palepale implements these behaviors in Rust with `rmcp`; it does not embed OpenCode's TypeScript runtime.

| Behavior | Palepale coverage |
| --- | --- |
| Local stdio, Streamable HTTP, legacy SSE | Implemented |
| Local environment/cwd, HTTP headers, OAuth configuration | Implemented with Palepale TOML fields |
| Paginated tools, resources, templates, prompts | Implemented with item, page, and repeated-cursor limits |
| Tool-list changes | Refresh the running connection |
| Native skill loading and supporting-file sample | Implemented through `skill` |
| Compatible local skill folders and automatic invocation controls | Implemented |
| Remote skill-index distribution | Implemented with versioned files and complete-set publication |
| Skill slash controls and generated MCP prompt slash commands | Implemented; prompts use `/server:prompt` |
| Image/PDF delivery and session replay | Implemented; WAV/MP3 attachments are also supported |
| Legacy SSE reconnection | Resume the receive stream with Last-Event-ID; never repeat POSTed tool calls |

This comparison covers MCP and Agent Skills workflows in the pinned revision. Palepale retains its TOML configuration, trust reviews, name validation, permission modes, bounded catalogs, and compact terminal UI. Server-initiated sampling and interactive requests remain disabled; these are separate protocol features and are not advertised by this client.

# Commands and keyboard shortcuts

Type `/` in a session to open the command menu. Use Up and Down to select a command,
Tab to complete it, Enter to run it, and Escape to close the menu. A message that
starts with `//` sends a literal leading slash to the model.

## Slash commands

| Command | Action |
| --- | --- |
| `/help` | List commands available in the session. |
| `/models [--refresh]` | Choose a model and save it as the default. Alias: `/model`. |
| `/mode [manual\|plan\|auto]` | Set the permission mode. With no argument, cycle modes. |
| `/status` | Show session details, balance, and usage. |
| `/new` | Start a fresh conversation and keep the previous session file. Alias: `/clear`. |
| `/sessions [id]` | Pick a saved conversation or resume one by ID. Alias: `/resume`. |
| `/undo` | Take back the last prompt and restore its file checkpoint. |
| `/retry` | Undo the last prompt and send it again. This can incur another charge. |
| `/compact [focus]` | Summarize earlier turns while retaining recent context. |
| `/prompt [id]` | List local system prompts, or select one and save it as the default. |
| `/init` | Ask the model to study the project and write `AGENTS.md` for the next conversation. |
| `/copy` | Copy the last reply as Markdown. |
| `/login [--key <inference-key> \| --status]` | Pair this device, enter a key, or check login status. |
| `/logout` | Remove the stored inference key. The owner credential stays saved. |
| `/keys [list]` | List inference keys using the separate owner credential. |
| `/keys create <label> --use` | Create and save a new inference key for this device. |
| `/keys revoke <ref> --yes` | Revoke the specified inference key. |
| `/mcp` | Open the MCP server manager. |
| `/skills` | Open the skill browser. |
| `/forums` | Manage forums and their login state. Alias: `/forum`. |
| `/godmode [profile\|custom [apply]\|off\|status]` | Choose a server profile or test your own system prompt. |
| `/quit` | Leave the session. Alias: `/exit`. |

`/mcp` and `/skills` also accept subcommands for adding, checking, enabling, disabling,
and removing sources. See [MCP servers and skills](mcp-and-skills.md) for syntax,
discovery, imports, and authentication. Connected MCP prompts appear as
`/server:prompt`; quote arguments that contain spaces.

For forums, use `/forums list`, `/forums add <url or name>`, `/forums login <forum>`,
`/forums logout <forum>`, `/forums remove <forum>`, or
`/forums search <forum> <words>`.

Account and key commands use the gateway configured for this session. Inference
keys start with `pk_`; owner credentials start with `po_`. Account numbers and
owner credentials cannot authorize chat. See [account setup](bun-gateway.md) for
pairing, key recovery, and owner exports.

## Keyboard shortcuts

| Key | Action |
| --- | --- |
| Enter | Send a message, or queue a follow-up while a turn runs. |
| Shift+Enter or Ctrl+J | Insert a newline. |
| Tab | Complete a slash command when idle; steer a running turn after its current tool calls. |
| Shift+Tab | Cycle permission modes. |
| Escape | Close the active card or menu; otherwise stop the current turn. |
| Ctrl+C | Clear the draft; otherwise interrupt, or press again to quit. |
| Ctrl+D | Quit. |
| Ctrl+T | Expand or collapse all thinking blocks. |
| Page Up / Page Down | Scroll the transcript. The mouse wheel also scrolls. |
| Up / Down | Move through logical input lines; browse prompt history at the edges. |
| Home / End | Move to the beginning or end of the input line. |
| Ctrl+Left / Ctrl+Right | Move by word. |
| Ctrl+W | Delete the previous word. |
| Ctrl+U / Ctrl+K | Delete to the beginning or end of the input line. |

Click a tool or thinking block to expand it. Expanded command cards show multi-line
input and complete output; task cards include child tool calls.

## Permission modes

New sessions start in AUTO. AUTO runs tools without asking and overrides deny rules.
MANUAL asks before edits, fetches, and commands that do more than inspect. PLAN
permits investigation and checks, with edits limited to the plan file.

## Models and sessions

In `/models`, press Tab to cycle through All, Uncensored and Cheapest. Uncensored
uses the catalog's badge. Cheapest sorts by the combined price of 1M uncached input
and 1M output tokens; the table shows each rate separately. Only available models
appear. Arrow keys move through the list and Enter selects a model.

Choosing a model with `/models` saves it for future sessions. `--model <id>` overrides
the choice for one launch. If the chosen model lists supported reasoning efforts,
a second picker lets you choose one. Escape keeps the current effort, or the server
default when switching models. The choice is saved with the conversation and restored
when you resume. Models without configurable effort skip this step.

Run `palepale --continue` to reopen the latest conversation for the project, or
`palepale --resume <id>` to open a specific one. Local system prompts and server
profiles have separate selections.

Undo checkpoints cover files inside a Git work tree and respect its ignore rules.
Files edited after a completed turn are kept. If a turn was interrupted before its
final checkpoint, undo rewinds only the conversation. Concurrent sessions cannot
share an active snapshot index.

Long conversations compact automatically near the context limit. Original messages
stay in the session file. See [sessions and recovery](reliable-sessions.md) for file
locks, interrupted tools, and instruction scopes.

## Working with tools

The opening turn includes a bounded directory listing and Git status. A successful
command starting with one literal `cd` sets the directory for the next `bash` call.
Shell variables, substitutions, and later directory changes stay local to the command.

Set `verify = "cargo test"` in user settings or trusted project settings to run a
check after file changes. A failed check returns its output to the agent for a bounded
repair attempt. Verification is off by default and follows command permissions.

The `task` tool can delegate to an `explore` agent with read-only tools or a `general`
agent with the parent's tool set and permission mode. Child tasks have bounded depth,
concurrency, and turns. Stopping the parent cancels its children.

The optional `decompile` tool lists functions and requests pseudocode from a local
`kuna` executable. It follows file-read permissions and reports a missing executable
without installing anything. Disassembly through `bash` remains available.

## Server profiles

On the Palepale Bun gateway, `/godmode` opens the profile picker. The server keeps
profile instructions private; the CLI receives the profile ID, name, and description.
Use `/godmode off` to clear the selection and `/godmode status` to inspect it.

Choose **Custom** to open your prompt in Notepad on Windows (the desktop text
editor on other platforms). Save the file, return to Palepale, and select
**Use saved prompt**. You can also open it with `/godmode custom` and apply it
with `/godmode custom apply`.

The UTF-8 prompt is saved in `~/.palepale/prompts/godmode-custom.md`, up to 32 KiB.
It replaces the built-in base prompt for this conversation and turns off the server
profile. Tool setup and workspace context remain. `/godmode off` restores the
built-in prompt; choosing a server profile also clears the custom selection.
Your default for new conversations stays unchanged. Leading `[Six]` persona tags
are hidden in replies while a server profile is active.

These screenshots show the legacy profile flow:

| Before selecting a profile | After selecting a profile |
| --- | --- |
| ![Session](images/palepale-session.png) | ![Selected profile](images/palepale-godmode.png) |

## Commands outside a session

Run these from your shell rather than the slash menu:

| Command | Action |
| --- | --- |
| `palepale signup` | Create an account and save separate owner and inference credentials. |
| `palepale keys export-owner <file> --yes` | Export the owner credential to a private plaintext file. |
| `palepale install` | Install for the current user and add Palepale to PATH. |
| `palepale uninstall` | Remove the installation; retain keys and sessions. |
| `palepale update` | Check for and install a newer verified release. |

Commands supported in both places, such as `login`, `models`, `sessions`, `mcp`, and
`skills`, also accept the `palepale <command>` form. `palepale help` lists shell
commands; `/help` lists session commands.

# Sessions, instructions, and local prompts

List saved conversations with `palepale sessions`. Open the latest conversation in
the current project with `palepale --continue`, or choose one with
`palepale --resume <id>`. Inside the terminal, `/sessions` opens the picker;
`/sessions <id>` opens a specific conversation. Resume opens the saved history and
waits for a new request. Headless use still needs a prompt:

```powershell
palepale --continue -p "continue fixing the test"
palepale --resume <id> --json "check the result"
```

New records retain the model, mode, profile, and local prompt choice. `--model`
and `--mode` override the saved values for that launch. The current project
configuration, permission rules, and trust decision are loaded again. File-read
state and temporary tool approvals are not restored. Older sessions without
saved settings use the current configuration; their original messages remain
readable. Reasoning effort remains server-managed; legacy effort metadata is
retained in the session without sending an override.

Sessions are private JSONL files under the configured Palepale home. Each has an
OS writer lock, so a second process can list it but cannot resume it while it is
open elsewhere. A crash releases the lock. Recovery preserves an incomplete final
record in a `.partial` sidecar before repairing the JSONL boundary. Corrupt complete
records, duplicate IDs, broken parent chains, and foreign project headers stop
resume with an error.

Completed tools are never replayed by resume. A tool call whose result was not
saved receives an explicit unknown-outcome record. Repeating the same tool and
arguments requires confirmation even in AUTO; headless runs cannot confirm it.
Check the current state before deciding whether that action should run again.
The transcript cannot determine whether an interrupted shell or remote service
finished its work.

The reader accepts sessions up to 64 MiB. The list considers the newest 1,000
session filenames and shows up to 100, ordered by recorded activity. A specific
older ID can still be opened directly. Active heads and settings are additive
records; turn undo, redo, and compaction remain separate roadmap items.

## Instruction scopes

Before each model request, Palepale refreshes the instructions under
`PALEPALE_HOME` (normally `~/.palepale`) and from the Git repository root toward
the working directory. Without a Git root, discovery starts at the launch
directory. `AGENTS.md` takes precedence at each scope; `CLAUDE.md` is used only
when `AGENTS.md` is absent. An empty or unreadable `AGENTS.md` does not activate
the fallback.

Reading or searching a file adds its directory ancestors to the active scope.
Each instruction block names its source and subtree. Sibling rules apply only
to their own subtree. Changed and deleted files replace the previous instruction
snapshot on the next request. Project instruction links outside the repository
are excluded. Discovery reads at most 64 scopes and 32 KiB of instruction text;
truncation is shown in the model context. Instructions do not change tool
permissions, project trust, or the selected mode.

## Choose the default system prompt

`/prompt` lists local system prompts. `/prompt <id>` selects one for this
conversation and saves the ID as the default in the user configuration.
`palepale prompt <id>` saves a default from the command line. The built-in IDs
are `default` and `chat`.

For a custom prompt, create a UTF-8 Markdown file under
`~/.palepale/prompts/<id>.md`. A trusted project can provide
`.palepale/prompts/<id>.md`; its matching ID takes precedence. `PALEPALE_HOME`
also relocates the personal prompt directory. Each file is limited to 32 KiB.
The selected file replaces the built-in base prompt; Palepale still adds the
environment, setup guidance, and scoped project instructions. Prompt files are
read as text and never executed.

The local prompt choice is saved independently from the `/godmode` server
profile. Changing either leaves the other choice intact. Profile synchronization
is deferred. If a saved local prompt disappears or becomes unreadable, sending
stops so another prompt can be chosen explicitly.

## Model compatibility

The model list retains optional `input_modalities` and `capability_source`
metadata. Plain text continues to work with older catalogs. Unknown attachment
support is distinct from an explicit unsupported type. Image, audio, PDF, and
generic file payloads require declared support; nothing is silently removed from
the transcript when changing models.

The shared request check validates streaming support, input types, reasoning
metadata, and context/output limits before connecting to inference. The CLI uses
the model's advertised output limit. Input budgeting estimates serialized
request size at three bytes per token plus per-message overhead, including
instructions and tool schemas. This estimate is not a tokenizer or billing
measurement; the gateway remains authoritative. Automatic compaction is still
planned under issue #5. A rejected terminal request keeps its text in the
composer.

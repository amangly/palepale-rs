# @ mentions

Status: implemented. See [Commands and shortcuts](commands.md#references) for usage.

Implementation uses recorded native read calls for file context, rather than
appending file bodies as user text. This preserves permission evidence and scoped
instruction discovery in the existing session format. Read failures stop before
inference. Agent mentions use recorded task calls, followed by the normal parent
loop and verification. External reference roots and media remain out of scope.

Typing `@` in a prompt opens a picker for project files, folders, and, in a second batch, available subagents. Selecting a file includes its contents with the next message. Selecting a folder includes a bounded directory listing. Selecting an agent delegates the prompt through the existing task system.

## Reference behavior

Checked on 2026-11-02:

- [OpenCode TUI](https://opencode.ai/docs/tui/#file-references): `@` fuzzy-searches project files and adds selected file contents to the conversation. Configured reference roots can also appear in the picker.
- [OpenCode agents](https://opencode.ai/docs/agents/#subagents): users can invoke subagents with `@` mentions.

The behavior below is the proposed Palepale design. External reference roots, remote downloads, and image attachments are outside the first implementation.

## Prompt behavior

Examples:

```text
Explain @src/main.rs
Compare @src/tools/read.rs with @src/tools/grep.rs
Review @"docs/design notes.md"
Explain the layout of @src/tui/
@explore trace how permissions reach @src/tools/read.rs
```

- Open the picker for an unescaped `@` at the start of a token, including in the middle of a multiline prompt. Email addresses and URLs remain ordinary text. Ignore inline and fenced code.
- Filter as the user types. Rank exact paths, basename prefixes, then fuzzy matches; break ties by path. Display project-relative paths and a file, folder, or agent label.
- Up/Down select; Tab or Enter inserts the selection without submitting. Escape dismisses the picker. Once closed, Enter submits normally and Tab retains its existing steering behavior during a run.
- Replace only the token at the cursor. Preserve surrounding text, Unicode, history, pasted text, and multiline editing. Quote paths containing spaces. Accept both path separators on Windows and display `/` consistently.
- Support manually typed mentions as well as picker selections. `@@name` sends a literal `@name`. A quoted mention supports filenames containing spaces or punctuation; an unquoted mention ends at whitespace or sentence punctuation.
- Resolve files relative to the active project. An explicit `@./explore` means a file if its name conflicts with an agent. Once agent mentions are enabled, bare `@explore` selects the agent.
- Resolve file contents when the message is consumed, including queued and steering messages. Show missing, denied, binary, and truncated-file results explicitly. Do not silently substitute a similarly named file.
- Deduplicate identical canonical paths within one message. Folder mentions list entries only; they do not recursively attach file contents.

## Batch 1: files and folders

1. Add a pure mention parser and matcher in `src/text/mentions.rs`. Return token spans and decoded paths so completion and submission use the same syntax. Keep cursor offsets on UTF-8 boundaries.
2. Extend `src/tui/editor.rs` with cursor position and range replacement operations. Add `src/tui/mentions.rs` for picker state and rendering, following the existing command palette conventions. Wire it into `src/tui/app.rs` without changing slash-command dispatch or prompt-card priority.
3. Add bounded path discovery through `src/io/search.rs`. Reuse the existing ignore walker. Respect ignore files, omit `.git`, do not follow directory symlinks, and include untracked non-ignored files. Search names only, not contents. Perform discovery off the input/render path; discard stale results when the query or project changes. Bound both traversal and results, and report when discovery is incomplete.
4. Add `src/agent/mentions.rs` to resolve parsed file mentions at the user-message boundary. Use the existing read tool's checks and native permission gate. Share bounded file reading/formatting with `src/tools/read.rs` where needed; do not call `Read::run` without its checks. Keep filesystem access under `src/io/`.
5. Apply expansion to initial, queued, and steering messages in `src/agent/run.rs`. Keep the original prompt as a text part and append labeled context using existing text parts. File bodies are source content, never system reminders. Use the existing session representation; replay stored content without reopening or expanding old messages.
6. Preserve read tracking, project instruction discovery, cancellation, PLAN restrictions, and external-path checks. Revalidate resolved paths at consumption time. Mentioning a path does not grant a lasting permission exception.
7. Put picker labels, context framing, errors, and help text in `data/`; put discovery, file-count, and aggregate context limits in `data/limits.toml`. Enforce input byte limits before loading whole files, in addition to output line limits. Document syntax in `docs/commands.md`.

Use the existing `ignore` dependency and terminal renderer. Start without a filesystem watcher or persistent index. Refresh discovery on picker open and project changes, with cancellation for superseded work.

## Batch 2: agents

1. Expose a read-only agent catalog from the definitions currently loaded by `src/tools/task.rs`. Use IDs and descriptions from `data/agents/*.toml` for suggestions and dispatch; do not duplicate agent names in the picker.
2. Add available agents to the same picker. Allow one agent mention per submitted prompt initially; report multiple agent selections before starting work. Resolve file mentions first and include their bounded context in the child task.
3. Dispatch the selected agent through the existing checked task execution path, with normal child events, cancellation, permission mode, model, spending budget, depth limits, and session recording. Record a valid task call/result pair rather than inserting an orphan tool result or merely asking the model to delegate.
4. Keep the current primary mode and profile. In PLAN mode, an agent mention must not grant tools that PLAN forbids. If the task tool or selected agent is unavailable, show the reason instead of silently falling back.
5. Preserve existing finish and verification behavior when the child returns. Agent selection must not skip verification after a child changes files.

Skill invocation keeps its existing `$skill-name` syntax. Agent mentions do not change that contract.

## Verification

- Parser/editor unit tests: cursor in the middle, multiple mentions, Unicode, quoted paths, Windows separators, punctuation, escape syntax, email/URL/code literals, paste, history, and path/agent collisions.
- Local filesystem fixtures: ignored and untracked paths, hidden paths, symlink escapes, missing or renamed files, duplicate canonical paths, binary files, large files, and bounded folder listings.
- Local agent fixtures: allow/ask/deny reads, cancellation during expansion, no request after cancellation, initial/queued/steering parity, instruction discovery, stored-context replay, and unchanged legacy sessions.
- Agent fixtures: selected task runs once, valid call/result history, disabled task, PLAN restrictions, inherited budget, depth limits, child cancellation, multiple-agent rejection, and verification after child edits.
- PTY tests: picker opens, filters, accepts, and dismisses; Enter selects without sending; Tab resumes steering after dismissal; slash commands and permission cards still work. Large-tree discovery must leave typing responsive and stale results must never replace newer results.
- Run the repository lint and unit commands, then local fixture e2e once per completed behavior batch. Check terminal behavior on Windows, Linux, and Apple Silicon through the platform CI jobs. No live inference is needed.

Implement and verify batch 1 before adding batch 2. Release only after the implementation and required checks are complete.

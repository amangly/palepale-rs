# Agent improvements

The agent supports per-file read ranges, multi-root search, full plan review,
private decompiler caching and a guard against repeated tool calls that make no progress.

- Read targets can mix paths with per-file offset, limit and depth options.
- Search checks every root, shares one result limit and returns overlapping files once.
- Plans appear whole in the scrollable conversation. Edits made while the approval
  card is open reach the model with the review response.
- Successful decompiler answers are cached by program, binary and adjacent PDB
  metadata, arguments and working directory. Failures and empty answers are not cached.
- Repeated calls count even when interleaved with other tools. Changed output resets
  the sequence so useful polling and re-reading can continue.
- The finish tool can complete a successful batch without a separate closing request.
  Configured verification, queued messages, steering and cancellation still apply.
  Its summary is displayed and saved only after the run succeeds. PLAN still uses plan_exit.

The cache uses path, size and modification time rather than hashing entire binaries.
External PDB locations and cache pruning remain known limits. Session files retain
their existing format; plan review events add a file field.

Model-routing presets and opening an external editor from the approval card remain
separate features. The configured turn bound remains in place.

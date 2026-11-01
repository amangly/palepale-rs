Search file contents with a regular expression (ripgrep syntax). Respects .gitignore.

- Returns `path:line: text` for each match, at most {max_results}. `context` adds that many lines before and after each match (at most {grep_context_max}), shown as `path-line- text`, with `--` between groups.
- `output: "files"` lists only the files that match; `output: "count"` gives how many lines match in each file.
- Narrow with `glob` (e.g. `*.ts`) or `path`; use `ignore_case` for case-insensitive search.
- `path` also accepts a list of up to {grep_batch_max} files or directories. The result cap applies to the whole search, and overlapping roots return each file once. Permissions are checked for every root.
- Use this, not `grep` or `rg` in the shell: it needs no permission and its paths and line numbers feed straight into `read` and `edit`.

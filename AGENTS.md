# Repository rules

- Keep changes small and read the relevant code first.
- Use the no-ai-slop skill for documentation and public copy when available.
- Put prompts, tool schemas, text, limits, and configuration in `data/`. Extend lists and shared interfaces instead of hard-coding cases.
- Give each module one concern. Share repeated logic; avoid abstractions for work nobody requested.
- Only `src/io/` touches files, network, processes, terminals, or clocks. Other modules use those interfaces; `clippy.toml` enforces this boundary.
- Preserve the native agent loop, streaming, permissions, and session format.
- Keep comments brief and explain why. Tag a known shortcut `CEILING:` with its constraint and upgrade path.
- Retain license files and copyright notices. Leave vendored `crates/cffetch/` and local `reference/` unchanged.
- Work on `main`. Push to `prod` only after an explicit release request.
- Run lint and unit tests before pushing. Run local end-to-end tests once per critical behavior-change batch; CI runs them on main pushes and releases.
- End-to-end tests use local fixtures. Live smoke tests require an explicit request and spend real money.
- Regenerate golden files and snapshots only for deliberate changes, with the reason in the commit.
- Never commit credentials, local account state, or private verification artifacts.

Read [README.md](README.md) for usage, [CONTRIBUTING.md](CONTRIBUTING.md) for development, and [docs/releases.md](docs/releases.md) for publishing.

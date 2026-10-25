# Contributing

## Build and check

Use the Rust version pinned in `rust-toolchain.toml`.

On Linux/macOS, install CMake, libclang, Perl, make, and a C/C++ compiler, then run:

```sh
make build
make lint
make test
```

On Windows, use PowerShell 7 with MSVC C++ Build Tools, CMake, NASM, LLVM/libclang, and Perl. Use a short build path:

```powershell
.\scripts\build.ps1 -Task build -TargetDirectory C:\build\palepale
.\scripts\build.ps1 -Task lint -TargetDirectory C:\build\palepale
.\scripts\build.ps1 -Task test -TargetDirectory C:\build\palepale
```

The Windows binary is `C:\build\palepale\release\palepale.exe`.

For changes to auth, API behavior, tools, or terminal interaction, run `make e2e` or the Windows wrapper with `-Task e2e`. These tests use local servers and real pseudo-terminals. `make smoke` uses a paid API and runs only with explicit approval and configured `PALEPALE_SMOKE_URL`, `PALEPALE_SMOKE_KEY`, and `PALEPALE_SMOKE_MODEL`.

## Layout

| Folder | Contents |
| --- | --- |
| `src/agent/` | Conversation loop and transcript |
| `src/api/` | Models, requests, and streamed responses |
| `src/tui/` | Terminal interface |
| `src/tools/` | Local and remote tool implementations |
| `src/permission/` | Permission checks |
| `src/io/` | External I/O interfaces |
| `data/` | Runtime prompts, tool descriptions, schemas, and settings |
| `tests/` | Fixtures and integration tests |
| `bench/` | Pinned tasks, patches, and benchmark settings |
| `crates/cffetch/` | Vendored web-fetch dependency |
| `scripts/` | Windows build wrapper |
| `.github/` | CI and release workflows |

Markdown in `data/` is runtime input compiled into the CLI. Keep those files with their tool schemas and prompt configuration.

The opening logo comes from `data/palepale-logo.svg`. After changing it, run `node scripts/render-logo.cjs` with `sharp` available to Node to regenerate the RGBA pixels and terminal cell fixture. Dimensions and block glyphs live in `data/logo.toml`. The CLI embeds the pixels; it does not need an SVG renderer at runtime.

## Benchmarks

`make e2e` also checks the benchmark runner against local fixtures. `make battery-setup` downloads pinned repositories and runs their setup commands. A live run needs explicit approval and the same three `PALEPALE_SMOKE_*` variables as smoke tests. Use `make battery TASKS=calc-fix REPEATS=1` for one task.

Runs save sessions, tool permissions, diffs, and check results under `bench/runs/`. Run `palepale battery report <run-directory>` to generate a report, or add `--against <earlier-run-directory>` to compare runs. Limits in `bench/battery.toml` stop tasks after observed usage; requests already in flight can exceed the threshold. Keep results and cached repositories out of Git.

The full external corpus also needs Git, Python 3, and Node 18.12 or later with Corepack. The pinned pnpm used by the UFO tasks refuses older Node versions. Its setup uses a Unix shell and `ln -s`; run it on Linux or macOS. `make battery-setup` installs dependencies and checks that untouched repositories pass and planted bugs fail, without calling a model.

The battery also has `mcp` and `re` suites. MCP tasks run against local simulated servers with
schema checks, state, and call records. The `re` suite uses stripped Linux x64 binaries whose
sources live under `bench/benchmarks/`; run those tasks on Linux. Select a suite with
`make battery SUITE=mcp`. Live runs still require explicit approval and the configured smoke
credentials. `BATTERY_VERIFY=1` measures the optional verification step using each repository's
test command.

Reports include grounding, tool batches, simulator errors, and server preconditions. The report
snapshot includes a synthetic MCP run to exercise those sections; regenerate it only when the
report or fixture deliberately changes.

## Configuration

User settings live in `~/.palepale/config.toml`; a project can add `.palepale/config.toml`. Settings that increase project permissions require trust. Sessions are saved under the CLI home after the first backend turn starts.

| Variable | Purpose |
| --- | --- |
| `PALEPALE_API_URL` | Override the API origin |
| `PALEPALE_CHAT_URL` | Override the chat endpoint |
| `PALEPALE_API_KEY` | Supply an inference credential |
| `PALEPALE_OWNER_CREDENTIAL` | Supply a separate owner credential for key management |
| `PALEPALE_HOME` | Change the state folder |
| `PALEPALE_NO_AUTO_LOGIN=1` | Skip startup pairing |
| `PALEPALE_NO_BROWSER=1` | Show the pairing link without opening it |
| `PALEPALE_NO_USER_SKILLS=1` | Skip personal skills for isolated runs |
| `PALEPALE_OFFLINE=1` | Disable network access |
| `PALEPALE_NO_UPDATE=1` | Disable automatic update checks |
| `PALEPALE_NO_ANIMATION=1` | Skip the Matrix opening animation |

Stored device keys are bound to their API origin. Windows protects them with DPAPI; other platforms use owner-only file permissions. Headless `--print` and `--json` runs require an existing key and do not open the browser. Keep credentials in local state or environment variables.

An alternate backend must support `GET /v1/models` and streamed `POST /v1/chat/completions`. Browser pairing also requires the gateway's pairing and account endpoints. Run `palepale models --refresh` after changing the catalog.

Read [AGENTS.md](AGENTS.md) for code rules and [docs/releases.md](docs/releases.md) for publishing.

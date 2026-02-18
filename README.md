# forge-loop-cli

Standalone Rust CLI for autonomous coding loops.

## Prerequisites

- Rust stable toolchain
- Codex CLI installed and available in `PATH`

Check your Codex installation:

```bash
codex --version
```

## Why

`forge` runs a spec-driven workflow:

1. Collect requirements in phases.
2. Write plan/spec artifacts.
3. Execute implementation loop with guarded completion.

## Commands

- `forge` (interactive assistant mode: asks SDD questions, writes plan/specs, then runs loop)
- `forge run [--full-access] [--thinking off|summary|raw] [--max-loops N] [--timeout-minutes N]`
- `forge analyze --modified-only`
- `forge status`
- `forge monitor [--refresh-ms N] [--stall-threshold-secs N]`
- `forge sdd list`
- `forge sdd load <id>`

## Assistant mode flow

Running `forge` with no subcommand starts an interview in phases:

1. Intent
2. Scope
3. Constraints
4. Acceptance criteria
5. Given/When/Then scenarios
6. Testing strategy
7. Thinking mode (`off`, `summary`, `raw`)

It generates:

- `.forge/sdds/<id>/` (snapshoted SDD files)
- `.forge/plan.md`
- `docs/specs/session/spec.md`
- `docs/specs/session/acceptance.md`
- `docs/specs/session/scenarios.md`

Then it executes `forge run` in loop mode automatically.

## Reuse Existing SDDs

List saved SDD snapshots:

```bash
forge --cwd /path/to/project sdd list
```

Load a previous SDD snapshot as the active plan/spec:

```bash
forge --cwd /path/to/project sdd load <id>
```

## Runtime files

The runtime state is stored in `.forge/`:

- `status.json`
- `progress.json`
- `live.log`
- `.session_id`
- `.call_count`
- `.last_reset`
- `.circuit_breaker_state`
- `.circuit_breaker_history`
- `.runner_pid`

## Live visibility (Ralph-style)

`forge monitor` now shows, in real time:

- current loop
- run timer and current command timer (`HH:MM:SS`)
- current Codex activity extracted from `.forge/live.log`
- stalled detection based on heartbeat (`last_heartbeat_at_epoch`)
- alert when heartbeat is stale (red status panel border and alert line)
- alert when runner process is missing but status says `running` (stale status)
- suppressed noise for repeated `codex_core::state_db record_discrepancy` warnings

`forge status` prints `run_timer` and `command_timer`.

`forge run` updates heartbeat (`last_heartbeat_at_epoch`) from real stream events during loop execution.
If Codex emits no output for 120s, Forge triggers a no-output watchdog and kills that iteration to avoid permanent hangs.

### Monitor screenshot

![Forge monitor live view](docs/assets/forge-monitor-live.png)

## Config precedence

`flags > environment > .forgerc > defaults`

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Release

Publishing runs automatically when pushing a tag matching `v*` (for example `v0.1.0`).

Artifacts published to GitHub Releases:

- `forge-<tag>-x86_64-unknown-linux-gnu.tar.gz`
- `forge-<tag>-x86_64-apple-darwin.tar.gz`
- `forge-<tag>-aarch64-apple-darwin.tar.gz`
- matching `.sha256` files

## Quick Start

```bash
cargo run -p forge
```

This starts the interactive SDD assistant, generates specs/plan files, and then executes the loop with Codex CLI.

To run against a different folder without changing directories:

```bash
cargo run -p forge -- --cwd /absolute/path/to/project
```

To pass native `codex` global flags through `forge run`:

```bash
forge --cwd /absolute/path/to/project run \
  --codex-arg=--sandbox \
  --codex-arg=danger-full-access
```

Shortcut for full sandbox permissions:

```bash
forge --cwd /absolute/path/to/project run --full-access
```

Control thinking verbosity presets (mapped to Codex `--config` flags):

```bash
forge --cwd /absolute/path/to/project run --thinking off
forge --cwd /absolute/path/to/project run --thinking summary
forge --cwd /absolute/path/to/project run --thinking raw
```

Monitor with a custom stall threshold:

```bash
forge --cwd /absolute/path/to/project monitor --stall-threshold-secs 20
```

You can also persist these args in `.forgerc`:

```toml
codex_pre_args = ["--sandbox", "danger-full-access"]
thinking_mode = "summary" # off | summary | raw
```

To force a new clean loop session (ignore previous runtime/session artifacts):

```bash
forge --cwd /absolute/path/to/project run --fresh
```

`--fresh` clears runtime state files in `.forge/` and adds `--ephemeral` to Codex execution to avoid reusing old sessions.

## Analyze modified files

Run a read-only risk analysis over modified files:

```bash
forge --cwd /absolute/path/to/project analyze --modified-only
```

For large diffs, split analysis into chunks:

```bash
forge --cwd /absolute/path/to/project analyze --modified-only --chunk-size 25
```

`forge analyze` persists output to:

- `.forge/analyze/latest.json`
- `.forge/analyze/history/<epoch>.json`

and prints per-chunk progress to stderr while running.

To resume only the final synthesis from the previously persisted chunk reports:

```bash
forge --cwd /absolute/path/to/project analyze --resume-latest-report
```

## Doctor

Check environment and runtime readiness:

```bash
forge --cwd /absolute/path/to/project doctor --json
```

Attempt automatic safe fixes:

```bash
forge --cwd /absolute/path/to/project doctor --fix
```

Fail if any operational warning remains:

```bash
forge --cwd /absolute/path/to/project doctor --strict
```

## License

MIT. See `LICENSE`.

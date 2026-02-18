# AGENTS.md

This document defines engineering guidance for AI/code agents working in this repository.

## Scope

- Project: `forge-loop-cli`
- Language: Rust (workspace in `crates/`)
- Goal: spec-driven autonomous loop CLI with reusable SDD snapshots
- Runtime state: `.forge/`

## Architecture

- `crates/forge-cli`: CLI surface (`forge`, `run`, `status`, `monitor`, `sdd`)
- `crates/forge-core`: loop engine, output analysis, rate limit, circuit breaker
- `crates/forge-config`: `.forgerc` and env/flag precedence
- `crates/forge-monitor`: TUI monitor
- `crates/forge-types`: shared serializable types

## SDD Workflow

When running `forge` with no subcommand, the assistant asks SDD questions and creates a snapshot under:

- `.forge/sdds/<id>/plan.md`
- `.forge/sdds/<id>/spec.md`
- `.forge/sdds/<id>/acceptance.md`
- `.forge/sdds/<id>/scenarios.md`

The active snapshot is tracked in:

- `.forge/current_sdd`

And activated into:

- `.forge/plan.md`
- `docs/specs/session/spec.md`
- `docs/specs/session/acceptance.md`
- `docs/specs/session/scenarios.md`

Use:

- `forge sdd list`
- `forge sdd load <id>`

## Runtime Contracts

Expected files in `.forge/`:

- `status.json`
- `progress.json`
- `live.log`
- `.session_id`
- `.call_count`
- `.last_reset`
- `.circuit_breaker_state`
- `.circuit_breaker_history`

Agents must preserve these contracts unless a migration is explicitly implemented.

Status semantics:

- `run_started_at_epoch`: start of current `forge run`
- `current_loop_started_at_epoch`: start of current loop command
- `last_heartbeat_at_epoch`: last real output/stream heartbeat from Codex process

## Config Precedence

Always preserve precedence:

1. CLI flags
2. Environment variables
3. `.forgerc`
4. Defaults

Codex runtime global flags can be passed via:

- `forge run --codex-arg=<value>` (repeatable)
- `forge run --full-access` (forces `--sandbox danger-full-access`)
- `.forgerc` key `codex_pre_args = ["--sandbox", "danger-full-access"]`

Monitor tuning:

- `forge monitor --stall-threshold-secs <N>` controls stale heartbeat alert threshold

## Development Commands

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Quality Bar

- Keep behavior backward-compatible within v0.x where practical
- Any CLI contract change must include test updates
- Any loop/state change must preserve `.forge/*` output consistency
- Prefer focused edits over broad refactors

## Testing Expectations

- Add or update contract tests in `crates/forge-cli/tests/`
- Add core behavior tests in `crates/forge-core/tests/`
- Validate `fmt`, `clippy`, and `test` before proposing changes

## Documentation Sync

When changing user-facing behavior, update at least:

- `README.md`
- `docs/specs/*` (if feature behavior changed)
- `AGENTS.md`

## Naming Rules

- Use `forge` naming only
- Do not introduce legacy `ralph` naming in code, docs, env vars, or output

## Safety Rules

- Do not run destructive git commands (`reset --hard`, `checkout --`) unless explicitly requested
- Do not delete runtime contracts under `.forge/` without a migration path
- Do not silently change exit-code semantics

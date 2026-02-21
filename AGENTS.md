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
- `crates/forge-engine`: Engine trait and implementations (Codex, OpenCode)
- `crates/forge-monitor`: TUI monitor
- `crates/forge-types`: shared serializable types

## Engines

Forge supports multiple AI coding engines via the `Engine` trait in `forge-engine`:

- **Codex** (default): OpenAI's Codex CLI
- **OpenCode**: Open source AI coding agent

To add a new engine:
1. Implement the `Engine` trait in `forge-engine/src/lib.rs`
2. Add the engine variant to `EngineKind` in `forge-config/src/lib.rs`
3. Update `create_engine()` factory function

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
- `.runner_pid`

Agents must preserve these contracts unless a migration is explicitly implemented.

Status semantics:

- `run_started_at_epoch`: start of current `forge run`
- `current_loop_started_at_epoch`: start of current loop command
- `last_heartbeat_at_epoch`: last real output/stream heartbeat from engine process
- `.runner_pid`: PID of active forge run process (used by monitor to detect stale `running` state)

## Config Precedence

Always preserve precedence:

1. CLI flags
2. Environment variables
3. `.forgerc`
4. Defaults

Engine runtime flags can be passed via:

- `forge run --engine <codex|opencode>` (select engine)
- `forge run --engine-arg=<value>` (repeatable)
- `forge run --full-access` (forces sandbox bypass)
- `.forgerc` key `engine_pre_args = ["--sandbox", "danger-full-access"]`

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
- Add engine tests in `crates/forge-engine/src/lib.rs`
- Validate `fmt`, `clippy`, and `test` before proposing changes

## Documentation Sync

When changing user-facing behavior, update at least:

- `README.md`
- `docs/specs/*` (if feature behavior changed)
- `AGENTS.md`

## Naming Rules

- Use `forge` naming only
- Do not introduce legacy `ralph` naming in code, docs, env vars, or output
- Use `engine` for generic engine references (not `codex` specific)

## Safety Rules

- Do not run destructive git commands (`reset --hard`, `checkout --`) unless explicitly requested
- Do not delete runtime contracts under `.forge/` without a migration path
- Do not silently change exit-code semantics

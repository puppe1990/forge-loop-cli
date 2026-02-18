# forge-loop-cli

Standalone Rust CLI for autonomous coding loops.

## Why

`forge` runs a spec-driven workflow:

1. Collect requirements in phases.
2. Write plan/spec artifacts.
3. Execute implementation loop with guarded completion.

## Commands

- `forge` (interactive assistant mode: asks SDD questions, writes plan/specs, then runs loop)
- `forge run`
- `forge status`
- `forge monitor`

## Assistant mode flow

Running `forge` with no subcommand starts an interview in phases:

1. Intent
2. Scope
3. Constraints
4. Acceptance criteria
5. Given/When/Then scenarios
6. Testing strategy

It generates:

- `.forge/plan.md`
- `docs/specs/session/spec.md`
- `docs/specs/session/acceptance.md`
- `docs/specs/session/scenarios.md`

Then it executes `forge run` in loop mode automatically.

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

## Config precedence

`flags > environment > .forgerc > defaults`

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## License

MIT. See `LICENSE`.

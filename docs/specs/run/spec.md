# Run - Specification

## Goal
Provide `forge run` to execute autonomous loop iterations against Codex CLI and stop only when dual exit gate is satisfied.

## Rules
- MUST write runtime artifacts to `.forge/`.
- MUST evaluate completion with dual gate:
  - At least one completion indicator.
  - Explicit `EXIT_SIGNAL: true`.
- MUST support session continuity with `--resume <id>` and `--resume-last`.
- MUST enforce hourly call limit using persisted counters.
- MUST apply circuit breaker after repeated no-progress loops.

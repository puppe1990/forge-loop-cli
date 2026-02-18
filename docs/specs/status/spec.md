# Status - Specification

## Goal
Provide `forge status` to inspect current runtime state.

## Rules
- MUST read `.forge/status.json`.
- MUST support human output and `--json` output.
- MUST fail with clear message if status file does not exist.

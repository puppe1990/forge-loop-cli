# Monitor - Specification

## Goal
Provide `forge monitor` as a TUI dashboard for live loop inspection.

## Rules
- MUST render fields from `.forge/status.json` and `.forge/progress.json`.
- MUST refresh on interval.
- MUST exit on `q` key.
- MUST tolerate missing/corrupted files without crashing.

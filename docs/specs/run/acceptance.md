# Run - Acceptance Criteria

- [ ] `forge run` creates `.forge/status.json` and `.forge/progress.json`.
- [ ] Loop does not finish when `EXIT_SIGNAL: false` even if completion text exists.
- [ ] Loop finishes when completion indicator and `EXIT_SIGNAL: true` are present.
- [ ] Resume options are parsed and reflected in execution strategy.
- [ ] Rate limit prevents calls beyond configured hourly limit.

# Contributing

Thanks for contributing to `forge-loop-cli`.

## Development setup

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Pull request checklist

- Add or update specs in `docs/specs/` when behavior changes.
- Add tests for new behavior.
- Keep CLI contracts stable or document breaking changes.
- Ensure CI passes.

## Commit style

Prefer focused commits with clear messages, for example:

- `feat(run): add rate-limit pause behavior`
- `fix(status): handle missing status file gracefully`
- `docs(spec): clarify dual exit gate`

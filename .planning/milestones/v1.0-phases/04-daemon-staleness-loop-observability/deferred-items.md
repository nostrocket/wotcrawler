# Phase 4 Deferred Items

Out-of-scope discoveries logged during execution (not fixed — unrelated to the
current task's changes, per the executor scope boundary).

## Pre-existing clippy warnings (discovered 04-01, not introduced by Phase 4)

`cargo clippy --all-targets` surfaces 5 warnings, all in test files NOT touched
by plan 04-01:

- `tests/concurrency.rs:45` — manual implementation of `.is_multiple_of()`
- `tests/nip11_limits.rs:74` — redundant pattern matching, consider `is_err()`
- `tests/graph_writer.rs:7` / `:8` — doc list item without indentation
- `tests/pagination.rs:217` — assertion has a constant value

These predate Phase 4 and are unrelated to the daemon/staleness work. Left as-is.

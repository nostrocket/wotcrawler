# Deferred Items — Phase 02

Out-of-scope discoveries logged during execution (not fixed here).

| Item | File | Discovered | Notes |
|------|------|-----------|-------|
| Pre-existing clippy `redundant_closure` warning | tests/concurrency.rs:45 | 02-05 | Introduced in 01-03 (commit 4f0af1b), unrelated to fetch.rs; left untouched per scope boundary. |
- [02-11] Pre-existing clippy warnings (out of scope, not introduced by 02-11): tests/nip11_limits.rs constant-value assertion; tests/pagination.rs redundant pattern matching (use is_err); tests/concurrency.rs manual is_multiple_of.

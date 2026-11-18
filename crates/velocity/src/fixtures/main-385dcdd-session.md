This JSON was generated, not written by hand. `git archive 385dcdd` was extracted
into `/tmp/velocity-r20-policy-review/main-fixture`. Its production snapshot and
serializer code were unmodified. An isolated `workbench_tests` generator opened
Shift JIS bytes, called `reopen_encoding("Japanese (Shift JIS)")`, then called
`Workspace::snapshot` and `serde_json::to_vec_pretty(Session { .. })`.

The test relocates `Document.path` after deserialization. No JSON keys or history
payloads are rewritten. All generator builds/tests use fixture HOME/SHELL. The
archive's three compile-time permissive build resources link to the workspace's
same vendored Rust/Python/Makefile resources; no Default package is involved.

Generator source: `main-385dcdd-session-generator.rs` (append to the archived
`crates/velocity/src/workbench_tests.rs`, then run its single test with
`R20_MAIN_SESSION_OUTPUT` set). The fixture contains the output JSON verbatim.

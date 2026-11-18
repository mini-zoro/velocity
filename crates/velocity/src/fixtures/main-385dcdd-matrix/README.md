These ten fixtures were generated with unmodified production snapshot code
from `git archive 385dcdd` by the independent verifier. `generator.rs` is the
added test used for generation; `session.json`, `doc.txt`, `encoding.txt` and
`expected.txt` are copied verbatim from `/tmp/a20v3-evidence/main-sessions`.
The tests only relocate paths in memory and restore the recorded mtime.
No session schema is handwritten or retrofitted with choice/identity fields.

Two additional dirty BOM fixtures were generated during this review using
`additional-generator.rs` in the archived `385dcdd` production tree, with fixture
HOME/SHELL. Its successful test log is `/tmp/velocity-r20-model-review/main-generation.log`.
Their JSON and data are also copied verbatim; only test-time path relocation occurs.

# Earlier round-11 measurements

These are the complete observations before moving prefix assembly into its own text module
and removing the unused append/owned-slice APIs. They are retained, including slow/outlier
samples, for comparison; the root-level paired files describe the final implementation.

`pre-module-legacy-pairs.jsonl` contains the initial five pairs plus extensions to 25 for the
flagged cases and 101 for the roughly-one-microsecond undo case. Its paired undo estimate
was +9.49%, CI [1.04384, 1.125], which did not certify the 5% gate. The separate root-level
`undo-aa.jsonl` uses the *same baseline executable* for A and B (51 pairs), showing how noisy
that cold, single-operation probe can be. Neither result is discarded or called a pass.

The earlier feature probes included a 1.258 s full-load observation and a 3.765 ms insertion
outlier. A later five-pair pass alone cannot establish a hard latency guarantee.
`pre-module-linked-sections.json` records hashes of this project's own benchmark code during
layout diagnosis; it is not a claim that the binaries are identical. The loader-v1/v2 paired
experiment is retained separately in `../loader-experiment.jsonl`.

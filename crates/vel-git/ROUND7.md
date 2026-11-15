# Lane B round seven evidence

arm64 macOS, Rust 1.99.1, release. Five separate processes for every new-engine timing;
500 incremental edit samples each for diff and spelling. Times below are median / nearest-rank
p95. All budgets pass for these fixtures. No sysctl calls are used by the new harness.

| Scenario | Budget | Observed median / p95 |
|---|---:|---:|
| Capture 10k one-byte history steps | ≤5 ms | 0.193 / 0.410 ms |
| Capture 10k 100-byte history steps | ≤5 ms | 0.391 / 0.565 ms |
| Serialized one-byte history | ≤20,000 B (2× edits) | 5,311 B (0.5311×) |
| Serialized 100-byte history | ≤2,000,000 B (2× edits) | 18,717 B (0.018717×) |
| Initial diff, 100k lines / 5.5 MB | ≤50 ms | 19.988 / 20.382 ms |
| Local 1-char diff update | ≤1 ms p95 | 0.134 / 0.147 ms |
| Git status, 50,000 tracked files, fresh process | ≤1,000 ms | 70.328 / 194.155 ms |
| Git one-file refresh, 50,000 files | ≤100 ms | 9.724 / 11.003 ms |
| Packages Git status, 1,751 tracked files, fresh process | reported | 9.187 / 35.769 ms |
| Packages Git one-file refresh | reported | 5.459 / 11.274 ms |
| Spell check, exact-scoped 100-line viewport | ≤1 ms | 0.479 / 0.682 ms |
| Spell update plus viewport (one edited line) | ≤1 ms | 0.084 / 0.097 ms |
| Existing buffer benchmarks, 16 observations per side | ≤5% median regression | max +4.85%; metadata unchanged |

History capture shares rope roots and step/selection storage; delta extraction and serde
encoding run on a writer. Encoding itself costs 5.042 / 5.633 ms for one-byte edits,
7.520 / 7.898 ms for 100-byte edits, outside the export/capture timer. Both fixtures
retain 10,000 actual steps with default bounded per-view soft history and full change-map log.
The 100-byte fixture contains 100 bytes per edit, not a synthetic no-op. Tiny snapshots and
incompressible independent metadata have fixed/extra overhead; 2× is a measured fixture gate.

The diff reference has 100,000 unique LF-terminated 55-byte lines. The current file has 101
existing distant changes; initial timing includes flattening, interning and histogram diff.
Each of the 500 character edits alternates a scalar on line 50,000; timer includes engine
update, hunk relocation and snapshot handoff, with Text replacement outside the timer. Only
five affected lines are re-diffed. Tests also cover disjoint windows, Unicode/CR/LF/EOF,
inline spans and exact revert scripts through 1,024 random sequences. Edits in a large existing
hunk can expand to that hunk; ambiguous repeated-line alignments can differ from a whole-file
histogram alignment while retaining an exact edit/revert script.

Git fixtures are isolated temporary repositories: an exact copy of the vendored public
Packages tree (1,751 tracked files), plus 100 folders × 500 generated tracked text files.
No commits or staging occur in the user's repository. 'Cold' means a fresh process and gix
handle/empty application caches; OS disk caches are **not** evicted or claimed cold. Warm timing
modifies exactly one tracked file and includes literal-pathspec refresh and publication assembly.
The fixture generator restores changed files. Neither status nor the library writes the index;
tests compare index bytes and cover both staged/worktree layers, precedence, ignored paths,
conflicts, unborn/detached HEAD, local upstream counts, linked worktrees, symlinks and real
watch delivery/coalescing. Native FSEvents events were unavailable in this sandbox; actual
watch tests use explicit polling with content comparisons. Native delivery still needs an
unrestricted-machine integration check. Polling scans trees and is a fallback, not the large-repo
performance path; status budgets measure the synchronous worker core, excluding watch setup.

Spelling uses the actual pinned English dictionary and actual vel-highlight scope snapshots.
The viewport has 100 lines / 7,590 bytes, 1,500 words and 100 misspellings. Dictionary parsing
and highlighting are outside the timer. The cold viewport check includes tokenization, selector
filtering, dictionary lookup and diagnostics. Edited-viewport timing includes cache relocation
and rechecking exactly one changed line; unchanged cached lines are reused. Suggestions are
on-demand worker operations and are outside the viewport budget.

The original buffer harness remains intact. [Buffer regression details](../vel-buffer/ROUND7.md)
include every sequential and alternating observation, retained p95s and the explicit median gate.
Other lanes may be active; unavailable host-load information is not guessed.

Validation passed for all four touched crates: fmt, clippy all-targets with `-D warnings`,
release unit/integration/doc tests with `PROPTEST_CASES=1024`, rustdoc with `-D warnings`.
No application/UI/session/highlight sources were edited by this lane.

```sh
cargo fmt -p vel-buffer -p vel-diff -p vel-git -p vel-spell --check
cargo clippy -p vel-buffer -p vel-diff -p vel-git -p vel-spell --all-targets -- -D warnings
PROPTEST_CASES=1024 cargo test -p vel-buffer -p vel-diff -p vel-git -p vel-spell --release
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-buffer -p vel-diff -p vel-git -p vel-spell --no-deps
cargo build --release -p vel-buffer -p vel-diff -p vel-git -p vel-spell --examples
python3 crates/vel-git/benches/round7.py --runs 5
```

The last command creates only temporary fixture repositories and writes timing evidence into
the four lane-B crates. `--fixtures-json FILE` reuses prepared fixtures. Build before measuring;
the harness runs binaries serially and never times compilation.

Raw data: [history](../vel-buffer/round7-timings.json), [diff](../vel-diff/round7-timings.json),
[Git](round7-timings.json), [spelling](../vel-spell/round7-timings.json), [summary](round7-summary.json).
Integration examples: [history](../vel-buffer/README.md#round-seven-native-hot-exit-history),
[diff](../vel-diff/README.md), [Git](README.md), [spelling](../vel-spell/README.md).

Dictionary: unmodified SCOWL-derived en_US size 60, 2020.12.07, pinned to dictionaries upstream
commit `0a51498d29db2653688dcb80cdb65a7233b82f1f`. Reviewed the exact shipped license file:
permission to use/copy/modify/distribute/sell with retained copyright/permission notices;
public-domain sources; Princeton WordNet notice; VarCon notice; modified BSD-style Ispell
notice without its advertising clause. No GPL-only dictionary was selected. Full 15,731-byte
[license/credits](../vel-spell/dictionaries/en_US/LICENSE) and
[provenance/SHA-256 manifest](../vel-spell/dictionaries/en_US/PROVENANCE.json) ship with both
`.aff` and `.dic`; `ENGLISH_LICENSE` exposes the same text for application packaging.
The spellbook 0.4.2 library uses MPL-2.0 separately; its license and exact published-source
notice are in [LICENSES](../vel-spell/LICENSES/NOTICE.md). Lane B code is MIT OR Apache-2.0.

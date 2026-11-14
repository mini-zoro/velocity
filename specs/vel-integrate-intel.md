# Spec: integrate scopes/index/completions/snippets into the editor; Goto Definition; scroll regression

Status: P2/P3 / lane A (platform, UI). Locations: `crates/velocity/`, `crates/vel-ui/`, `crates/vel-commands/`,
`crates/vel-config/`, other lane-A crates as needed. Consumes lane-B crates read-only: `vel-highlight`
(scope snapshots, symbols), `vel-index`, `vel-complete`, `vel-snippet`.

## 0. First: the single-view scroll regression

Round 7 measured single-view CPU scroll p50 +8.2% vs round 6 (p95 −16%) under an unisolated
concurrent workload; it was merged under a documented reviewer exception. Re-measure round 6
(`5b128ba^^`… use the archived binary) vs current in **one process-isolated A/B session**
(alternate runs, ≥ 10 each, same input, no other benchmark running in your lane). If the
regression is real (> 5% p50 with non-overlapping confidence), find and fix it; report either way.

## 1. One parse: drop the lane-A `ScopeCache`

Follow `crates/vel-highlight/README.md` ("Scopes and symbols") and the B6 handoff: remove the
second parser/checkpoints; use versioned `ScopeSnapshot` for selector contexts, comments,
brackets, folds and minimap analysis; document symbols from `HighlightService::symbols`; clip
style/scope runs to the line body because lines now include LF. Measure: memory and CPU must go
down vs round 7 (report both).

## 2. Project index UI

- Start `vel-index` per window/project (`index_files`, `index_workers`, exclusions); publish
  unsaved-buffer analysis via `analyze_scopes` → `update_buffer_analysis`.
- **Goto Symbol in Project** (Cmd+Shift+R) in the quick panel with kind badges and file:line;
  **Goto Definition** (F12 / Cmd+Alt+Down) and **Goto Reference** (Shift+F12): single result jumps,
  several open a quick panel with previews; `jump_back`/`jump_forward` (Ctrl+- / Ctrl+Shift+-) with a
  bounded jump history across files.
- Hover over a symbol (`show_definitions` setting): a simple definitions popup (plain widget for
  now; minihtml comes later) listing locations with click-to-open.
- Help → Indexing Status window (progress + log) and a status-bar percentage while indexing.

## 3. Auto-complete UI

- Popup widget anchored at the caret (flips above near the bottom edge), kind badges and
  annotations, details line, keyboard navigation, `auto_complete_commit_on_tab`,
  `auto_complete_with_fields`, `auto_complete_cycle`, mouse selection; driven by
  `vel-complete`'s cancellable worker with the documented triggers/selector/delay settings;
  must never add latency to typing (results arrive asynchronously; stale results dropped).
- Commands: `auto_complete`, `hide_auto_complete`, `commit_completion`, `insert_best_completion`
  (Tab), `replace_completion_with_next_completion`, `move {by: lines}` while the popup is visible
  via contexts (`auto_complete_visible`).

## 4. Snippets UI

- `insert_snippet {contents|name}` and tab-trigger expansion through `vel-snippet`; field
  navigation with Tab/Shift+Tab (`next_field`, `prev_field`, `clear_fields`), contexts
  `has_next_field`/`has_prev_field`, mirrors updating live, field highlight drawing,
  exiting on Esc or caret leaving the fields; snippets from `.sublime-snippet` files in packages,
  listed in the command palette (`Snippet: …`).

## Budgets

- Typing p50/p95 with the completion popup visible and a 100k-word project index: ≤ 2 / 4 ms.
- Goto Definition jump ≤ 20 ms (index lookup + open/scroll of an already-open file).
- Memory: report phys_footprint for a 10-file Rust project with index + completions (target: no
  more than +20 MB over round 7 with the second parser removed).

## Rules

- Modify only lane-A crates (+ root `Cargo.lock`); lane B is concurrently changing `vel-buffer` (additive
  history API), `vel-diff`, `vel-git`, `vel-spell` — don't touch those; if their in-progress `vel-buffer`
  work breaks your build, wait and retry.
- Fresh/read-only/corrupt profile directories must keep working (index cache dir!).
- Headless tests for everything testable headlessly; automatic GUI measurement modes; human checklist
  additions to `MANUAL_TEST.md`.

# Round nine review notes

Implemented in Lane A only; no commit made. This round replaces command replay with native
Hot Exit history and integrates the existing read-only Git, incremental diff and spell engines.

- Native sessions retain text/soft undo, grouping/continuation, saved identity, view owners,
  selections/settings/regions and redo. Clean current text stays lazy; old profiles upgrade
  the information they stored. Changed disk contents or invalid imports discard history safely.
- Restored sheets share immutable keymap tables with independent chord state. Syntax setup
  avoids corpus copies on Unix, loads metadata concurrently and cleans temporary staging on
  a worker; source files stay untouched and syntax contexts remain lazy.
- Git badges follow engine precedence and include dimmed ignored entries plus virtual missing
  paths. Branch counts and optional configured merge-tool actions are available without shell Git.
- Diff markers/navigation, inline original rows and scalar shading, per-hunk undoable reverts,
  saved/index/HEAD references, settings and context menu entry points are wired.
- Spelling uses exact shared scopes and independent clone viewport requests. Suggestions and
  document navigation run on a worker. Add/ignore actions preserve User preferences; package
  dictionaries and per-syntax settings are supported.

Validation and final measurements are recorded in RESULTS.md and timings-round9.json.
RUNBOOK.md contains exact CPU and reviewer native commands. MANUAL_TEST.md records the human's
Chinese IME confirmation and the pending full-history/Git/diff/spelling acceptance cases.
No GUI, native input, display scanout or Metal execution was performed in this sandbox.

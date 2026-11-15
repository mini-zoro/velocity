# Spec: wire release/crash/packages into the app, fix the flaky IPC test, accessibility (VoiceOver)

Status: P4 / lane A. Locations: lane-A crates. Consumes (read-only) `vel-crash`, `vel-packages`
(see `crates/velocity/packaging/README.md` — the round-12 handoff).

## 0. Flaky IPC test → restore parallel CI

`cargo test --workspace --release` with default test threads intermittently times out an existing CLI/instance
IPC test (seen in rounds 10 and 12; passes alone in 0.11 s). Round 12 set `RUST_TEST_THREADS=1` in CI as a stopgap.
Make the test robust under load (deterministic readiness signalling instead of wall-clock deadlines, generous
timeouts only as a last resort, unique socket paths), prove it with 20 consecutive parallel workspace runs, and
remove `RUST_TEST_THREADS: '1'` from `.github/workflows/ci.yml` (this one-line CI edit is yours this round).

## 1. Wiring the round-12 engines

- `vel-crash`: install hooks at startup; on next launch after a crash show a native prompt offering to open the
  report folder; Help menu "Open Crash Reports Folder".
- `install_cli` command + Velocity menu entry "Install Command Line Tool…" with the confirmation flow.
- Packages: command palette entries "Package: Install from File…", "Package: Install from URL…", "Package: Remove",
  "Package: Disable/Enable", "Package: List" (quick panel showing contributions and the "no plugin API" marker),
  "Package: Check for Upgrades" (only when the user configured an index URL); after install/remove/enable, reload
  affected resources (syntaxes, color schemes, themes, keymaps, menus, snippets, completions, build systems) live,
  honouring `ignored_packages`. Work happens on a worker; the UI never blocks.

## 2. Accessibility (VoiceOver)

References: Apple's NSAccessibility protocols (public documentation).
- Expose the editor view as an `NSAccessibility` text area: value (visible text or windowed document text for huge
  files), selected text range(s), insertion point line number, `AXLineForIndex`/`AXRangeForLine`/
  `AXStringForRange`/`AXBoundsForRange` parameterized attributes, announcements on caret/selection changes; works
  with multiple carets (expose the primary).
- Tabs, side bar tree, quick panel rows, status bar, buttons and popups as accessible elements with roles, labels and
  actions (press, select); keyboard focus order; respect "Reduce motion" (disable theme animations) and
  "Increase contrast" if feasible.
- Tests: headless unit tests of the accessibility model (ranges, lines, bounds, roles); a MANUAL_TEST.md VoiceOver
  checklist for the human tester.

## Rules

- Your worktree: /Users/zoro/dev/velocity-lane-a (branch `lane-a`). Do not commit. Lane-A crates + the single CI line
  above (+ root `Cargo.lock`). Lane B works on HTML export / command coverage / performance in another worktree.
- Keep typing/scroll within 5% (paired A/B against main). `cargo test --workspace --release` with **default
  parallelism** must pass.

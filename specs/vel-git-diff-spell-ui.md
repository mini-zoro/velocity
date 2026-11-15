# Spec: Git & incremental diff UI, spell-check UI, complete Hot Exit history

Status: P3 / lane A (platform, UI). Locations: `crates/velocity/`, `crates/vel-ui/`, `crates/vel-session/`,
`crates/vel-commands/`, `crates/vel-config/`, other lane-A crates as needed. Consumes (read-only) the
lane-B engines `vel-buffer` (history API), `vel-diff`, `vel-git`, `vel-spell`.

References (public docs only): git_integration.html, incremental_diff.html, spell_checking.html,
settings.html.

## 1. Hot Exit with complete history

Switch `vel-session` to `Buffer::export_history` / `Buffer::restore_history` (see
`crates/vel-buffer/README.md`): soft-undo history, groups, saved point and view ownership survive
quit/relaunch; mismatch (file changed on disk) drops history as before. Keep fresh/read-only/
corrupt profile robustness. Tests: quit-restore round trips with randomized edit/undo/soft-undo
sequences in two views produce identical subsequent undo/redo behavior.

## 2. Git integration UI (`vel-git`)

- Side bar status badges for files and folders (documented badge set and precedence), ignored
  entries de-emphasized, tooltips; status bar branch name + counts (untracked/modified/staged/
  unmerged), clicking it opens Sublime-Merge-like actions only if a merge tool is configured
  (otherwise nothing); settings `show_git_status`, `show_git_status_in_status_bar`, `git_binary`
  ignored (we use gix).

## 3. Incremental diff UI (`vel-diff`)

- Gutter diff markers (added/modified/deleted) with the color scheme's `line_diff_*` globals and
  `mini_diff` setting (`true`/`false`/`"auto"`), `git_diff_target` (`"index"`/`"head"`) when the file
  is in a repo, otherwise the saved file; `next_modification`/`prev_modification` (Ctrl+. / Ctrl+,),
  **inline diff** toggle (`toggle_inline_diff`, Cmd+K Cmd+/ and the "show only this" variant),
  **revert hunk** (`revert_hunk`, Cmd+K Cmd+Z), `revert_modification`, context-menu entries.
  Feed every vel-buffer ChangeSet with its post-edit snapshot as the README requires; reset on
  undo snapshot gaps.

## 4. Spell checking UI (`vel-spell`)

- `spell_check` setting (per syntax), squiggly underlines (color scheme `misspelling` global) only in
  `spelling_selector` scopes for visible lines, `next_misspelling`/`prev_misspelling` (F6 toggles
  spell check, Ctrl+F6 / Ctrl+Shift+F6 navigate as documented), context menu with suggestions,
  "Add to dictionary", "Ignore word"; `dictionary` setting selects among available `.dic` files.

## Budgets

- Typing latency unchanged within 5% with git badges, diff markers and spell check active
  (re-measure isolated A/B as in round 8).
- Hot Exit save with full history for 50 dirty files ≤ 30 ms background; restore unchanged budget.

## Rules

- You work in your own git worktree (`/Users/zoro/dev/velocity-lane-a`, branch `lane-a`). Modify only
  lane-A crates (+ root `Cargo.lock`). Lane B works in another worktree on `vel-theme` / `vel-minihtml`.
  Do not commit. No GUI/Metal in the sandbox: headless tests + automatic GUI modes + MANUAL_TEST.md
  additions for the human tester (they tested IME and are testing Hot Exit — keep that checklist current).

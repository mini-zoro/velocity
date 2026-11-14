# Spec: syntax highlighting in the editor + core view features

Status: P2 / lane A (platform, UI). Locations: `crates/velocity/`, `crates/vel-ui/`, `crates/vel-render/`,
`crates/vel-commands/`, `crates/vel-config/` (+ the other lane-A crates as needed). Consumes (read-only)
`vel-highlight`, `vel-color-scheme`, `vel-syntax` (metadata), `vel-scope` — owned by lane B.

Behavior references (public docs only): settings.html, color_schemes.html, indentation.html,
https://www.sublimetext.com/docs/ (code folding notes in the ST4 changelog: "syntax-based code folding"),
key_bindings.html for command names.

## 1. Syntax highlighting integration

- One `vel_highlight::HighlightService` per buffer (see `crates/vel-highlight/README.md`), fed by
  `vel-buffer` change sets; render the versioned style spans; never block the UI thread.
- Syntax detection on open (extension, `hidden_file_extensions`, `first_line_match`), the
  `set_file_type {syntax}` command, a syntax picker in the status bar and in the command palette
  (`Set Syntax: …`), and syntax-specific settings files (`<Syntax>.sublime-settings`).
- Color schemes: `color_scheme` setting incl. `"auto"` with `dark_color_scheme` /
  `light_color_scheme` following the macOS appearance (live switch), default
  `Velocity Dark`/`Velocity Light` from `resources/Packages/Color Scheme - Velocity/`. Apply all
  relevant **globals** to rendering (background, foreground, caret, line_highlight, selection,
  selection_border, inactive_selection, gutter, gutter_foreground, find_highlight, invisibles,
  guide/active_guide/stack_guide, brackets_*, rulers, minimap_border, misspelling later).
- Font styles: bold / italic (font variants via CoreText fallback chain), underline,
  stippled/squiggly underline; `font_options` subset (`no_bold`, `no_italic`).
- `selector` key-binding contexts now evaluate against real scopes (auto-pairing quotes inside
  strings/comments must behave like ST's documented contexts).

## 2. Language-aware editing (uses `vel-syntax` metadata)

- `toggle_comment {block: bool}` with TM_COMMENT_START/END variables, multi-cursor aware.
- Auto-indent on Enter and `reindent` using the indentation rules (increase/decrease/
  bracketIndentNextLine/disableIndentNextLine/unIndentedLine); `detect_indentation` on open.
- **Goto Symbol** (Cmd+R): symbols of the current file from `showInSymbolList` metadata with
  `symbolTransformation`; preview while moving through the list. Leave "Goto Symbol in Project"
  for the index round.
- Bracket matching: highlight the matching bracket/tag pair at carets (`match_brackets`,
  `match_brackets_content`, `match_brackets_square`, `match_brackets_braces`,
  `match_brackets_angle`, `match_tags`), ignoring brackets inside strings/comments by scope.

## 3. View rendering features

- **Word wrap**: `word_wrap` (true/false/"auto" per syntax), `wrap_width`, `wrap_width_style`,
  `indent_subsequent_lines`; vertical caret movement uses visual lines (`VerticalLayout` from
  vel-buffer); wrapping must be incremental (only re-wrap edited lines) and correct for CJK/emoji.
- `draw_white_space` (selection/all/none + leading/trailing options), `draw_indent_guides` +
  `indent_guide_options` (draw_normal, draw_active), `rulers`, `highlight_line`, `caret_style`
  (smooth/phase/blink/solid), `caret_extra_width`, `line_padding_top/bottom`, `scroll_past_end`,
  `scroll_context_lines`, `relative_line_numbers`, `gutter`/`line_numbers`/`margin`.
- **Code folding**: indentation-based and syntax-based (fold regions from scope-based heuristics:
  matching brackets / block meta scopes), fold markers in the gutter (`fold_buttons`,
  `fade_fold_buttons`), commands `fold`, `unfold`, `fold_by_level {level}`, `unfold_all`,
  `fold_tag_attributes`; folded text drawn as an ellipsis marker; folds survive edits (track as
  `vel-buffer` named regions).
- **Minimap**: scaled-down rendering of the document using the same style spans (rects per token,
  not glyphs, at small scale), viewport indicator (`always_show_minimap_viewport`), click/drag to
  scroll, `toggle_minimap`. Must stay cheap: incremental updates, GPU cost ≤ 1 ms at 4K.

## Budgets

| Metric | Target |
|---|---|
| Typing latency (1 caret, highlighted Rust file, input → present-return) | p50 ≤ 2 ms, p95 ≤ 4 ms |
| Open a 10 MB Rust file: first highlighted screen | ≤ 150 ms after the window shows content |
| Scroll frame CPU with highlighting + minimap at 4K | p95 ≤ 2 ms |
| Re-wrap after a 1-char edit in a 100k-line file | ≤ 1 ms |
| Fold/unfold all in a 100k-line file | ≤ 20 ms |

## Rules

- Modify only lane-A crates: `velocity` (except `resources/Packages/Color Scheme - Velocity/`, owned by
  lane B), `vel-ui`, `vel-render`, `vel-platform`, `vel-config`, `vel-commands`, `vel-fileio`,
  `vel-search` (+ root `Cargo.lock`). Do not modify lane-B crates; if you need an API change there,
  describe it in your final message. Lane B is concurrently rewriting `vel-regex` internals; if the build
  breaks because of their in-progress work, retry later instead of editing their files.
- Headless tests for everything testable headlessly (wrap math, fold regions, bracket matching, comment
  toggling, reindent, syntax detection, style application); automatic GUI measurement modes for the
  reviewer; human checklist additions in `MANUAL_TEST.md`.

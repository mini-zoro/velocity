# Contributing to Velocity

## Clean-room policy (mandatory)

Velocity re-implements the *behavior* of Sublime Text from public information only.

**Allowed sources**

- Sublime Text's public documentation (sublimetext.com/docs), blog posts and changelogs.
- Black-box observation of how the shipping editor behaves (typing, commands, rendering).
- Files published under a permissive license, notably
  [sublimehq/Packages](https://github.com/sublimehq/Packages) (syntax definitions, syntax tests,
  tmPreferences) — keep their license notices.
- Any open-source project whose license is compatible with MIT OR Apache-2.0 (e.g. syntect,
  ropey, crop, Zed's MIT/Apache crates) — credit the source when adapting code.

**Not allowed**

- Disassembling, decompiling or otherwise inspecting Sublime Text binaries.
- Copying files that ship only inside the Sublime Text application: the `Default` package, the
  Default/Adaptive themes, the bundled color schemes (Mariana, Monokai, Breakers, Celeste,
  Sixteen), `sublime.py` / `sublime_plugin.py`, icons, artwork.
- Using the "Sublime Text" name or logo for this product.

Key bindings and command names are functional facts and may match Sublime Text so that existing
`.sublime-keymap` files work, but every file in this repository must be written from scratch.

## Engineering rules

- **Performance is a feature.** Changes on hot paths must include benchmark numbers. A regression
  of more than 5% on a tracked benchmark blocks the merge.
- **Tests are the referee.** Syntax engine work is judged by the official `syntax_test_*` files;
  data-structure work by property tests against a simple model.
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` must pass.
- `unsafe` requires a `// SAFETY:` comment explaining the invariant.
- Public items need rustdoc.

## AI-assisted development

Much of the code is written by AI coding agents working from the specs in `specs/`. The same rules
apply to agent-written code: it must follow the clean-room policy, pass the tests and benchmarks,
and be reviewed before it is merged.

# Native review of the packaged app (round 12, 2026-11-16)

Artifact: `scripts/package-macos.sh` output at `85e6cc1` (Velocity.app, arm64, ad-hoc signed), launched outside
the sandbox with a fresh `--data-dir` and `crates/vel-text/src/tree.rs:120`. Screenshot of the Velocity window only:
[round12-native-light.png](round12-native-light.png) (system appearance: light).

## Works

- Window, Velocity light theme, Rust highlighting (keywords, types, constants, italic comments), gutter, minimap with
  viewport indicator, tab bar, side bar sections, status bar (position, selections, encoding, line endings,
  indentation, Git branch/status, syntax).
- DMG: strict deep codesign verification of the app inside the mounted image; headless smoke test runs from the DMG.

## Bugs found (headless tests could not catch these)

1. **`file:line` does not reveal the line.** Opened `tree.rs:120`: the status bar reports `Ln 120, Col 1`, but the view
   shows lines 351–387. Opening at a location must scroll so the caret is visible (Sublime centers it).
2. **Unthemed black rectangle** at the top-left: the strip above the side bar, level with the tab bar, is painted black
   instead of the theme's side bar / tab set background.
3. **Debug window title** "Velocity · native AppKit". Should follow Sublime's convention: file name (and dirty marker,
   project name when one is open), e.g. `tree.rs — Velocity`.
4. **UI font falls back to Times New Roman.** Log: `CoreText note: Client requested name ".SFNS-Regular", it will get
   TimesNewRomanPSMT`. Side bar headings render in a serif font. Use the system UI font API
   (`CTFontCreateUIFontForLanguage` / `NSFont.systemFont`), never the private `.SFNS-*` names.
5. **Release app writes measurement JSONL to the build machine's source tree**
   (`…/velocity-lane-b/crates/velocity/output/interactive-*.jsonl`, a compile-time path). Interactive telemetry must be
   off by default in normal launches; when enabled it must go to `~/Library/Logs/Velocity/`.

## Not verified here

- Gatekeeper: `spctl` rejects the ad-hoc signed app, as expected without Developer ID signing and notarization (user decision).
- Dark appearance, popups, IME in the packaged app (IME was verified by the user on an earlier build).

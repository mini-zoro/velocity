# User behaviour: decisions and audit

Rule: behave like Sublime Text 4. Where Velocity adds something Sublime does not have
(the graphical Settings page, the Simplified Chinese UI), the addition must not change
how a Sublime user's habits work. Sublime's default values live in its Default package,
which the clean-room rule forbids us to read; where a default cannot be confirmed from
public sources, the decision is the user's and is recorded here.

## Decisions (user, 2026-11-20)

| # | Topic | Decision | Status |
| --- | --- | --- | --- |
| D1 | ⌘, | Opens the Default/User two-pane JSON settings, as in Sublime. The graphical Settings page is reached from Preferences › Settings…, the command palette and a button in the JSON window. | To do (revert R1's ⌘, change) |
| D2 | Default UI language | `ui_language: "auto"`: Simplified Chinese when it is the first macOS preferred language that has a catalog, otherwise English. | To do (default was `"en"`) |
| D3 | Find in Files results | A "Find Results" tab like Sublime: scrollable, editable, double-click / Return jumps to the match; a "Use Buffer"-style toggle in the panel switches to the bottom output panel. | Done (round 31 lane C) |
| D4 | Opening files from Finder / Dock | `open_files_in_new_window: "never"` stays the default: files open as tabs in the current window. | Done (unchanged) |
| D5 | Closing windows / relaunch | Closed windows do not come back at launch; quitting restores the windows open at quit (`hot_exit`). Closing a non-project window with unsaved changes prompts. | Done (round 28 lane A) |

## Velocity-only additions (kept, opt-in or out of the way)
- Graphical Settings page (`specs/vel-settings-ui.md`); JSON editing stays the primary Sublime path (D1).
- Simplified Chinese UI and the i18n framework (`docs/i18n.md`); English text and behaviour are unchanged.

The public [Search and Replace guide](https://docs.sublimetext.io/guide/usage/search-and-replace.html#results-format)
documents the separate view, context and match navigation. The [theme reference](https://www.sublimetext.com/docs/themes.html#icons)
distinguishes a buffer from an output panel. The [changelog](https://www.sublimetext.com/download)
documents ordered results and long-line truncation. D3 supplies the default-on setting, append/reuse
policy, exact report text, and Command-click behavior; those details are not established by these sources.
No separate public Use Buffer menu or palette entry was confirmed.

## Audit
The journey-by-journey comparison (open, edit, find, navigate, save, close, quit, relaunch,
projects, build, packages, settings) is added below by the behaviour audit round. Each item is
classified: same as Sublime / differs on purpose (with reason) / differs (bug, fixed or
scheduled) / uncertain (to discuss with the user).

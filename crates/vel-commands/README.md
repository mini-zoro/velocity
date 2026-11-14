# vel-commands

Sublime-named command registry over `vel-buffer`. This is an original implementation of the
round specification and public command vocabulary; it contains no copied Default package code.

`Registry::standard()` installs text, window and application command names. The `Host` trait
supplies the active buffer/view, effective settings, clipboard and native actions. It also
supplies enablement policy; unknown commands are disabled and execution returns an error.
Extensions can replace commands with callbacks. Chains have a recursion bound of 32.

Text commands delegate to `vel_buffer::ops`, including multi-caret edits, movement, find/add/
skip/all selections, line operations, case operations and hard/soft history. The adapter adds
newline indentation, case-insensitive line sorting, clipboard line-copy behavior and minimal
snippets (`$0`, numeric fields, `${1:default}`). Field navigation and wrap_lines remain deferred
as specified. Native scrolling/centering is delegated to the host. Editing settings are only
installed when changed, so ordinary typing keeps the buffer's undo grouping.

Macro recording/replay and redo-or-repeat are available for text commands in the active view.
Velocity executes window/text chains in order through its application dispatcher. Recorded
macros should stay within one view; macro state is currently per view.

Tests apply the text command set to multiple selections, verify line-block isolation, Unicode,
clipboard, snippet fields, undo/redo and soft history, and check all window command dispatches.

```sh
cargo test -p vel-commands
```

Round 5 registers window actions for overlays, input/output/find panels, file search/replacement,
find-next/previous/all/under, slurp strings, search flags, side-bar visibility/focus and project
folders. The host implements these workflows using vel-search and vel-ui; the registry remains
independent of both. Panel text edits use the same text commands against their own buffer.

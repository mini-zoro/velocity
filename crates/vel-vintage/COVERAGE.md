# Vintage command coverage and integration

The engine is headless and original. The fixtures encode public vi semantics;
these are not recorded observations of the shipping editor. The command table has
94 triples and the text-object table has 24; the final property run uses 4,096
random sequences, including a saved Unicode visual-replace/search regression. Every command below
has an executable case in `tests/commands.rs`, with random-sequence invariants in
the same target. Tests use byte caret markers and include combining accents,
CJK and ZWJ emoji, multiple carets, overlapping operators and empty lines.

| Area | Implemented entrypoints |
| --- | --- |
| Modes | insert, normal, replace, visual character/line/block, command line |
| Counts/operators | leading and motion counts multiply; d c y > < = g~ gu gU; doubled line operators |
| Motions | h j k l w W b B e E ge 0 ^ $ gg G f F t T ; , % { } ( ) H M L |
| Search | / ? n N * #, wrapping and counts, operator-pending search |
| Text objects | iw aw iW aW is as ip ap i( a( i[ a[ i{ a{ i< a< it at i" a" i' a' i` a` |
| Registers | a–z, unnamed, 0, +; fragments retain multiple-cursor and line/block shape |
| Replay/history | dot, q/@ macros, marks m/'/backtick, u and Ctrl-R, insert/change sessions |
| Editing | J r ~ x X s S C D Y p P o O a A i I, R, Unicode text input and backspace |
| Ex | :w :wq :q :e (optional file, !), numeric line addresses |
| Control keys | Ctrl-V, Ctrl-R, Ctrl-F/B/D/U, controlled by vintage_ctrl_keys |

`=` requests the host's syntax-aware reindent command. H/M/L use `set_viewport`
and the supplied VerticalLayout; the layout trait addresses LF lines rather than
soft-wrapped display rows. File commands are requests and retain force/path args.
Clipboard access is explicit: supply a snapshot with `set_clipboard`, then dispatch
`set_clipboard` requests. The engine performs no I/O or OS clipboard calls.

```rust,ignore
let outcome = vi.handle_key(key, &mut buffer, view, layout);
match outcome {
    Outcome::Handled => invalidate_view(),
    Outcome::PassThrough => dispatch_remaining_keymap(),
    Outcome::NeedsCommand(command) => {
        let undo_groups = dispatch_host_command(command);
        vi.complete_host_command(&mut buffer, view, undo_groups);
    }
}
```

`complete_host_command` records reindent history and restores normal carets. For
file/clipboard requests it is a no-op. Hosts retain responsibility for normal dirty
file checks and asynchronous file completion. Macro playback is bounded to 16
nested calls and 10,000 keys per input event. Macros containing host requests do not
suspend and resume around asynchronous dispatch; only the last request is returned.

Contexts: setting.command_mode, setting.vintage_ctrl_keys, vi_mode_normal,
vi_mode_insert, vi_mode_replace, vi_mode_visual, vi_mode_visual_line,
vi_mode_visual_block, vi_command_line, vi_has_action/vi_operator_pending,
vi_has_motion, vi_has_count, vi_recording_macro. Unknown predicates are false.
`mode()` and `command_line()` supply the corresponding UI state.

Compatibility limits still open:

- Vi sessions are one **engine undo action**, using the real buffer history.
  The buffer has no explicit merge-transaction API, so direct Buffer::undo may
  expose its individual insert transactions. Bind vi undo/redo through Vintage.
  External buffer edits invalidate the engine's journal; ordinary external cursor
  movement does not. Marks map through retained ChangeIds.
- Search uses Rust regex syntax, not Vim's magic/nomagic dialect or search flags.
  Text objects use lexical delimiter/sentence/tag rules rather than a syntax AST.
- Block register paste handles existing rows; virtual columns, padding short rows
  and creating rows beyond EOF remain open. Rectangle selection itself uses the
  supplied layout and yields one region per existing row.
- The small requested Ex surface is implemented. Full Vim Ex ranges, substitution,
  registers outside the specified set, jump lists and persistent macro/mark storage
  are outside this engine's current surface. There is no shipped editor keymap in
  this lane; lane A owns that binding and the prompt/clipboard/file UI.

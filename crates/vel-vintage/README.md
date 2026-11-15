# vel-vintage

A pure, headless vi state machine over `vel_buffer::Buffer`. One `Vintage` belongs
to one buffer/view pair. Byte offsets and grapheme motions follow vel-buffer.

```rust
use vel_buffer::{Buffer, MonospaceLayout};
use vel_vintage::{Vintage, Settings, Key};
let mut buffer = Buffer::new("one two");
let view = buffer.new_view();
let mut vi = Vintage::new(Settings {
    vintage_start_in_command_mode: true,
    vintage_ctrl_keys: true,
    ..Settings::default()
});
vi.handle_key('d', &mut buffer, view, &MonospaceLayout::default());
vi.handle_key('w', &mut buffer, view, &MonospaceLayout::default());
assert_eq!(buffer.text().to_string(), "two");
vi.handle_key('u', &mut buffer, view, &MonospaceLayout::default());
assert_eq!(buffer.text().to_string(), "one two");
vi.handle_key(Key::Escape, &mut buffer, view, &MonospaceLayout::default());
```

The host sends decoded `Key`s to `handle_key`, dispatches `Outcome::NeedsCommand`
through its command system and uses `context` for keymap predicates. File commands
are requests; the engine never saves, opens, closes or accesses the system clipboard.
Supply clipboard snapshots with `set_clipboard`; dispatch `set_clipboard` requests
when `vintage_use_clipboard` is enabled. `set_viewport` supplies H/M/L geometry.

Undo/redo use the buffer's real history. Route vi undo through `u`/Ctrl-R: the engine
journals the transaction count for each command, including the entire insert/change
session, and restores its command selections. Direct host `Buffer::undo` bypasses
this vi grouping. External edits invalidate that journal; marks use ChangeId mapping.
All multi-cursor edits are committed in sorted pre-transaction coordinates.

This crate is an original implementation from the task specification, public vi
semantics and the public Vintage documentation. No application package or binary
was inspected. See `COVERAGE.md` for tested commands and compatibility limits.

After dispatching a requested reindent, call `complete_host_command(buffer, view,
undo_groups)` (normally one group). File and clipboard dispatches need no history
callback. See [COVERAGE.md](COVERAGE.md) for the context names, command matrix and
remaining compatibility limits.

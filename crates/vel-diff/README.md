# vel-diff

Incremental LF line diffs over vel-text snapshots, with histogram anchors, gutter hunks,
navigation, UTF-8 inline diffs and transaction-ready revert plans. MIT OR Apache-2.0.
Initial/reference-reset work belongs on the analysis worker. Each change re-diffs separate
changed windows expanded through existing hunks to unchanged context. It never tokenizes an
unchanged tail. An edit inside a large existing hunk can require re-diffing that whole hunk.
Repeated equal lines can admit multiple valid minimal scripts; incremental anchors preserve
local alignment, so hunk placement can differ from a fresh whole-file histogram alignment.

```rust
use vel_buffer::{Buffer, Change, ChangeSet, Edit, EditKind, Selection};
use vel_diff::{DocumentDiff, InlineMode};
let mut buffer = Buffer::new("hello\nworld\n");
let view = buffer.new_view();
let mut diff = DocumentDiff::new(buffer.text().clone(), buffer.text().clone(), buffer.change_id());
let edits = [Edit { range: 0..5, text: "hi" }];
buffer.transact(view, EditKind::Other, &edits, Selection::default());
let changes = ChangeSet::new(vec![Change { old_range: 0..5, new_len: 2 }]);
diff.apply_change(buffer.text().clone(), buffer.change_id(), &changes)?;
let hunk = &diff.hunks()[0];
let inline = diff.inline_diff(hunk, InlineMode::Character)?;
let plan = diff.revert_hunk(hunk)?;
buffer.transact(view, EditKind::Other, &[Edit { range: plan.range, text: &plan.text }], Selection::default());
# Ok::<(), vel_diff::Error>(())
```

Coordinates are half-open and zero-based. Tokens include LF (CR stays content), with no
phantom empty EOF line. A deletion's current interval is an empty boundary, possibly EOF;
clamp that boundary to a display row in the gutter. Re-query hunks after any change. Revert
plans do not mutate Buffer and therefore preserve ordinary undo. `revert_all` returns sorted
old-coordinate edits. Navigation is strict and has an explicit wrap option.

Forward every committed ChangeSet with its matching post-edit snapshot, including undo/redo
steps. `Buffer::changes_since` supplies retained descriptors. If several steps were published
without their intermediate snapshots, recreate `DocumentDiff` from the reference/current
snapshots. Version IDs alone cannot validate a descriptor from another buffer.
`set_reference_document` selects saved-file/HEAD/index content supplied by fileio/vel-git;
`reset_reference_document` captures current content after a successful save.

Round-seven budget, validation and raw timing evidence: [engine report](../vel-git/ROUND7.md).

# vel-fileio

Headless encoding-aware document IO. Automatic loading checks BOMs, valid UTF-8,
then a bounded UTF-16 heuristic before using the configured fallback (Windows-1252
by default). NULs outside recognizable UTF-16 route to the binary view. The UTF-16
heuristic scores both byte orders over the first 8 KiB, validates surrogate pairs,
and allows tab, CR, LF and form feed. A trailing high surrogate can belong to the
next sample. Pure BMP CJK without ASCII or surrogate evidence is deliberately not
guessed: either byte order can look like letters. Decode errors never silently
replace characters. `Format` retains the encoding, chosen alias caption, BOM and
majority LF/CRLF/CR convention; mixed input is normalized to LF. Ties choose LF,
then CRLF. Encoding names ending in “with BOM” request a BOM; other names do not.

`DecodeOptions.encoding` represents an explicit user choice, never an automatic
detection result. Reload and clean session restore retain the explicit preference
separately from the decoder used for the current revision. A conflicting disk BOM
or clear UTF-16 byte-order evidence overrides that preference with a non-modal
diagnostic; binary content is rejected before decoding. A chosen decoder that
cannot read the bytes falls back to ordinary detection with a diagnostic, without
turning the detected/fallback codec into a user choice. Later reloads retry the
retained preference.

`load_background_with_options` streams UTF-8 through a bounded CR/CRLF normalizer;
other codecs use encoding_rs streaming conversion followed by the same rope snapshot
builder. Both publish shared prefixes before the complete file is decoded,
including forced encodings on reload/restore. Byte progress and cancellation track
the source stream. Provisional buffers stay read-only and unsavable. Invalid codec
attempts clear their prefixes before retrying detection/fallback. Dropping a task
cancels without joining the UI thread. Clean native history is attached to the
completed decoded snapshot without rereading the disk file.
`load_background_for_reload` also compares the decoded revision with the immutable
saved snapshot on the worker (length plus exact chunk comparison). It publishes no
provisional prefixes and retires an unchanged decoded tree on the worker. The UI
can acknowledge a touch without scanning text, replacing a buffer, losing undo,
or conflicting with dirty edits.

`prepare_save` makes whitespace/final-newline cleanup one undoable buffer transaction and maps
all selections. `ChangeTracker` retains the saved text/change identity and uses the buffer's
historic position mapping, so inserting lines does not make shifted unchanged lines eligible
for trimming. Expired history conservatively marks uncertain lines as modified. `not_on_caret`
excludes every caret line. A successful caller marks the buffer/tracker saved.

`save` converts line endings and encodings, rejects unrepresentable characters before the
destination changes and syncs the result. New files are created directly, so they get the process
umask. With `atomic_save` an existing file is replaced by renaming a synced temporary file with
the same permissions over it; files with other hard links, another owner, extended attributes or a
read-only mode are written in place instead (a read-only file therefore fails to save). Symlinks
keep pointing to their target. `atomic_save` defaults to false, as in Sublime.

`Watcher` watches parent directories through notify, surviving atomic replacements. It also
checks watched-file metadata when polled, because native notifications may be unavailable in
restricted sessions. Velocity polls every 200 ms without rendering when idle, reloads clean
files and asks before replacing dirty files. With `reload_file_on_change: false`, clean files
use that same confirmation path. Large external reloads also use workers.

```sh
cargo test -p vel-fileio
cargo run --release -p vel-fileio --example load_bench
cargo run --release -p vel-fileio --example load_bench -- /path/to/file.txt
```

The generated load workload is exactly 100,000,000 ASCII bytes. Generation is outside the
measurement. Output reports worker queue time and completed streaming load time, with GUI/GPU
explicitly false. Tests cover encoding/BOM/ending round trips, Unicode property tests, streaming
seams, atomic failure/permissions, modified-line cleanup and watcher delivery with fallback.

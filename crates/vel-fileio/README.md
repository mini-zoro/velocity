# vel-fileio

Headless encoding-aware document IO. BOMs select UTF-8/UTF-16LE/UTF-16BE; otherwise loading
tries UTF-8 before the configured fallback (Windows-1252 by default). NUL in the first 8 KiB
is rejected as binary unless a UTF-16 BOM establishes the encoding. Decode errors never
silently replace characters. The detected encoding, BOM and majority LF/CRLF/CR convention
are retained in `Format`; mixed input is normalized to LF and saved in the majority convention.
Ties choose LF, then CRLF. A leading UTF-8 U+FEFF byte sequence is interpreted as a BOM.

A streaming decoder feeds `vel_text::Text::from_reader`, including CR/UTF-8/UTF-16 boundary
handling. `load_background` exposes byte progress and a terminal channel without UI dependencies.
Velocity queues files at/above 64 MiB on this worker. UTF-8 uses `vel_largefile::stream_utf8`
through a bounded CR/CRLF normalizer, publishing shared prefix snapshots from the first 2 MiB.
`LoadTask::try_snapshot` is a nonblocking, coalescing mailbox; poll it before the terminal result.
Prefixes are provisional and must remain read-only and unsavable. Closing/dropping a task cancels
at the next read/build boundary without joining the UI thread. Encoding fallback clears the
prefix before the existing full worker decoder retries; cancellation also covers that decoder.
Progress counts source bytes, while the snapshot metrics describe normalized UTF-8. The final
`Format` preserves BOM, encoding and majority source line endings. `content_kind` exposes the
first-8-KiB/BOM heuristic so the editor can route binary files to a hex tab before decoding.

`prepare_save` makes whitespace/final-newline cleanup one undoable buffer transaction and maps
all selections. `ChangeTracker` retains the saved text/change identity and uses the buffer's
historic position mapping, so inserting lines does not make shifted unchanged lines eligible
for trimming. Expired history conservatively marks uncertain lines as modified. `not_on_caret`
excludes every caret line. A successful caller marks the buffer/tracker saved.

`save` converts line endings and encodings into a buffered temporary file, rejects unrepresentable
characters, syncs it, preserves existing permissions and atomically renames it. Symlinks keep
pointing to their target. `atomic_save:false` still validates the encoding in a temp file before
copying into the destination, but intentionally loses rename atomicity.

`Watcher` watches parent directories through notify, surviving atomic replacements. It also
checks watched-file metadata when polled, because native notifications may be unavailable in
restricted sessions. Velocity polls every 200 ms without rendering when idle, reloads clean
files and asks before replacing dirty files. Large external reloads also use workers.

```sh
cargo test -p vel-fileio
cargo run --release -p vel-fileio --example load_bench
cargo run --release -p vel-fileio --example load_bench -- /path/to/file.txt
```

The generated load workload is exactly 100,000,000 ASCII bytes. Generation is outside the
measurement. Output reports worker queue time and completed streaming load time, with GUI/GPU
explicitly false. Tests cover encoding/BOM/ending round trips, Unicode property tests, streaming
seams, atomic failure/permissions, modified-line cleanup and watcher delivery with fallback.

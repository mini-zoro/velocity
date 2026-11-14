# vel-build

Device-free `.sublime-build` parsing, variable expansion, asynchronous process execution and
result navigation. Original implementation from the public [build-system documentation](https://www.sublimetext.com/docs/build_systems.html).
No application-bundled Default files or binaries were used.

`BuildSystem` parses comment/trailing-comma JSON through vel-config, retains extension keys,
merges platform options and named variants, and resolves the built-in `exec` target. `cmd` is
an argv array; `shell_cmd` takes precedence. `shell: true` shell-quotes argv. Working directory,
environment and PATH are explicit. Automatic eligibility accepts a matching scope, filename
glob, `keyfile` or `keyfiles`. Custom targets are rejected before spawning. Built-in cancellation
is provided by `Task::cancel`; Python cancellation commands/plugins are outside this executor.

`Variables` supplies file, project, folder, packages and platform variables, nested
`${name:default}`, regex `${name/pattern/format/}`, capture replacement and escaped dollars.
Values substituted from a filename are never recursively expanded. Regex syntax is Rust regex;
unsupported lookarounds/backreferences produce errors. Shell quoting in `shell_cmd` remains the
build author's responsibility. Output decoding is UTF-8 with replacement for invalid bytes;
incomplete Unicode sequences are retained between pipe reads.

`Task` runs in its own Unix process group, reads stdout/stderr concurrently, and sends output
through a bounded 128-chunk queue. Cancel/Drop kills the process group, reaps the child, and
unblocks inherited pipes. Descendants still holding pipes after the parent exits are terminated.
The terminal event follows pipe drain and reports exit code, cancellation and elapsed time.
No build directory or output file is created by this crate: the invoked command controls its
own output. A missing/non-directory/read-only working directory is a recoverable spawn error.

`ResultParser` supports filename/line/column/message groups, filename-only headers followed by
`line_regex` records, ANSI escapes and relative paths. Existing paths are canonicalized for
matching editor tabs (including macOS `/var` aliases). Output line indices drive double-clicks;
F4/Shift+F4 and inline text badges are integrated in velocity. Its output panel uses the normal
syntax/wrap renderer; retained output is bounded to 8 MiB. Input longer than 1 MiB without a
newline is truncated for diagnostic parsing while the process continues draining.

Velocity embeds a few public build resources as-is from `third_party/sublime-packages`, whose
[license](../../third_party/sublime-packages/LICENSE) remains in the repository. Our interactive
fixture under `velocity/fixtures/workbench.sublime-project` is authored from scratch.

```sh
cargo test -p vel-build
cargo clippy -p vel-build --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-build --no-deps
```

Tests run real short-lived processes without AppKit/Metal and check both pipes, split UTF-8,
process-tree cancellation, variable/default/substitution expansion, variants, eligibility,
unsupported targets, relative filenames and line-only diagnostics.

# Lane B, round 5: regex backend, parser throughput and metadata

> Reviewed round-four baseline: **scopes 125,877/0, symbols 688/0, reindent 26/9, zero skips**. Partial-symbols files still evaluate scope assertions; annotation tails use the general longest-parseable-prefix rule.

Pinned public corpus: **202 syntaxes, 242 test files, 259 tmPreferences files**. Loading warnings: 0; error files: 0.

| Category | Passed | Failed | Skipped | Unit |
| --- | ---: | ---: | ---: | --- |
| Scopes | 125,877 | 0 | 0 | assertion span |
| Symbols/references | 688 | 0 | 0 | assertion / unannotated symbol |
| Reindent | 30 | 5 | 0 | whole-file variant |
| Total | 126,595 | 5 | 0 | sum of the above |

Overall: **126,595/126,600 (99.99605%)**. Scope columns: 590,758 passed / 0 failed.
The denominator remains 126,600. All prior per-file scope/symbol/reindent passes are preserved, and four previously failing reindent variants now pass.

## Metadata behavior

XML/binary plist preferences merge independently per key by selector score and resource order. Shell variables merge by name. All requested indentation, symbol, completion and preservation settings remain available through Metadata; unknown plist keys are retained. The plist XML dependency drops CDATA events, so the loader preserves them as equivalent escaped character data before parsing.

All 688 symbol assertions pass, including quoted transformed names, local/global definitions, indexed references, partial-symbol files, and exhaustive checking for unannotated symbols. Adjacent nested symbol spans coalesce and transformations support captures, case conversion, and global zero-width/nonempty retry semantics. The base symbol conventions are clean-room implementations of public scope naming; no application Default preferences are used.

Assertion selectors use the longest prefix accepted by the selector grammar when trailing text is unparseable. This is a general rule; the literal suffix '() {}' has no special handling. Negative selectors still reject actual comment scopes. Partial-symbols files execute their scope assertions.

All 31 reindent files execute, totaling 35 requested whole-file variants. reindent runs both unchanged and unindented checks; reindent-unindented strips whitespace and reparses the resulting text before indentation. One whole-file comparison is one case, regardless of file length. Remaining mismatches are failures, not skips. The corpus guard locks scopes, symbols, and reindent separately per file; baseline migration permits newly tested failures without weakening existing scope thresholds.

## Remaining reindent failures

| File | Variant | First differing line | Evidence |
| --- | --- | ---: | --- |
| C++/tests/syntax_test_indent_common.c++ | reindent-unchanged | 770 | expected "        v++ /*(*/ // ; \"comment\" ()", got "    v++ /*(*/ // ; \"comment\" ()" |
| CSS/tests/syntax_test_indentation.css | reindent-unchanged | 77 | expected "} /**/ .foo /**/ {              /*", got "    } /**/ .foo /**/ {              /*" |
| JavaScript/tests/syntax_test_js_indent_common.js | reindent-unchanged | 682 | expected "    case                         // comments", got "        case                         // comments" |
| JavaScript/tests/syntax_test_jsx_indent.jsx | reindent-unchanged | 34 | expected "                indented=\"attribute\"", got "            indented=\"attribute\"" |
| ShellScript/Zsh/tests/syntax_test_reindent_case.zsh | reindent-unchanged | 107 | expected "case $var {", got "    case $var {" |

Remaining failures expose nested continuation/control indentation, CSS comment transitions, JSX attributes and Zsh brace-style cases. Both PHP variants, Bash case indentation and Bash heredoc indentation now pass. Scope-selected metadata is preserved; continuation bodies on the same line no longer arm another indentation step. The indentation engine is usable but is not fully conformant; retain this evidence when extending it. Symbol/scope success does not imply unasserted tokens or all editor behavior are identical to ST.

`.tmLanguage` conversion and zip syntax loading remain prior-round limitations. Numeric selector scoring remains the approximation documented by vel-scope. Full-file default palettes, style rendering flags, and editor scheduling are provided by vel-color-scheme / vel-highlight, not the syntax runner.

Runner regex counters: `{"compilations": 18444, "compile_errors": 0, "search_errors": 499}`. Resource-limit events remain explicit counters and are treated as no-match; they are not a claim that every regex path completed without limits.

## Parser performance

Release, macOS 26.1 arm64, rustc 1.99.1. Every timing below has five runs. No sysctl was called; the parallel lane can affect scheduling. Input: 10,000,000 Rust bytes / 297,030 lines. These parser-only timings use reusable LineOps and bounded stable-transition memoization, and exclude styling and UI work. Backend-isolated measurements (memoization disabled for both engines), per-context profiles, cold compilation and full differential coverage are recorded in ../vel-regex/PERF.md.

| Measurement | Five-run median | Budget / reviewed comparison |
| --- | ---: | --- |
| Rust parser | 2.475 MB/s; 73,501 lines/s | 1.408 MB/s reviewed baseline; +75.75% throughput |
| YAML loading | 882.556 ms | 1500 ms |
| Cached index + source content hashes | 26.457 ms | 20 ms |
| Stable edit convergence, parser only | 0.00013 ms | one reparsed line |
| Forced fresh 100-line parse | 0.010 ms | checkpoint replay with the shared stable-line cache; service bounds synchronous work |

Search/capture workspaces and output vectors reuse allocated capacity across lines and nested branch replay. Stable line transitions share interned operations in a bounded document cache. The allocations integration test counts zero Rust allocations over 10,000 warmed simple streaming lines, both with memoization disabled and enabled; native Oniguruma C allocation is outside that counter. This does not serialize scratch into checkpoints or change semantic parser equality. End-to-end service timings, including five actual background-thread runs, are in ../vel-highlight/RESULTS.md. Warm style resolution and its zero-allocation check are in ../vel-color-scheme/README.md.

Raw parser seconds: `[4.003842958, 4.041190542, 4.105135458, 4.106061167, 4.004366833]`. Regex counters over warm-up and five runs: `{"compilations": 80291, "compile_errors": 0, "search_errors": 6}`. All observations are in tests/timings.json.

## All 202 loaded syntaxes

Counts include metadata assertions attributed by the declared syntax path. A dash means no executed assertions; it is not reported as 100%.

| Syntax | Passed | Failed | Skipped | Pass rate |
| --- | ---: | ---: | ---: | ---: |
| ASP | 0 | 0 | 0 | — |
| ActionScript 2.0 | 184 | 0 | 0 | 100.0000% |
| AppleScript | 513 | 0 | 0 | 100.0000% |
| Bash | 11136 | 0 | 0 | 100.0000% |
| Bash (for Markdown) | 0 | 0 | 0 | — |
| Batch File | 3918 | 0 | 0 | 100.0000% |
| Batch File (Compound) | 0 | 0 | 0 | — |
| BibTeX | 107 | 0 | 0 | 100.0000% |
| Binary | 0 | 0 | 0 | — |
| C | 876 | 0 | 0 | 100.0000% |
| C# | 9425 | 0 | 0 | 100.0000% |
| C++ | 1757 | 1 | 0 | 99.9431% |
| CSS | 3514 | 1 | 0 | 99.9716% |
| CSS (Go) | 0 | 0 | 0 | — |
| CSS (PHP) | 0 | 0 | 0 | — |
| CSS (Rails) | 48 | 0 | 0 | 100.0000% |
| CSS (for ASP) | 0 | 0 | 0 | — |
| CSS (for HAML) | 0 | 0 | 0 | — |
| CSS (for JS template) | 0 | 0 | 0 | — |
| CSS (for JSP) | 0 | 0 | 0 | — |
| CSS (for PHP Interpolated) | 0 | 0 | 0 | — |
| CSS (for PHP double-quoted) | 0 | 0 | 0 | — |
| CSS (for PHP single-quoted) | 0 | 0 | 0 | — |
| CSS (for Ruby) | 0 | 0 | 0 | — |
| CSS (for TS template) | 0 | 0 | 0 | — |
| CSS inside Go backtick string | 0 | 0 | 0 | — |
| Cabal | 421 | 0 | 0 | 100.0000% |
| Cargo Build Results | 0 | 0 | 0 | — |
| Cassandra Query Language | 399 | 0 | 0 | 100.0000% |
| Clojure | 1699 | 0 | 0 | 100.0000% |
| ClojureScript | 0 | 0 | 0 | — |
| D | 3110 | 0 | 0 | 100.0000% |
| DMD Output | 0 | 0 | 0 | — |
| DTD | 0 | 0 | 0 | — |
| Diff | 311 | 0 | 0 | 100.0000% |
| Diff (Basic) | 0 | 0 | 0 | — |
| Diff (for Markdown) | 0 | 0 | 0 | — |
| Diff (for PHP Interpolated) | 0 | 0 | 0 | — |
| Erlang | 4350 | 0 | 0 | 100.0000% |
| Git Attributes | 176 | 0 | 0 | 100.0000% |
| Git Blame Ignore Revisions | 37 | 0 | 0 | 100.0000% |
| Git Code Owners | 92 | 0 | 0 | 100.0000% |
| Git Commit | 258 | 0 | 0 | 100.0000% |
| Git Commit Message | 137 | 0 | 0 | 100.0000% |
| Git Common | 0 | 0 | 0 | — |
| Git Config | 566 | 0 | 0 | 100.0000% |
| Git Diff | 220 | 0 | 0 | 100.0000% |
| Git Ignore | 285 | 0 | 0 | 100.0000% |
| Git Link | 53 | 0 | 0 | 100.0000% |
| Git Log | 162 | 0 | 0 | 100.0000% |
| Git Mailmap | 45 | 0 | 0 | 100.0000% |
| Git Rebase Todo | 392 | 0 | 0 | 100.0000% |
| Go | 4402 | 0 | 0 | 100.0000% |
| Go (Embedded Backtick String) | 0 | 0 | 0 | — |
| Go (Embedded CSS) | 0 | 0 | 0 | — |
| Go (Embedded HTML) | 0 | 0 | 0 | — |
| Go (Embedded JSON) | 0 | 0 | 0 | — |
| Go (Embedded JavaScript) | 0 | 0 | 0 | — |
| Go (Embedded Regex) | 0 | 0 | 0 | — |
| Go (Embedded SQL) | 0 | 0 | 0 | — |
| Go (Embedded T-SQL) | 0 | 0 | 0 | — |
| Go (Embedded XML) | 0 | 0 | 0 | — |
| Graphviz (DOT) | 226 | 0 | 0 | 100.0000% |
| Groovy | 295 | 0 | 0 | 100.0000% |
| HAML | 368 | 0 | 0 | 100.0000% |
| HTML | 1013 | 0 | 0 | 100.0000% |
| HTML (ASP) | 993 | 0 | 0 | 100.0000% |
| HTML (Erlang) | 60 | 0 | 0 | 100.0000% |
| HTML (Go) | 169 | 0 | 0 | 100.0000% |
| HTML (JSP) | 701 | 0 | 0 | 100.0000% |
| HTML (Plain) | 0 | 0 | 0 | — |
| HTML (Rails) | 167 | 0 | 0 | 100.0000% |
| HTML (Tcl) | 0 | 0 | 0 | — |
| HTML (for HAML) | 0 | 0 | 0 | — |
| HTML (for HSX) | 0 | 0 | 0 | — |
| HTML (for JS template) | 0 | 0 | 0 | — |
| HTML (for PHP Interpolated) | 0 | 0 | 0 | — |
| HTML (for Ruby) | 0 | 0 | 0 | — |
| HTML (for TS template) | 0 | 0 | 0 | — |
| HTML inside Go backtick string | 0 | 0 | 0 | — |
| Haskell | 3492 | 0 | 0 | 100.0000% |
| JSON | 135 | 0 | 0 | 100.0000% |
| JSON (Go) | 0 | 0 | 0 | — |
| JSON (PHP) | 0 | 0 | 0 | — |
| JSON (Rails) | 0 | 0 | 0 | — |
| JSON (for ASP) | 0 | 0 | 0 | — |
| JSON (for HAML) | 0 | 0 | 0 | — |
| JSON (for JS template) | 0 | 0 | 0 | — |
| JSON (for JSP) | 0 | 0 | 0 | — |
| JSON (for PHP Interpolated) | 0 | 0 | 0 | — |
| JSON (for TS template) | 0 | 0 | 0 | — |
| JSON inside Go backtick string | 0 | 0 | 0 | — |
| JSX | 619 | 1 | 0 | 99.8387% |
| JSX (Plain) | 0 | 0 | 0 | — |
| Java | 8904 | 0 | 0 | 100.0000% |
| Java (for HTML) | 0 | 0 | 0 | — |
| Java (for JSP) | 0 | 0 | 0 | — |
| Java Properties | 84 | 0 | 0 | 100.0000% |
| JavaDoc | 0 | 0 | 0 | — |
| JavaScript | 3013 | 1 | 0 | 99.9668% |
| JavaScript (Go) | 0 | 0 | 0 | — |
| JavaScript (PHP) | 0 | 0 | 0 | — |
| JavaScript (Plain) | 0 | 0 | 0 | — |
| JavaScript (Rails) | 31 | 0 | 0 | 100.0000% |
| JavaScript (for ASP) | 0 | 0 | 0 | — |
| JavaScript (for Go Embedded Backtick Strings) | 0 | 0 | 0 | — |
| JavaScript (for HAML) | 0 | 0 | 0 | — |
| JavaScript (for JS template) | 0 | 0 | 0 | — |
| JavaScript (for JSP) | 0 | 0 | 0 | — |
| JavaScript (for PHP Interpolated) | 0 | 0 | 0 | — |
| JavaScript (for PHP double-quoted) | 0 | 0 | 0 | — |
| JavaScript (for PHP single-quoted) | 0 | 0 | 0 | — |
| JavaScript (for Ruby) | 0 | 0 | 0 | — |
| JavaScript (for TS template) | 0 | 0 | 0 | — |
| LaTeX | 1827 | 0 | 0 | 100.0000% |
| LaTeX Log | 80 | 0 | 0 | 100.0000% |
| Lisp | 1995 | 0 | 0 | 100.0000% |
| Literate Haskell | 32 | 0 | 0 | 100.0000% |
| Lua | 915 | 0 | 0 | 100.0000% |
| MATLAB | 631 | 0 | 0 | 100.0000% |
| Make Output | 0 | 0 | 0 | — |
| Makefile | 552 | 0 | 0 | 100.0000% |
| Markdown | 4883 | 0 | 0 | 100.0000% |
| Markdown (Go) | 160 | 0 | 0 | 100.0000% |
| Markdown (for HAML) | 0 | 0 | 0 | — |
| MultiMarkdown | 26 | 0 | 0 | 100.0000% |
| MySQL | 3735 | 0 | 0 | 100.0000% |
| OCaml | 238 | 0 | 0 | 100.0000% |
| OCamllex | 0 | 0 | 0 | — |
| OCamlyacc | 41 | 0 | 0 | 100.0000% |
| Objective-C | 710 | 0 | 0 | 100.0000% |
| Objective-C++ | 1513 | 0 | 0 | 100.0000% |
| PHP | 4468 | 0 | 0 | 100.0000% |
| PHP Source | 0 | 0 | 0 | — |
| PHP Source (for PHP) | 0 | 0 | 0 | — |
| Pascal | 397 | 0 | 0 | 100.0000% |
| Perl | 4592 | 0 | 0 | 100.0000% |
| PostgreSQL | 710 | 0 | 0 | 100.0000% |
| Python | 6313 | 0 | 0 | 100.0000% |
| R | 334 | 0 | 0 | 100.0000% |
| R Console | 0 | 0 | 0 | — |
| Rd (R Documentation) | 0 | 0 | 0 | — |
| RegExp (for Python raw-f-string) | 0 | 0 | 0 | — |
| RegExp inside Go backtick string | 0 | 0 | 0 | — |
| Regex (for C# literal string) | 0 | 0 | 0 | — |
| Regex (for C# verbatim string) | 0 | 0 | 0 | — |
| Regular Expression | 548 | 0 | 0 | 100.0000% |
| Regular Expression (Basic) | 0 | 0 | 0 | — |
| Regular Expression (Perl inside angle brackets) | 0 | 0 | 0 | — |
| Regular Expression (Perl inside braces) | 0 | 0 | 0 | — |
| Regular Expression (Perl inside parens) | 0 | 0 | 0 | — |
| Regular Expression (Perl inside square brackets) | 0 | 0 | 0 | — |
| Regular Expression (Perl) | 0 | 0 | 0 | — |
| Regular Expression Replacement | 36 | 0 | 0 | 100.0000% |
| Regular Expressions (C#) | 206 | 0 | 0 | 100.0000% |
| Regular Expressions (JavaScript) | 0 | 0 | 0 | — |
| Regular Expressions (PHP) | 0 | 0 | 0 | — |
| Regular Expressions (Python) | 0 | 0 | 0 | — |
| Ruby | 1151 | 0 | 0 | 100.0000% |
| Ruby (Rails) | 20 | 0 | 0 | 100.0000% |
| Ruby (for HAML) | 0 | 0 | 0 | — |
| Rust | 2440 | 0 | 0 | 100.0000% |
| SQL | 0 | 0 | 0 | — |
| SQL (Basic) | 0 | 0 | 0 | — |
| SQL (Rails) | 37 | 0 | 0 | 100.0000% |
| SQL (for C# raw string) | 0 | 0 | 0 | — |
| SQL (for C# verbatim format string) | 0 | 0 | 0 | — |
| SQL (for C# verbatim string) | 0 | 0 | 0 | — |
| SQL (for C#) | 0 | 0 | 0 | — |
| SQL (for JS template) | 0 | 0 | 0 | — |
| SQL (for PHP Interpolated) | 0 | 0 | 0 | — |
| SQL (for PHP) | 0 | 0 | 0 | — |
| SQL (for Ruby) | 0 | 0 | 0 | — |
| SQL (for TS template) | 0 | 0 | 0 | — |
| SQL inside Go backtick string | 0 | 0 | 0 | — |
| Scala | 1704 | 0 | 0 | 100.0000% |
| Shell | 5 | 0 | 0 | 100.0000% |
| Shell (for Makefile) | 0 | 0 | 0 | — |
| ShellScript (for Ruby) | 0 | 0 | 0 | — |
| Sublime File Pattern | 144 | 0 | 0 | 100.0000% |
| T-SQL | 1660 | 0 | 0 | 100.0000% |
| T-SQL inside Go backtick string | 0 | 0 | 0 | — |
| TOML | 660 | 0 | 0 | 100.0000% |
| TOML (for Python) | 0 | 0 | 0 | — |
| TSX | 662 | 0 | 0 | 100.0000% |
| TSX (Plain) | 0 | 0 | 0 | — |
| Tcl | 201 | 0 | 0 | 100.0000% |
| TeX | 794 | 0 | 0 | 100.0000% |
| TeX (DocStrip) | 46 | 0 | 0 | 100.0000% |
| Textile | 655 | 0 | 0 | 100.0000% |
| TypeScript | 2307 | 0 | 0 | 100.0000% |
| TypeScript (Plain) | 0 | 0 | 0 | — |
| XML | 1485 | 0 | 0 | 100.0000% |
| XML (for PHP Interpolated) | 0 | 0 | 0 | — |
| XML inside Go backtick string | 0 | 0 | 0 | — |
| YAML | 607 | 0 | 0 | 100.0000% |
| YAML (Go) | 95 | 0 | 0 | 100.0000% |
| YAML (for PHP Interpolated) | 0 | 0 | 0 | — |
| Zsh | 8075 | 1 | 0 | 99.9876% |
| Zsh (for Markdown) | 0 | 0 | 0 | — |
| camlp4 | 0 | 0 | 0 | — |
| reStructuredText | 42 | 0 | 0 | 100.0000% |

Reproduce with cargo run --release -p vel-syntax --bin syntax-tests; cargo run --release -p vel-syntax --bin syntax-bench; python3 crates/vel-syntax/scripts/report.py. tests/last-run.json contains every file/category and the first mismatch evidence. tests/baseline.json is the monotonic regression gate, not a claim of zero known failures.

Editor API: see ../vel-highlight/README.md. Metadata-only callers use MetadataSet::metadata_for, indent::scoped_spans, MetadataSet::symbols, and indent::reindent.

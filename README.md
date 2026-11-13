# Velocity

**A fast, native text editor for macOS, written in Rust.**
Velocity aims for the feature set and responsiveness of Sublime Text 4, and reads the same
package formats (`.sublime-syntax`, `.sublime-color-scheme`, `.sublime-theme`, `.sublime-keymap`,
`.sublime-settings`, `.sublime-snippet`, `.sublime-build`, …), so existing user configuration and
syntax packages keep working.

> Status: **pre-alpha / P0 validation.** Nothing here is usable as an editor yet.

Velocity is an independent project. It is not affiliated with, endorsed by, or derived from the
source code of Sublime HQ Pty Ltd. "Sublime Text" is a trademark of its owner and is used here only
to describe compatibility.

## Goals

- **Speed first.** Typing latency, startup, large files and multi-cursor editing on par with or
  better than Sublime Text 4 — enforced by benchmarks in CI.
- **Format compatibility.** Load Sublime-format syntaxes, color schemes, themes, key bindings,
  menus, snippets, completions, build systems, macros and projects unchanged.
- **Native macOS.** GPU rendering (Metal via wgpu), proper input methods (CJK IME), native menus
  and window behavior. Other platforms come later.

Not in scope for 1.0: Python plugin API compatibility, Windows/Linux.

## Repository layout

| Path | What |
|---|---|
| `crates/` | Product crates (`vel-*`) |
| `docs/PLAN.md` | Feasibility study, architecture and roadmap (Chinese) |
| `specs/` | Per-component specifications and acceptance criteria |
| `spikes/` | Building-block benchmarks (rope, syntect, search, text layout) and results |
| `prototypes/` | Throwaway prototypes that de-risk specific decisions |

## Building

Requires macOS 13+ on Apple Silicon or Intel and the Rust toolchain pinned in `rust-toolchain.toml`.

```sh
cargo build --release
cargo test --workspace
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), in particular the **clean-room policy**.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions.

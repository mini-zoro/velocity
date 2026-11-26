# Velocity

**A fast, native text editor for macOS and Windows, written in Rust.**
Velocity aims for the feature set and responsiveness of Sublime Text 4, and reads the same
package formats (`.sublime-syntax`, `.sublime-color-scheme`, `.sublime-theme`, `.sublime-keymap`,
`.sublime-settings`, `.sublime-snippet`, `.sublime-build`, …), so existing user configuration and
syntax packages keep working.

> Status: **early release (0.1).** Daily-usable on macOS. The Windows x64 build is
> built, tested and packaged by GitHub Actions but has not been tried on a Windows desktop
> yet; see [docs/windows-port.md](docs/windows-port.md).

## Download

Get the latest build from [GitHub Releases](https://github.com/mini-zoro/velocity/releases):

- **macOS** (universal, macOS 14+): `.dmg` or `.app.zip`. The app is ad-hoc signed and not
  notarized; after copying it to Applications run
  `xattr -dr com.apple.quarantine /Applications/Velocity.app` once.
- **Windows** (x64): the portable `.zip` (run `velocity.exe`; `vel.exe` is the command-line
  client) or the per-user `-setup.exe` (Start menu, Explorer "Open with", optional PATH).
  The builds are unsigned, so SmartScreen may warn.

Check downloads against `SHA256SUMS`.

Velocity is an independent project. It is not affiliated with, endorsed by, or derived from the
source code of Sublime HQ Pty Ltd. "Sublime Text" is a trademark of its owner and is used here only
to describe compatibility.

## Goals

- **Speed first.** Typing latency, startup, large files and multi-cursor editing on par with or
  better than Sublime Text 4 — enforced by benchmarks in CI.
- **Format compatibility.** Load Sublime-format syntaxes, color schemes, themes, key bindings,
  menus, snippets, completions, build systems, macros and projects unchanged.
- **Native on each platform.** GPU rendering through wgpu (Metal on macOS, DX12 on Windows),
  proper input methods (CJK IME), native menus and window behavior, and each platform's
  Sublime-style default key bindings.

Not in scope for 1.0: Python plugin API compatibility, Linux.

## Repository layout

| Path | What |
|---|---|
| `crates/` | Product crates (`vel-*`) |
| `docs/PLAN.md` | Feasibility study, architecture and roadmap (Chinese) |
| `specs/` | Per-component specifications and acceptance criteria |
| `spikes/` | Building-block benchmarks (rope, syntect, search, text layout) and results |
| `prototypes/` | Throwaway prototypes that de-risk specific decisions |

## Building

Requires the Rust toolchain pinned in `rust-toolchain.toml` and a checkout with submodules
(`git clone --recursive`, or `git submodule update --init`).

- **macOS** 14+ on Apple Silicon or Intel with the Xcode command line tools.
  `scripts/package-macos.sh` builds the app bundle, DMG and zip.
- **Windows** 10/11 x64 with the MSVC build tools and Windows SDK.
  `scripts/package-windows.ps1` builds the portable zip and, with Inno Setup installed, the
  setup program.

See [Release updates](docs/releases.md) for updater behavior and release signing.

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

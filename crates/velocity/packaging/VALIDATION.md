# Round 12 local validation — 2026-11-15

Built on macOS arm64 in the `lane-b` worktree. Only `aarch64-apple-darwin` was installed,
so the local artifacts are arm64; the workflow installs both targets for universal builds.
No app source or other existing crate was modified. No commit, push, release or upload was made.

Default output directory: `target/package-macos/`.

| Artifact | Bytes | MiB |
| --- | ---: | ---: |
| `Velocity.app` (sum of file contents) | 43,154,011 | 41.15 |
| `Velocity-0.0.0-arm64.dmg` | 13,949,808 | 13.30 |
| `Velocity-0.0.0-arm64.app.zip` | 15,207,100 | 14.50 |

The directory also contains `SHA256SUMS` and `packaging-report.json`. Prior successful
outputs are retained as timestamped backups on subsequent runs.

Verified by running `scripts/package-macos.sh` locally:

- Bundle id `dev.velocity.Velocity`; Cargo version `0.0.0`; macOS 14.0 in both Info.plist
  and the actual Mach-O load command; executable architecture `arm64`.
- Original generated icon, document Editor types, Retina/system appearance keys, all required
  package/grammar/dictionary resources and licenses, and the bundled CLI launcher.
- 1,618 hashed resource-manifest entries; strict ad-hoc app and DMG code-sign verification.
- The DMG checksum and a decompressed HFS+ read-back: all 1,623 app files, their bytes/modes,
  and the real `Applications -> /Applications` symlink matched the signed source bundle.
- Packaged CLI forwarding to a running headless protocol peer: symlink invocation, relative
  paths, spaces, Unicode row/column locations, project/new/add flags, `--`, stdin, and waiting
  until a close acknowledgement. The existing instance-server unit test also passed.

Final code checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
RUST_TEST_THREADS=1 cargo test --workspace --release --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
rustfmt --edition 2024 --check crates/velocity/packaging/forwarding-fixture.rs
bash -n scripts/package-macos.sh scripts/test-package-macos.sh
sh -n crates/velocity/packaging/velocity
```

All passed. Workspace release tests: **474 passed, 0 failed, 0 ignored**, including 19 new
package/crash/CLI tests. Python helpers were parsed with the system Python AST parser.
Shellcheck was not installed; shell syntax checks passed, and CI runs shellcheck when available.

One parallel full-suite run hit the existing
`instance::tests::fresh_profile_socket_forwarding_waits_for_requested_sheet_close` timeout
(2-second server read / 3-second test deadline). It passed alone in 0.11 seconds and in the
complete serial run. CI now sets `RUST_TEST_THREADS=1` for the workspace test step to avoid
worker contention; the existing test/app source remains unchanged.

Unexercised paths: x86_64/universal (target absent), Developer ID signing and notarization
(no credentials). The current native platform still rejects GUI startup on macOS 13; Lane A
must add the frame-clock fallback documented in README.md. Crash/menu/package integration
also remains with Lane A. The optional package index is unset and no public server was used
by the new tests.

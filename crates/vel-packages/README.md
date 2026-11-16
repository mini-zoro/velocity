# vel-packages

Synchronous package management for a single Velocity data directory. There is no Python
runtime, plugin execution, bundled registry, timer, or implicit network request. Perform
filesystem/download operations on the app's worker queue, then publish results on the UI thread.

`PackageManager::new(data)` is read-only and has no index. `list()` combines
`Installed Packages/Name.sublime-package` and `Packages/Name`, with loose resources and metadata
overriding matching archive paths. It reports syntax, color scheme, theme, keymap, snippet,
completion and build-system contributions, versions from `package-metadata.json`, Python
plugin paths, and user-level disabled state. `Support::PartialNoPluginApi.label()` is
`partially supported: no plugin API`; Python files do not prevent declarative resource use.
Discovery checks archived bytes/CRCs, skips symlinks in loose trees, and returns an error naming
an invalid archive rather than loading it. A fresh profile remains absent after listing.

`install_local(name, file)` copies to a temporary file on the destination filesystem.
`install_https(name, url, client)` downloads there using `/usr/bin/curl` on macOS. Both validate
ZIP paths, entry types, case collisions, decompression, lengths and CRCs before atomic rename.
Limits: 128 MiB archive, 32 MiB per resource, 256 MiB expanded, 10,000 entries. ZIP contents are
never extracted or executed. HTTPS requires certificate verification, has a deadline and size
cap, ignores `.curlrc`, and allows only HTTPS redirects. No shell is involved. An explicit
private CA is supported for enterprise servers and tests; there is no insecure TLS setting.

`Installed { path, backup }` returns the previous archive's recoverable backup. `remove(name)`
renames both archive and loose overrides into `data/.velocity-backups` and
returns their paths. It preflights both sources and rolls back prior moves on a rename error.
It leaves preferences, including `ignored_packages`, intact. Backups are outside both resource
search roots, so old/removed resources cannot load again. `User` is protected from install,
disable and remove. Managed subdirectory symlinks and conflicting CLI targets are rejected.
Mutations use a nonblocking OS file lock; concurrent operations fail rather than racing, and
the kernel releases the lock on exit/crash. The small lock file intentionally stays on disk.

`set_enabled(name, enabled)` edits only the user's `ignored_packages`, preserving JSON comments,
trailing commas and unrelated preferences. Existing preferences are backed up before atomic
replacement. `set_enabled_inheriting(name, enabled, inherited)` preserves lower-priority ignored
defaults when creating the first user override. The caller must reload settings/resources and
exclude ignored packages when loading them; this crate does not mutate an editor workspace.
`Package.disabled` reflects user-level settings; the app can overlay effective defaults for UI.

The optional index has exactly this version-1 schema (example values; no service is configured):

```json
{
  "schema_version": 1,
  "packages": [{
    "name": "Example",
    "version": "1.2.3",
    "url": "https://example.invalid/Example.sublime-package",
    "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
  }]
}
```

Versions use SemVer. The archive contains `package-metadata.json` with `{"version":"1.2.3"}`.
Unknown fields, schema versions, duplicate name/version entries, insecure URLs and bad hashes
are rejected. `Index::parse` and `Index::upgrades` are offline operations. Only explicitly calling
`set_index_url(Some(user_url))` followed by `check_upgrades(&client)` fetches anything (1 MiB
index limit). No configured index returns an empty list without even scanning the filesystem.
Upgrade checks select the highest SemVer precedence, skip prereleases for stable/unknown
installs, include disabled packages, and report unknown installed versions explicitly.
`install_release` verifies SHA-256 and matching archive metadata before installation. An index
digest detects corrupt/replaced downloads; trust still comes from the user-selected HTTPS index.

`cli::CliInstallPlan::for_bundle(app, bin_dir)` resolves the bundled launcher.
`cli::install_directories(home)` offers `/usr/local/bin` and `~/.local/bin`.
`cli::install_cli(&plan, confirm)` requires the app's confirmation callback before any write,
creates the symlink atomically, is idempotent, and never overwrites an existing unrelated tool.
It does not escalate privileges or modify shell profiles. If `/usr/local/bin` is unwritable,
offer the user-local choice and explain adding that directory to PATH.

Tests generate archives locally, exercise corruption, traversal, overlays, inherited settings,
locking, backups, removal and CLI confirmation. The TLS fixture uses a locally generated CA,
OpenSSL for certificate generation, and Python's standard-library TLS server bound to loopback.
Python is only a test fixture dependency, never a library/package runtime. Tests cover rejected
certificates, HTTPS redirects, rejected HTTP downgrades, HTTP errors, size caps, index checks,
metadata mismatches and SHA-256 failures. No public server is contacted.

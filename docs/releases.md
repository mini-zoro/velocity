# Release updates

Velocity checks the public GitHub `releases/latest` endpoint for newer stable
versions. It sends `User-Agent: Velocity/<version>` and a cached ETag, without
accounts, cookies or telemetry. Automatic checks run on an existing idle tick no
earlier than 30 seconds after launch and at most once per 24 hours. Turn off
**Settings › General › Automatically check for updates** to leave only manual
checks. **Later** dismisses the notice. **Skip This Version** persists in the
profile's `Updates/check.json`.

**Restart to Update** downloads, verifies and stages the replacement, then uses
normal Quit and Hot Exit. **Install on Quit** arms the same restart after staging
finishes; quitting before the download finishes or logging out does not apply it.
Use **Velocity › Quit Velocity** on macOS or **File › Exit** on Windows; closing
an individual window is not an app quit.
The updater uses a one-time full Hot Exit checkpoint for the restart, without
changing the configured Hot Exit preferences. It does not install automatically
without one of these choices.

- **macOS:** The universal app ZIP is unpacked with `ditto`. The app ID,
  packaged version and strict code-signature check must all pass. The helper
  places the candidate on the bundle's volume, then atomically exchanges it
  with the old bundle. A read-only install (for example, one running from the
  DMG) links to the release instead.
- **Windows x64:** A portable ZIP must have the original `Velocity` layout.
  The helper waits for the editor to exit and swaps the entire packaged folder.
  Keep projects outside that folder. Extra top-level files, Inno Setup installs
  and other unrecognized layouts link to the release for manual installation.
  The updater never runs setup silently or modifies the registry or PATH.

Trial startup must confirm after opening the recovered session. If it exits or
does not confirm within two minutes, the helper restores and starts the prior
version. Power loss, locked files, or a killed helper can leave an adjacent
`Velocity.app.old` or `<portable folder>.old`. Exit Velocity before recovering
manually; check the version of both folders, then move the unsuccessful trial
aside and restore the previous folder to its original name. The updater does
not overwrite an existing `.old` folder.

## Provision the signing key before the first updater release

The public-key list is intentionally empty until a maintainer provisions it.
The publish job fails if the repository secret is absent or mismatches all
embedded keys. Older releases without `SHA256SUMS.sig` cannot be installed by
the updater. Users on builds without this updater must install the first
updater-enabled release manually.

1. Use OpenSSL 3 (set `OPENSSL` to its executable if the system version is older).
   From the repo, write the private key **outside the checkout** and retain a
   secure offline backup. The command prints the raw public key in hex:

   ```sh
   sh scripts/update-signing.sh generate "$HOME/velocity-update-signing.pem"
   ```

2. Add only that 64-digit public key on its own line in
   `crates/vel-update/src/public-keys.hex`. Review, commit and merge the change
   before tagging the release. Never commit the PEM.
3. In repository **Settings › Secrets and variables › Actions**, set the
   repository secret `VELOCITY_UPDATE_SIGNING_KEY` to the entire PEM, including
   the boundary lines. With authenticated GitHub CLI, the equivalent is:

   ```sh
   gh secret set VELOCITY_UPDATE_SIGNING_KEY --repo mini-zoro/velocity < "$HOME/velocity-update-signing.pem"
   ```

4. Bump the workspace version and push its matching `v*` tag normally. The
   publish job signs the combined checksums and attaches both checksum files.
   To reproduce the detached signature locally and verify it, with the same
   OpenSSL 3 binary:

   ```sh
   "${OPENSSL:-openssl}" pkey -in "$HOME/velocity-update-signing.pem" -pubout -out update-public.pem
   sh scripts/update-signing.sh sign "$HOME/velocity-update-signing.pem" SHA256SUMS SHA256SUMS.sig
   sh scripts/update-signing.sh verify update-public.pem SHA256SUMS SHA256SUMS.sig
   shasum -a 256 -c SHA256SUMS
   ```

### Rotation

Publish a bridge build embedding **both** public keys while the workflow still
signs with the previous key. After clients have installed the bridge, replace
the secret with the new private key and publish using the new key. A later
release can remove the previous public key. Clients that miss the bridge will
not automatically follow the rotation because the updater only checks the
latest release; they must install a current release manually. If a private key
is compromised, remove the secret and stop publishing under that key; distribute
a trusted replacement out of band rather than trusting a bridge signed by it.

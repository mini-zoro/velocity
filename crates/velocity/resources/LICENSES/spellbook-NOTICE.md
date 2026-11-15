# Third-party spell engine

vel-spell uses unmodified spellbook 0.4.2, licensed under MPL-2.0. The complete license is
in spellbook-MPL-2.0.txt. Source for this exact dependency is available from the crates.io
spellbook 0.4.2 source archive:
https://static.crates.io/crates/spellbook/spellbook-0.4.2.crate
(upstream project: https://github.com/helix-editor/spellbook).
Cargo.lock pins the published crate checksum. No spellbook source was copied or modified.
Include its license and source-availability notice in binary distributions.

The English dictionary is a separate work with separate permissive terms. Its full notices
are in ../dictionaries/en_US/LICENSE, and pinned source/hashes in PROVENANCE.json beside it.
No French or other dependency-vendored dictionaries are shipped by vel-spell.

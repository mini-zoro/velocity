#!/usr/bin/env python3
"""Deterministic resource assembly and layout verification (Python standard library only)."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import struct
import subprocess


EXTENSIONS = "txt text md markdown rst rs c h cc cpp hpp m mm swift py js jsx ts tsx json yaml yml toml xml html css sh bash zsh sql go java rb php lua pl ini cfg conf log csv tsv tex cmake make sublime-project sublime-workspace".split()

ICON_CHUNKS = [
    (b"icp4", "icon_16x16.png", 16), (b"ic11", "icon_16x16@2x.png", 32),
    (b"icp5", "icon_32x32.png", 32), (b"ic12", "icon_32x32@2x.png", 64),
    (b"ic07", "icon_128x128.png", 128), (b"ic13", "icon_128x128@2x.png", 256),
    (b"ic08", "icon_256x256.png", 256), (b"ic14", "icon_256x256@2x.png", 512),
    (b"ic09", "icon_512x512.png", 512), (b"ic10", "icon_512x512@2x.png", 1024),
]


# UI languages with a shipped catalog (English is the source language).
LOCALIZATIONS = ("en", "zh-Hans")

def icon(args):
    # ICNS is a big-endian container of typed PNG chunks. Encode it directly so artwork
    # generation does not depend on iconutil/ImageIO's host-specific conversion behavior.
    chunks = []
    for kind, name, size in ICON_CHUNKS:
        data = (Path(args.iconset) / name).read_bytes()
        assert data[:8] == b"\x89PNG\r\n\x1a\n"
        assert struct.unpack(">II", data[16:24]) == (size, size)
        chunks.append(kind + struct.pack(">I", len(data) + 8) + data)
    body = b"".join(chunks)
    (Path(args.app) / "Contents/Resources/Velocity.icns").write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)


def copy_tree(source, destination):
    shutil.copytree(source, destination, dirs_exist_ok=True,
                    ignore=lambda _path, names: [n for n in names if n.startswith(".") or n == "tests"])


def document_types():
    """Offer text editing, generic-file opening and folder viewing as alternate handlers."""
    return [
        {
            "CFBundleTypeName": "Text and Source Code", "CFBundleTypeRole": "Editor",
            "LSHandlerRank": "Alternate", "CFBundleTypeExtensions": EXTENSIONS.copy(),
            "LSItemContentTypes": ["public.plain-text", "public.source-code", "public.text", "public.json"],
        },
        {
            "CFBundleTypeName": "Files", "CFBundleTypeRole": "Editor",
            "LSHandlerRank": "Alternate", "LSItemContentTypes": ["public.data"],
        },
        {
            "CFBundleTypeName": "Folders", "CFBundleTypeRole": "Viewer",
            "LSHandlerRank": "Alternate", "LSItemContentTypes": ["public.folder"],
        },
    ]


def validate_document_types(plist):
    """Require the native-open contract without changing default file ownership."""
    types = plist["CFBundleDocumentTypes"]
    for expected in document_types():
        for uti in expected["LSItemContentTypes"]:
            matches = [entry for entry in types if uti in entry.get("LSItemContentTypes", [])]
            assert matches, f"missing document type: {uti}"
            for entry in matches:
                assert entry["CFBundleTypeRole"] == expected["CFBundleTypeRole"], uti
                assert entry["LSHandlerRank"] == "Alternate", uti
                if "CFBundleTypeExtensions" in expected:
                    assert set(EXTENSIONS) <= set(entry.get("CFBundleTypeExtensions", [])), uti


def build_number(epoch):
    """CFBundleVersion: the source commit time. LaunchServices prefers the bundle with the
    highest CFBundleVersion when several copies share an identifier, so every package must
    carry a larger number than older ones (a constant 0.0.0 let it launch a stale copy
    that predates Finder open support)."""
    epoch = int(epoch)
    assert epoch >= 0, epoch
    return str(epoch)


def assemble(args):
    repo, app = Path(args.repo), Path(args.app)
    metadata = json.loads(Path(args.metadata).read_text())
    version = next(p["version"] for p in metadata["packages"] if p["name"] == "velocity")
    numeric_version = version.split("-")[0].split("+")[0]
    contents = app / "Contents"
    resources = contents / "Resources"
    for path in [contents / "MacOS", resources, contents / "SharedSupport/bin"]:
        path.mkdir(parents=True, exist_ok=True)
    plist = {
        "CFBundleDevelopmentRegion": "en", "CFBundleDisplayName": "Velocity",
        # AppKit localizes the menu items it adds itself (Services, Window tiling, Minimize All)
        # only into languages the bundle declares; keep in step with the UI catalogs.
        "CFBundleLocalizations": list(LOCALIZATIONS),
        "CFBundleExecutable": "velocity", "CFBundleIconFile": "Velocity.icns",
        "CFBundleIdentifier": "dev.velocity.Velocity", "CFBundleName": "Velocity",
        "CFBundlePackageType": "APPL", "CFBundleShortVersionString": numeric_version,
        "CFBundleVersion": build_number(args.epoch), "NSHighResolutionCapable": True,
        "LSMinimumSystemVersion": "14.0", "NSPrincipalClass": "NSApplication",
        "NSRequiresAquaSystemAppearance": False, "NSSupportsAutomaticTermination": False,
        "CFBundleDocumentTypes": document_types(),
    }
    (contents / "Info.plist").write_bytes(plistlib.dumps(plist, sort_keys=True))
    for language in LOCALIZATIONS:
        strings = resources / f"{language}.lproj/InfoPlist.strings"
        strings.parent.mkdir(parents=True, exist_ok=True)
        strings.write_text('"CFBundleDisplayName" = "Velocity";\n"CFBundleName" = "Velocity";\n', encoding="utf-8")
    shutil.copy2(args.binary, contents / "MacOS/velocity")
    shutil.copy2(repo / "crates/velocity/packaging/velocity", contents / "SharedSupport/bin/velocity")
    for path in [contents / "MacOS/velocity", contents / "SharedSupport/bin/velocity"]:
        path.chmod(0o755)
    copy_tree(repo / "third_party/sublime-packages", resources / "Syntaxes")
    copy_tree(repo / "crates/velocity/resources", resources)
    copy_tree(repo / "crates/vel-spell/dictionaries/en_US", resources / "Dictionaries/en_US")
    copy_tree(repo / "crates/vel-spell/LICENSES", resources / "LICENSES/vel-spell")
    for license_name in ["LICENSE-MIT", "LICENSE-APACHE"]:
        shutil.copy2(repo / license_name, resources / "LICENSES" / ("Velocity-" + license_name))
    shutil.copy2(repo / "crates/velocity/fixtures/ime-and-unicode.rs", resources / "ime-and-unicode.rs")
    info = {"version": version, "architectures": args.architectures.split(),
            "revision": args.revision, "source_date_epoch": int(args.epoch)}
    (resources / "build-info.json").write_text(json.dumps(info, indent=2, sort_keys=True) + "\n")


def manifest(args):
    app = Path(args.app)
    resources = app / "Contents/Resources"
    entries = {}
    for path in sorted(resources.rglob("*")):
        if path.is_file() and path.name != "bundle-manifest.json":
            data = path.read_bytes()
            entries[str(path.relative_to(resources))] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
    (resources / "bundle-manifest.json").write_text(json.dumps(entries, indent=2, sort_keys=True) + "\n")
    epoch = int(args.epoch)
    for path in sorted(app.rglob("*"), reverse=True) + [app]:
        os.utime(path, (epoch, epoch), follow_symlinks=False)


def validate(args):
    app = Path(args.app)
    contents, resources = app / "Contents", app / "Contents/Resources"
    plist = plistlib.loads((contents / "Info.plist").read_bytes())
    info = json.loads((resources / "build-info.json").read_text())
    assert plist["CFBundleIdentifier"] == "dev.velocity.Velocity"
    assert plist["CFBundleExecutable"] == "velocity" and plist["CFBundlePackageType"] == "APPL"
    assert plist["CFBundleShortVersionString"] == info["version"].split("-")[0].split("+")[0]
    assert plist["CFBundleVersion"] == build_number(info["source_date_epoch"])
    assert plist["NSHighResolutionCapable"] is True
    assert plist["LSMinimumSystemVersion"] == "14.0"
    assert plist["NSRequiresAquaSystemAppearance"] is False
    validate_document_types(plist)
    assert plist["CFBundleLocalizations"] == list(LOCALIZATIONS)
    for language in LOCALIZATIONS:
        assert (resources / f"{language}.lproj/InfoPlist.strings").stat().st_size > 0, language
    required = ["Velocity.icns", "Syntaxes/LICENSE", "Syntaxes/Rust/Rust.sublime-syntax",
                "Packages/Default/Preferences.sublime-settings",
                "Packages/Theme - Velocity/Velocity.sublime-theme",
                "Packages/Color Scheme - Velocity/Velocity Dark.sublime-color-scheme",
                "Dictionaries/en_US/en_US.dic", "Dictionaries/en_US/en_US.aff",
                "Dictionaries/en_US/LICENSE", "Dictionaries/en_US/PROVENANCE.json",
                "LICENSES/Velocity-LICENSE-MIT", "LICENSES/Velocity-LICENSE-APACHE",
                "LICENSES/vel-spell/NOTICE.md", "LICENSES/vel-spell/spellbook-MPL-2.0.txt"]
    for name in required:
        assert (resources / name).stat().st_size > 0, name
    assert (resources / "Velocity.icns").read_bytes()[:4] == b"icns"
    icon_bytes = (resources / "Velocity.icns").read_bytes()
    assert struct.unpack(">I", icon_bytes[4:8])[0] == len(icon_bytes)
    offset = 8
    for kind, _name, size in ICON_CHUNKS:
        assert icon_bytes[offset:offset + 4] == kind
        length = struct.unpack(">I", icon_bytes[offset + 4:offset + 8])[0]
        assert struct.unpack(">II", icon_bytes[offset + 24:offset + 32]) == (size, size)
        offset += length
    assert offset == len(icon_bytes)
    for name in ["MacOS/velocity", "SharedSupport/bin/velocity"]:
        assert os.access(contents / name, os.X_OK), name
    executable = str(contents / "MacOS/velocity")
    architectures = subprocess.check_output(["/usr/bin/lipo", "-archs", executable], text=True).split()
    assert set(architectures) == set(info["architectures"]), "Mach-O architectures differ from build info"
    load_commands = subprocess.check_output(["/usr/bin/otool", "-l", executable], text=True)
    minimums = re.findall(r"\bminos\s+(\d+(?:\.\d+)*)", load_commands)
    assert len(minimums) == len(architectures), "missing Mach-O deployment target"
    assert all(tuple(map(int, version.split(".")[:2])) == (14, 0) for version in minimums), minimums
    assert not any(path.name in {".git", ".github"} for path in app.rglob("*"))
    expected = json.loads((resources / "bundle-manifest.json").read_text())
    actual = {str(path.relative_to(resources)) for path in resources.rglob("*") if path.is_file() and path.name != "bundle-manifest.json"}
    assert actual == set(expected), "manifest file set differs"
    for name, entry in expected.items():
        data = (resources / name).read_bytes()
        assert len(data) == entry["bytes"] and hashlib.sha256(data).hexdigest() == entry["sha256"], name
    result = {"app": str(app.resolve()), "version": info["version"], "architectures": info["architectures"],
              "resource_files": len(expected), "app_bytes": sum(p.stat().st_size for p in app.rglob("*") if p.is_file()),
              "minimum_macos": plist["LSMinimumSystemVersion"], "mach_o_minimums": minimums,
              "layout": "passed", "resource_hashes": "passed"}
    print(json.dumps(result, indent=2, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["assemble", "icon", "manifest", "validate"])
    parser.add_argument("--app", required=True)
    parser.add_argument("--repo")
    parser.add_argument("--binary")
    parser.add_argument("--iconset")
    parser.add_argument("--metadata")
    parser.add_argument("--architectures", default="arm64")
    parser.add_argument("--revision", default="unknown")
    parser.add_argument("--epoch", default="0")
    args = parser.parse_args()
    {"assemble": assemble, "icon": icon, "manifest": manifest, "validate": validate}[args.mode](args)


if __name__ == "__main__":
    main()

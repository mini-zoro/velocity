"""Packaging contract tests; only the extension is built, in a disposable fixture."""
import copy
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

import bundle


class DocumentTypes(unittest.TestCase):
    def setUp(self):
        # Exercise the plist representation used by assemble, without packaging an app.
        self.plist = plistlib.loads(plistlib.dumps({
            "CFBundleDocumentTypes": bundle.document_types(),
        }))

    def test_text_data_and_folders_are_alternate_handlers(self):
        bundle.validate_document_types(self.plist)
        by_uti = {
            uti: entry for entry in self.plist["CFBundleDocumentTypes"]
            for uti in entry["LSItemContentTypes"]
        }
        self.assertEqual(set(by_uti), {
            "public.plain-text", "public.source-code", "public.text", "public.json",
            "public.data", "public.folder",
        })
        for uti, entry in by_uti.items():
            self.assertEqual(entry["LSHandlerRank"], "Alternate")
            self.assertEqual(entry["CFBundleTypeRole"], "Viewer" if uti == "public.folder" else "Editor")
        self.assertLessEqual(set(bundle.EXTENSIONS), set(by_uti["public.text"]["CFBundleTypeExtensions"]))

    def test_validator_rejects_missing_types_wrong_roles_and_default_rank(self):
        for index in range(3):
            with self.subTest(index=index, defect="missing"):
                invalid = copy.deepcopy(self.plist)
                invalid["CFBundleDocumentTypes"].pop(index)
                with self.assertRaises(AssertionError):
                    bundle.validate_document_types(invalid)
            for key, value in [("CFBundleTypeRole", "None"), ("LSHandlerRank", "Owner")]:
                with self.subTest(index=index, key=key):
                    invalid = copy.deepcopy(self.plist)
                    invalid["CFBundleDocumentTypes"][index][key] = value
                    with self.assertRaises(AssertionError):
                        bundle.validate_document_types(invalid)

    def test_validator_rejects_lost_extensions_and_conflicting_declarations(self):
        invalid = copy.deepcopy(self.plist)
        invalid["CFBundleDocumentTypes"][0]["CFBundleTypeExtensions"].remove("rs")
        with self.assertRaises(AssertionError):
            bundle.validate_document_types(invalid)
        invalid = copy.deepcopy(self.plist)
        invalid["CFBundleDocumentTypes"].append({
            "LSItemContentTypes": ["public.folder"],
            "CFBundleTypeRole": "Editor", "LSHandlerRank": "Owner",
        })
        with self.assertRaises(AssertionError):
            bundle.validate_document_types(invalid)


class BuildNumber(unittest.TestCase):
    def test_build_number_grows_with_the_source_commit_time(self):
        # LaunchServices launches the copy with the highest CFBundleVersion.
        self.assertEqual(bundle.build_number("1795070000"), "1795070000")
        self.assertGreater(int(bundle.build_number(1795070001)), int(bundle.build_number(1795070000)))
        self.assertEqual(bundle.build_number(0), "0")
        with self.assertRaises(AssertionError):
            bundle.build_number(-1)


class FinderExtension(unittest.TestCase):
    source = Path(__file__).parent / "finder-new-file"
    app_plist = {
        "CFBundleVersion": "1795070000", "CFBundleShortVersionString": "0.0.0",
        "LSMinimumSystemVersion": "14.0",
    }

    def test_url_and_extension_contract(self):
        self.assertEqual(bundle.url_types(), [{
            "CFBundleTypeRole": "Editor", "CFBundleURLName": "dev.velocity.Velocity",
            "CFBundleURLSchemes": ["velocity"],
        }])
        self.assertEqual(bundle.FINDER_APPEX, Path("Contents/PlugIns/VelocityFinderNewFile.appex"))
        plist = bundle.finder_plist(self.app_plist)
        self.assertEqual(plist["CFBundleIdentifier"], "dev.velocity.Velocity.FinderNewFile")
        self.assertEqual(plist["CFBundlePackageType"], "XPC!")
        self.assertEqual(plist["CFBundleDisplayName"], "Velocity")
        self.assertEqual(plist["CFBundleLocalizations"], ["en", "zh-Hans"])
        self.assertEqual(plist["CFBundleExecutable"], "VelocityFinderNewFile")
        self.assertEqual(plist["NSExtension"], {
            "NSExtensionPointIdentifier": "com.apple.FinderSync",
            "NSExtensionPrincipalClass": "VelocityFinderNewFile",
        })
        for key, value in self.app_plist.items():
            self.assertEqual(plist[key], value)
        self.assertEqual(plistlib.loads((self.source / "entitlements.plist").read_bytes()),
                         {"com.apple.security.app-sandbox": True})

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("swiftc"), "requires macOS swiftc")
    def test_compiled_extension_layout_and_signed_entitlements(self):
        if subprocess.run(["/usr/bin/xcrun", "--find", "swiftc"], capture_output=True).returncode:
            self.skipTest("no active Swift toolchain")
        with tempfile.TemporaryDirectory() as tmp:
            app = Path(tmp) / "Velocity.app"
            appex = app / "Contents/PlugIns/VelocityFinderNewFile.appex"
            architectures = ["arm64", "x86_64"]
            bundle.assemble_finder_extension(app, self.source, self.app_plist, architectures)
            for language, title in [("en", "New Text File"), ("zh-Hans", "新建文本文件")]:
                strings = appex / "Contents/Resources" / f"{language}.lproj/Localizable.strings"
                self.assertEqual(strings.read_text(encoding="utf-8"), f'"New Text File" = "{title}";\n')
            subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none",
                            "--entitlements", self.source / "entitlements.plist", appex], check=True)
            bundle.validate_finder_extension(app, self.app_plist, architectures)
            subprocess.run(["/usr/bin/codesign", "--verify", "--strict", appex], check=True)
            subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", appex], check=True)
            with self.assertRaisesRegex(AssertionError, "invalid Finder extension entitlements"):
                bundle.validate_finder_extension(app, self.app_plist, architectures)


if __name__ == "__main__":
    unittest.main()

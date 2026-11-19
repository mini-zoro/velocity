"""Headless plist contract tests; no build, signing, bundle assembly or GUI."""
import copy
import plistlib
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


if __name__ == "__main__":
    unittest.main()


class BuildNumber(unittest.TestCase):
    def test_build_number_grows_with_the_source_commit_time(self):
        # LaunchServices launches the copy with the highest CFBundleVersion.
        self.assertEqual(bundle.build_number("1795070000"), "1795070000")
        self.assertGreater(int(bundle.build_number(1795070001)), int(bundle.build_number(1795070000)))
        self.assertEqual(bundle.build_number(0), "0")
        with self.assertRaises(AssertionError):
            bundle.build_number(-1)

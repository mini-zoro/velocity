"""Device-free regression tests for settings reference coverage."""
from pathlib import Path
import runpy
import unittest

SCRIPT = runpy.run_path(str(Path(__file__).with_name("settings-reference.py")))


class SettingsReferenceTests(unittest.TestCase):
    def test_reader_discovery_keeps_production_after_tests(self):
        source = SCRIPT["production_rust"]("""
            // settings.bool("only_in_comment", false);
            let example = r#"settings.get("only_in_example")"#;
            fn first(settings: &Settings) {
                let n = |key, fallback| settings.get(key).unwrap_or(fallback);
                n("large_file.wrap_bytes", default);
                let alias = &editor.settings;
                alias.get("auto_complete_triggers");
                Settings::get(settings, "direct_associated");
                settings.string(if dark { "dark_theme" } else { "light_theme" }, "");
            }
            #[cfg(test)]
            mod tests {
                fn test_k() { settings.get("only_in_test"); }
            }
            fn later(settings: &Settings) {
                for key in ["file_exclude_patterns", "binary_file_patterns"] {
                    settings.get(key);
                }
                if let Some(value) = settings.layer(Layer::User).get("hot_exit") {
                    HotExit::from_setting(value);
                }
                map.get("not_a_settings_read");
            }
        """)
        self.assertEqual(SCRIPT["keyed_reads"](source), {
            "large_file.wrap_bytes", "auto_complete_triggers", "direct_associated",
            "dark_theme", "light_theme", "file_exclude_patterns",
            "binary_file_patterns", "hot_exit",
        })

    def test_typed_reads_require_an_editor_settings_receiver(self):
        source = """
            fn uses(options: &EditorSettings) { options.atomic_save; }
            editor.preferences.word_separators;
            let prefs = tab.editor.settings.editor();
            prefs.default_encoding;
            arbitrary_style.font_size;
        """
        fields = {"atomic_save", "word_separators", "default_encoding", "font_size"}
        self.assertEqual(SCRIPT["typed_reads"](source, fields), fields - {"font_size"})

    def test_unread_defaults_require_a_compatibility_reason(self):
        readers = {"works": {"some.rs"}, "reserved": set(), "orphan": set(), "no_reason": set()}
        notes = {
            "reserved": "Accepted for compatibility, no effect in Velocity: macOS has no such UI.",
            "no_reason": "Accepted for compatibility, no effect in Velocity.",
        }
        self.assertEqual(
            SCRIPT["uncovered_settings"](readers, notes, readers),
            ["no_reason", "orphan"],
        )


if __name__ == "__main__":
    unittest.main()

//! Inspect draw-list text without initializing the offscreen renderer.
use super::App;
use std::{collections::BTreeSet, path::Path};
use vel_render::scene::{Scene, TextRun};

#[derive(Debug, serde::Serialize)]
pub(super) struct AsciiRun<'a> {
    layer: usize,
    text: &'a str,
    words: Vec<&'a str>,
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/')))
        .map(|word| word.trim_matches('.'))
        .filter(|word| word.bytes().any(|c| c.is_ascii_alphabetic()))
}

// These are invariant product names, syntax names, technical names and encodings,
// not a general dictionary of English UI words. Mixed-script captions are still
// checked word by word. Shortcut key names such as F6 are handled separately.
const TECHNICAL: &[&str] = &[
    "Velocity", "UTF-8", "UTF-16", "BE", "LE", "BOM", "Unix", "Windows", "CRLF", "ASCII", "HTML",
    "API", "Python", "Rust", "JSON", "Markdown", "Big5", "GBK", "EUC-JP", "EUC-KR", "KOI8-R",
    "KOI8-U", "Shift", "JIS", "English", "KB", "MB", "GB",
];

fn literal_measurement(word: &str) -> bool {
    let duration = word.strip_suffix("ms").or_else(|| word.strip_suffix('s'));
    duration.is_some_and(|n| n.parse::<f64>().is_ok())
        || word
            .strip_prefix("0x")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn shortcut_word(word: &str, text: &str) -> bool {
    word.strip_prefix('F')
        .is_some_and(|n| n.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)))
        || (word.len() == 1 && text.contains(['⌘', '⌥', '⌃', '⇧']))
}

fn fixture_label(scene: &str, text: &str, word: &str, root: &Path) -> bool {
    match scene {
        // Goto Anything's details are styled a path component at a time.
        "goto_anything"
        | "r5_goto_anything"
        | "overlay_small"
        | "r41-goto-preview"
        | "r43-empty-folder-goto" => {
            if root
                .components()
                .any(|part| words(&part.as_os_str().to_string_lossy()).any(|w| w == word))
                || ["velocity", "render", "scenes", "src", "mod.rs", "rs"].contains(&word)
                || (scene == "r41-goto-preview" && word == "README")
                || root
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().split('-').next_back() == Some(word))
            {
                return true;
            }
        }
        // Scheme and theme pickers include literal resource paths as details.
        "scheme_picker" | "theme_picker" => {
            if [
                "Packages",
                "Color",
                "Scheme",
                "Theme",
                "Light",
                "Dark",
                "Default",
                "Velocity.sublime",
                "Dark.sublime",
                "Light.sublime",
                "tmTheme",
                "color",
                "scheme",
                "sublime",
                "sublime-theme",
                "sublime-color-scheme",
                "scheme-picker",
                "theme-picker",
                "color_scheme",
                "dark_color_scheme",
                "light_color_scheme",
                "dark_theme",
                "light_theme",
                "theme",
            ]
            .contains(&word)
            {
                return true;
            }
        }
        // These are author strings in minihtml fixtures, never app labels.
        "phantoms_popup_build_errors" | "phantom_selections" => {
            return [
                "Recursive",
                "helper",
                "Show",
                "callers",
                "Note",
                "Prints",
                "to",
                "the",
                "standard",
                "output",
                "with",
                "a",
                "newline",
                "Close",
                "println",
                "value",
                "Task",
                "test",
                "fn",
                "first",
                "expected",
                "types",
                "mismatched",
                "priority",
                "pop",
                "after",
                "recursion",
                "T",
                "x",
            ]
            .contains(&word);
        }
        // Raw subprocess output is intentionally not translated. Completion banners
        // render separately and are not included in these fixtures.
        "build_output" => {
            if [
                "   Compiling",
                "error[E0308]",
                "error: could not compile",
                "  -->",
                "55 |",
                "   |",
            ]
            .iter()
            .any(|prefix| text.starts_with(prefix))
            {
                return true;
            }
        }
        "r25_build_output" => {
            if matches!(
                text,
                "Compiling demo.rs" | "warning: unused variable: sample"
            ) {
                return true;
            }
        }
        // Search input, named syntax/scope data, and third-party snippet descriptions.
        "find_in_files"
        | "find_results_tab"
        | "find_in_files_short"
        | "r5_find_in_files"
        | "r25_find_in_files_ui2"
        | "zh_hans"
        | "zh_hans_ui2"
        | "zh_hans_mixed" => {
            if text == "<project>, *.rs" || word == "task" || word == "rs" {
                return true;
            }
        }
        "r5_palette_mixed" if text == "代码片段：Rust function" => return true,
        // Values of the fixture's text inputs, including English palette searches.
        "r5_palette_mixed" if text == "rust" => return true,
        "command_palette" if text == "toggle" => return true,
        "r5_palette" if text == "save" => return true,
        "r46-tab-rename-ime" if text == "ni hao.txt" => return true,
        "find" | "find_options_on" | "find_regex_error" | "r25_find_ui2" | "r5_find"
            if matches!(text, "task" | "(task") =>
        {
            return true;
        }
        "replace" | "replace_300_ui2" | "replace_scaled_narrow" | "narrow_640x480"
            if matches!(text, "task" | "job") =>
        {
            return true;
        }
        "r4_multiline_replace" | "r4_multiline_replace_ui2"
            if matches!(text, "canvas" | "job" | "parse" | "write") =>
        {
            return true;
        }
        "r5_narrow" if text == "neighbor" => return true,
        // Completion glyphs, sample words and built-in snippet content.
        "completion" | "completion_short" | "completion_ui2" => {
            return [
                "f", "i", "v", "s", "t", "test", "type", "version", "visiting",
            ]
            .contains(&word);
        }
        // Indexed symbol names and their single-letter badges.
        "goto_symbol" | "r5_goto_symbol" => {
            return ["Task", "after", "f", "fmt", "new", "s", "t"].contains(&word);
        }
        // The corrupt PNG fixture's hex inspector shows its literal bytes/ASCII.
        "image_fallback" => {
            return matches!(text, ".PNG....not real" | "ly an image     ")
                || (text.len() == 2 && text.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        "sidebar_narrow" if word == "Py" && text.ends_with("Py…") => return true,
        _ => {}
    }
    false
}

fn is_data(run: &TextRun, word: &str, known_labels: &BTreeSet<String>, source: &str) -> bool {
    // A code run in the editor may be split mid-token by styling or clipped.
    if run.family.is_none()
        && !run.text.trim().is_empty()
        && source.contains(run.text.trim().trim_end_matches('…'))
    {
        return true;
    }
    // The fixture's paths and document names (including an ellipsized form).
    if known_labels.contains(word) {
        return true;
    }
    run.text.contains('…') && known_labels.iter().any(|label| label.starts_with(word))
}

pub(super) fn runs<'a>(name: &str, app: &App, root: &Path, scene: &'a Scene) -> Vec<AsciiRun<'a>> {
    // A Latin-script catalog can legitimately paint ASCII. Keep the audit's
    // existing data exceptions and recognize vocabulary from translated copy.
    let tr = &app.workspace.translations;
    let localized_words: BTreeSet<_> = tr
        .catalog(tr.matched_language().as_str())
        .into_iter()
        .flat_map(|catalog| catalog.strings.iter())
        .filter(|(source, translation)| source != translation)
        .flat_map(|(_, translation)| {
            // Placeholder names are never drawn and must not whitelist words.
            translation
                .split('}')
                .flat_map(|part| words(part.split_once('{').map_or(part, |(literal, _)| literal)))
        })
        .collect();
    let sources = app
        .workspace
        .tabs
        .iter()
        .map(|tab| tab.editor.text.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let mut labels = BTreeSet::from_iter(
        [
            "project",
            "sample",
            "src",
            "render",
            "tests",
            "target",
            "User",
            "ScenePackage",
            "Plain",
            "Text",
            "Light",
            "Dark",
            // Standard git indicators and truncated codec at the narrow status width.
            "main",
            "M",
            "S2",
            "M1",
            "UT",
            "A",
            "C",
        ]
        .map(str::to_owned),
    );
    for tab in &app.workspace.tabs {
        labels.extend(words(&tab.name()).map(str::to_owned));
    }
    // Static fixture resources, query input, scope names, and command/setting keys
    // deliberately painted in development scenes. Keep this in step with fixtures.
    labels.extend(
        [
            "gitignore",
            "Cargo.toml",
            "README.md",
            "notes.txt",
            "main.rs",
            "lib.rs",
            "config.rs",
            "mod.rs",
            "canvas.rs",
            "schedule.rs",
            "logo.png",
            "damaged.png",
            "invisible.rs",
            "md",
            "an_unusually_long_module_name_for_truncation_checks.rs",
            "render/an_unusually_long_module_name_for_truncation_checks.rs",
            "src/main.rs",
            "src/lib.rs",
            "src/config.rs",
            "src/render/mod.rs",
            "src/render/canvas.rs",
            "src/render/an_unusually_long_module_name_for_truncation_checks.rs",
            "tests/schedule.rs",
            "visit",
            "schedule",
            "orders_dependencies",
            "source.rust",
            "meta.block.rust",
            "meta.function.rust",
            "meta.group.rust",
            "string.quoted.double.rust",
            "constant.other.placeholder.rust",
            "toml",
        ]
        .map(str::to_owned),
    );
    let labels = &labels;
    let sources = sources.as_str();
    let localized_words = &localized_words;
    let settings = app.workspace.settings_page.as_deref();
    let settings_path_label = settings.map(|_| tr.tr("User file: {path}"));
    let settings_path_label = settings_path_label
        .as_deref()
        .and_then(|label| label.split_once("{path}"));
    let root_text = root.to_string_lossy();
    let root_text = root_text.as_ref();
    scene
        .layers
        .iter()
        .enumerate()
        .flat_map(|(layer, entry)| {
            entry.text.iter().filter_map(move |run| {
                if settings.is_some_and(|page| page.is_row_data(&run.text, tr)) {
                    return None;
                }
                // The displayed path can start with ~, or omit intermediate components.
                // Only exempt the substituted value; the surrounding caption is UI chrome.
                let settings_path = settings_path_label.and_then(|(prefix, suffix)| {
                    run.text.strip_prefix(prefix)?.strip_suffix(suffix)
                });
                let words = words(&run.text)
                    .filter(|word| {
                        !TECHNICAL.contains(word)
                            && !localized_words.contains(word)
                            && !literal_measurement(word)
                            && !shortcut_word(word, &run.text)
                            && !is_data(run, word, labels, sources)
                            && !settings_path
                                .is_some_and(|path| words(path).any(|part| part == *word))
                            && !fixture_label(name, &run.text, word, root)
                            && !settings.is_some_and(|page| {
                                word.starts_with(root_text)
                                    || page.read_only().is_some_and(|detail| {
                                        run.text.ends_with(detail)
                                            && words(detail).any(|raw| raw == *word)
                                    })
                            })
                    })
                    .collect::<Vec<_>>();
                (!words.is_empty()).then_some(AsciiRun {
                    layer,
                    text: &run.text,
                    words,
                })
            })
        })
        .collect()
}

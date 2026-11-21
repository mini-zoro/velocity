#!/usr/bin/env python3
"""Regenerate the settings reference; --check detects stale docs."""
import argparse
import json
from pathlib import Path
import re
import tempfile
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[3]
DEFAULT = ROOT / "crates/velocity/resources/Packages/Default"
DOC = ROOT / "docs/settings.md"
UI_REFERENCE = ROOT / "crates/velocity/src/settings_reference.json"
BEGIN = "<!-- BEGIN GENERATED SETTINGS -->"
END = "<!-- END GENERATED SETTINGS -->"
NO_EFFECT = "accepted for compatibility, no effect in Velocity"
STRINGS = r'"(?:[^"\\]|\\[\s\S])*"'
CHAR = r"'(?:[^'\\\n]|\\(?:u\{[\da-fA-F_]+\}|x[\da-fA-F]{2}|.))'"
KEY = r'"([\w.]+)"'

# Used by both the Settings sheet and the generated reference below.
EFFECTS = {
    "next_window": ("new_window_settings",),
    "next_document": ("default_encoding", "default_line_ending"),
    "next_open": ("fallback_encoding", "detect_indentation", "large_file.load_bytes",
                  "open_files_in_new_window", "update_system_recent_files"),
    "next_save": ("atomic_save", "ensure_newline_at_eof_on_save", "default_dir",
                  "trim_trailing_white_space_on_save", "trim_only_modified_white_space",
                  "save_on_focus_lost"),
    "next_build": ("show_panel_on_build", "save_all_on_build", "result_file_regex",
                   "result_line_regex", "result_base_dir", "build_env"),
    "next_session": ("hot_exit", "remember_open_files"),
    "next_full_screen": ("use_simple_full_screen",),
    "next_vintage_view": ("vintage_start_in_command_mode",),
}

EFFECT_LABELS = {
    "immediate": "Immediately",
    "next_window": "Next window",
    "next_document": "Next new document",
    "next_open": "Next file open",
    "next_save": "Next save",
    "next_build": "Next build",
    "next_session": "Next close or quit",
    "next_full_screen": "Next full-screen transition",
    "next_vintage_view": "When a view enters Vintage",
}

# Background references belong in the documentation, not the shipped JSON editor
# or the Settings page. Keep these next to the generator for the Notes column.
SETTINGS_REFERENCE = "https://docs.sublimetext.io/reference/settings.html"
ARCHIVED_REFERENCE = "https://sublime-text-unofficial-documentation.readthedocs.io/en/latest/reference/settings.html"
CHANGELOG = "https://www.sublimetext.com/download"
NOTES = {
    "draw_white_space": f"[Visual settings]({ARCHIVED_REFERENCE}#visual-settings)",
    "highlight_line": f"[Visual settings]({ARCHIVED_REFERENCE}#visual-settings)",
    "match_selection": f"[Visual settings]({ARCHIVED_REFERENCE}#visual-settings)",
    "trim_only_modified_white_space": f"[Build 4107]({CHANGELOG})",
    "reload_file_on_change": f"[Build 4107]({CHANGELOG})",
    "reload_file_in_background": f"[Build 4200]({CHANGELOG}#build-4200)",
    "show_syntax": f"[Build 4107]({CHANGELOG}#build-4107)",
    "show_indentation": f"[Build 4107]({CHANGELOG}#build-4107)",
    "show_sidebar_button": "[Status bar theme elements](https://www.sublimetext.com/docs/themes.html#status-bar)",
    "scroll_past_end": f"[Build 4107]({CHANGELOG})",
    "folder_exclude_patterns": f"[Global settings]({SETTINGS_REFERENCE}#global-settings)",
    "indent_guide_options": "[Active-guide default](https://www.sublimetext.com/updates/2/stable/release_notes.html)",
    "always_show_minimap_viewport": f"[Visual settings]({SETTINGS_REFERENCE}#visual-settings)",
    "block_caret": f"[Settings reference]({SETTINGS_REFERENCE}); [archived reference]({ARCHIVED_REFERENCE})",
    "mouse_wheel_switches_tabs": f"[Global settings]({SETTINGS_REFERENCE}#global-settings)",
    "result_file_regex": "[Build system options](https://www.sublimetext.com/docs/build_systems.html#exec-target-options)",
    "update_system_recent_files": f"[Build 4143]({CHANGELOG})",
    "draw_unicode_white_space": f"[Builds 4107 and 4126]({CHANGELOG})",
    "image_file_patterns": (
        f"WebP uses macOS ImageIO; PSD is not supported. [Build 4180]({CHANGELOG}#build-4180) "
        "[Preview loader](../crates/velocity/src/image_decode.rs)"
    ),
    "index_exclude_patterns": "[Indexing settings](https://www.sublimetext.com/docs/indexing.html#settings)",
    "spelling_selector": f"[Scope selector]({ARCHIVED_REFERENCE}#system-and-miscellaneous-settings)",
    "ignored_packages": "[Enabling Vintage](https://www.sublimetext.com/docs/vintage.html#enabling-vintage)",
    "vintage_ctrl_keys": "[Vintage Ctrl keys](https://www.sublimetext.com/docs/vintage.html#ctrl-keys)",
}


def unique_object(pairs):
    obj = {}
    for key, value in pairs:
        if key in obj:
            raise ValueError(f"Duplicate JSON property: {key}")
        obj[key] = value
    return obj


def jsonc(source, *, unique=False):
    tokens = re.compile(STRINGS + r'|//[^\n]*|/\*[\s\S]*?\*/')
    semantic = tokens.sub(lambda m: m[0] if m[0].startswith('"') else "", source)
    commas = re.compile(STRINGS + r'|,(?=\s*[}\]])')
    semantic = commas.sub(lambda m: m[0] if m[0].startswith('"') else "", semantic)
    return json.loads(semantic, object_pairs_hook=unique_object if unique else None)


def settings_file(name):
    """Read JSONC and optional explanatory comments for the reference's notes."""
    source = (DEFAULT / name).read_text(encoding="utf-8-sig")
    descriptions = {}
    for match in re.finditer(r'^((?:  //[^\n]*\n)+)  "([^"]+)":', source, re.M):
        descriptions[match[2]] = " ".join(s[5:] for s in match[1].splitlines())
    return jsonc(source), descriptions


def production_rust(source):
    """Drop comments, raw fixture strings and individual test items, not the rest of the file."""
    noise = re.compile(
        r'\b(?:br|r)(?P<hashes>\#*)"[\s\S]*?"(?P=hashes)'
        r'|' + STRINGS + r'|' + CHAR + r'|//[^\n]*|/\*[\s\S]*?\*/'
    )
    source = noise.sub(lambda m: m[0] if m[0].startswith('"') else " ", source)
    test_item = re.compile(r'#\[(?:cfg\(test\)|test)\]\s*(?:#\[[^\n]*\]\s*)*'
                           r'(?:pub(?:\([^)]*\))?\s+)?(?:mod|fn)\b[^;{]*([;{])')
    braces = re.compile(STRINGS + r"|[{}]")
    pieces = []
    start = 0
    while match := test_item.search(source, start):
        pieces.append(source[start:match.start()])
        start = match.end()
        if match[1] == "{":
            depth = 1
            for token in braces.finditer(source, start):
                depth += (token[0] == "{") - (token[0] == "}")
                if depth == 0:
                    start = token.end()
                    break
            else:
                raise ValueError("unterminated Rust test item")
    return "\n".join([*pieces, source[start:]])


# .get alone is also common on Maps, JSON and collections. Only recognize it
# on settings, explicit Layers, or locally inferred settings reader closures.
GETTER = (
    r'(?:\.\s*(?:bool|usize|number|string|window_visible|global_bool|global_string|global_usize)'
    r'|\b(?:\w*settings|prefs)(?:\(\))?\s*(?:\.\s*layer\([^()]*\)\s*)?\.\s*get)\s*\(\s*'
    r'|\bSettings::(?:bool|usize|number|string|get)\s*\(\s*&?[\w.]+\s*,\s*'
)


def keyed_reads(source):
    getter = f"(?:{GETTER})"
    keys = set(re.findall(getter + KEY, source))
    for alias in re.findall(r'\blet\s+(\w+)\s*=\s*&[\w.]*\bsettings\s*;', source):
        keys.update(re.findall(r'\b' + re.escape(alias) + r'\s*\.\s*get\(\s*' + KEY, source))
    # Conditional first argument, e.g. selecting the dark or light theme.
    for left, right in re.findall(
        getter + r'if\s+\w+\s*\{\s*' + KEY + r'\s*\}\s*else\s*\{\s*' + KEY + r'\s*\}',
        source,
    ):
        keys.update((left, right))
    for match in re.finditer(r'\blet\s+(\w+)\s*=\s*\|\s*(\w+)\b[^|]*\|\s*([^;]+);', source):
        helper, parameter, body = match.groups()
        if re.search(getter + re.escape(parameter) + r'\b', body):
            keys.update(re.findall(r'\b' + re.escape(helper) + r'\(\s*' + KEY, source))
    for match in re.finditer(r'\bfor\s+(\w+)\s+in\s+\[([^\]]+)\]\s*\{([^{}]+)\}', source):
        name, values, body = match.groups()
        if re.search(getter + re.escape(name) + r'\b', body):
            keys.update(re.findall(KEY, values))
    return keys


def typed_reads(source, fields):
    # Editor.preferences and CommandHost.settings() both hold EditorSettings.
    settings_editor = r'\bsettings\s*\.\s*editor\(\)'
    keys = set(re.findall(
        r'(?:\.\s*preferences\b|' + settings_editor + r'|\bhost\.settings\(\))\s*\.\s*(\w+)',
        source,
    ))
    bindings = set(re.findall(r'\b(\w+)\s*:\s*&?(?:\w+::)*EditorSettings\b', source))
    bindings.update(re.findall(r'\blet\s+(\w+)\s*=\s*[\w.]*' + settings_editor, source))
    for name in bindings:
        keys.update(re.findall(r'\b' + re.escape(name) + r'\s*\.\s*(\w+)', source))
    return keys & fields


def read_sites(shipped):
    def add(key, path):
        if not key.startswith("velocity_"):
            readers.setdefault(key, set()).add(path.relative_to(ROOT))

    readers = {key: set() for key in shipped}
    config = ROOT / "crates/vel-config/src/settings.rs"
    fields = re.search(r'pub struct EditorSettings\s*\{([^}]+)\}', config.read_text())[1]
    typed = set(re.findall(r'^\s*pub (\w+):', fields, re.M))
    for path in (ROOT / "crates").glob("*/src/**/*.rs"):
        if path.stem.endswith(("_tests", "_test", "_bench", "_perf")) or path.stem == "tests":
            continue
        try:
            source = production_rust(path.read_text(encoding="utf-8"))
        except ValueError as exc:
            raise ValueError(f"{path.relative_to(ROOT)}: {exc}") from exc
        # Outside the app, other formats also have maps named "settings"
        # (e.g. syntax metadata and color schemes). They aren't preferences.
        preference_client = (
            path.relative_to(ROOT).parts[1] == "velocity"
            or re.search(r'\bvel_config::[^;]*\bSettings\b', source)
        )
        for key in keyed_reads(source):
            if key in shipped or preference_client:
                add(key, path)
        if path != config:
            for key in typed_reads(source, typed) & shipped:
                add(key, path)
                add(key, config)

    # Key lookup and interpretation can live in different crates. Link the
    # interpreter only if the scanner still finds a preference lookup.
    for key, consumer in {
        "hot_exit": ["vel-session/src/lib.rs"],
        "added_words": ["vel-spell/src/lib.rs"],
        "ignored_words": ["vel-spell/src/lib.rs"],
        "word_separators": ["vel-buffer/src/buffer.rs", "vel-buffer/src/ops.rs"],
        "sub_word_separators": ["vel-buffer/src/ops.rs"],
        "default_completions_selector": ["vel-complete/src/lib.rs"],
    }.items():
        if readers.get(key):
            for path in consumer:
                add(key, ROOT / "crates" / path)

    packages = DEFAULT.parent
    for path in packages.rglob("*.sublime-theme"):
        for rule in jsonc(path.read_text(encoding="utf-8-sig")).get("rules", []):
            for guard in rule.get("settings", ()):
                add(guard.lstrip("!"), path)
    for suffix in ("*.sublime-keymap", "*.sublime-mousemap"):
        for path in packages.rglob(suffix):
            for binding in jsonc(path.read_text(encoding="utf-8-sig")):
                for context in binding.get("context", ()):
                    name = context["key"]
                    if name.startswith("setting.") and name.removeprefix("setting.") in shipped:
                        add(name.removeprefix("setting."), path)
    return readers


def uncovered_settings(shipped, descriptions, readers):
    return sorted(key for key in shipped
                  if not readers[key] and not re.search(
                      re.escape(NO_EFFECT.lower()) + r'\s*[:.]\s*\S',
                      descriptions.get(key, "").lower()))


def ui_reference(defaults, notes):
    metadata = jsonc((DEFAULT / "settings-ui.json").read_text(encoding="utf-8"), unique=True)
    missing, extra = defaults.keys() - metadata.keys(), metadata.keys() - defaults.keys()
    if missing or extra:
        raise ValueError(f"Settings UI metadata: missing {sorted(missing)}, extra {sorted(extra)}")
    timing = unique_object((key, effect) for effect, keys in EFFECTS.items() for key in keys)
    if extra_timing := timing.keys() - defaults.keys():
        raise ValueError(f"Unshipped settings in effect table: {sorted(extra_timing)}")
    data = {}
    for key, meta in sorted(metadata.items()):
        for field in ("name", "description"):
            value = meta.get(field)
            if not isinstance(value, str) or not value.strip() or len(value.splitlines()) != 1:
                raise ValueError(f"{key}: needs a one-line {field} in Settings UI metadata")
        row = {"description": meta["description"], "effect": timing.get(key, "immediate")}
        if NO_EFFECT in notes.get(key, "").lower():
            row["compatibility"] = True
        data[key] = row
    return data


def inline(value):
    """One JSON value as a Markdown table cell, including literal pipes/backticks."""
    encoded = json.dumps(value, ensure_ascii=False, separators=(", ", ": ")).replace("|", r"\|")
    return f"`` {encoded} ``"


def reference(defaults, mac, notes, readers, ui):
    lines = [
        BEGIN,
        "## Shipped and recognized settings",
        "",
        "Generated by `crates/velocity/scripts/settings-reference.py`. A blank macOS",
        "column means the general default applies. Links point to runtime readers,",
        "including typed consumers, shipped theme guards and key-binding contexts.",
        "",
        "| Setting | General default | macOS override | Takes effect | Description | Notes |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for key, paths in sorted(readers.items()):
        default = inline(defaults[key]) if key in defaults else "Not shipped"
        platform = inline(mac[key]) if key in mac else ""
        row = ui.get(key)
        effect = EFFECT_LABELS[row["effect"]] if row else ""
        description = row["description"] if row else ""
        refs = ", ".join(
            f"[{path.parts[1]}/{path.name}](../{quote(path.as_posix())})" for path in sorted(paths)
        )
        details = " ".join(part for part in (notes.get(key), NOTES.get(key), refs)
                           if part and part != description)
        description, details = (value.replace("|", r"\|") for value in (description, details))
        lines.append(f"| `{key}` | {default} | {platform} | {effect} | {description} | {details} |")
    return "\n".join([*lines, END])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    defaults, notes = settings_file("Preferences.sublime-settings")
    mac, mac_notes = settings_file("Preferences (OSX).sublime-settings")
    notes = mac_notes | notes
    shipped = defaults.keys() | mac.keys()
    readers = read_sites(shipped)
    if missing := uncovered_settings(shipped, notes, readers):
        parser.exit(1, f"Shipped settings need a reader or an explicit '{NO_EFFECT}' note: "
                       f"{', '.join(missing)}\n")
    try:
        ui = ui_reference(defaults, notes)
    except ValueError as error:
        parser.exit(1, f"{error}\n")
    generated = reference(defaults, mac, notes, readers, ui)
    reference_fields = {key: {k: v for k, v in row.items() if k != "description"}
                        for key, row in ui.items()}
    data = "{\n" + ",\n".join(f"  {json.dumps(key)}: {json.dumps(row)}"
                              for key, row in reference_fields.items()) + "\n}\n"
    source = DOC.read_text(encoding="utf-8")
    if BEGIN in source:
        result = source[:source.index(BEGIN)] + generated + source[source.index(END) + len(END):]
    else:
        result = source.rstrip() + "\n\n" + generated + "\n"
    for path, content in [(DOC, result), (UI_REFERENCE, data)]:
        if path.exists() and path.read_text(encoding="utf-8") == content:
            continue
        if args.check:
            parser.exit(1, f"{path.relative_to(ROOT)} is stale; run crates/velocity/scripts/settings-reference.py\n")
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=path.parent, delete=False) as out:
            out.write(content)
            scratch = Path(out.name)
        scratch.chmod(path.stat().st_mode & 0o777 if path.exists() else 0o644)
        scratch.replace(path)


if __name__ == "__main__":
    main()

#!/bin/zsh
# Run on an awake, unlocked display outside the sandbox. --prepare-only never starts a GUI.
set -eu
cd -- "${0:A:h}/../../.."
VELOCITY_R11_PREPARE=0
if [[ ${1:-} == --prepare-only ]]; then VELOCITY_R11_PREPARE=1; shift; fi
VELOCITY_R11_BINARY=${1:-./target/release/velocity}
VELOCITY_R11_GUI=$(mktemp -d /tmp/velocity-r11-gui.XXXXXX)
cp "$VELOCITY_R11_BINARY" "$VELOCITY_R11_GUI/velocity"
mkdir -p "$VELOCITY_R11_GUI/profile/Packages/User"
cp crates/velocity/fixtures/user-grammars/* "$VELOCITY_R11_GUI/profile/Packages/User/"
cat > "$VELOCITY_R11_GUI/profile/Packages/User/Preferences.sublime-settings" <<'JSON'
{
  "vintage_enabled": true,
  "vintage_start_in_command_mode": true,
  "vintage_ctrl_keys": true,
  "vintage_use_clipboard": true,
  "show_minimap": true,
  "translate_tabs_to_spaces": true
}
JSON
cat > "$VELOCITY_R11_GUI/notes.vnote" <<'TEXT'
TODO "review Vintage and user grammars"
# These notes use an original JSON TextMate fixture.
one two three
one two three
你好 🦀 é
TEXT
cat > "$VELOCITY_R11_GUI/xml-notes.vxnote" <<'TEXT'
TODO XML grammar detection and picker entry
TEXT
cat > "$VELOCITY_R11_GUI/editor.rs" <<'TEXT'
fn main() {
println!("reindent this line with ==, then u");
}
TEXT
shasum -a 256 "$VELOCITY_R11_GUI/velocity" > "$VELOCITY_R11_GUI/binary.sha256"
print "Review directory: $VELOCITY_R11_GUI"
print 'Follow MANUAL_TEST.md → Round 11. This disposable profile starts in command mode.'
print "Live grammar: $VELOCITY_R11_GUI/profile/Packages/User/Velocity Notes.tmLanguage.json"
if [[ $VELOCITY_R11_PREPARE -eq 0 ]]; then
  MTL_DEBUG_LAYER=1 "$VELOCITY_R11_GUI/velocity" -n --log-input \
    --data-dir "$VELOCITY_R11_GUI/profile" --output "$VELOCITY_R11_GUI/input.jsonl" \
    "$VELOCITY_R11_GUI/editor.rs" "$VELOCITY_R11_GUI/xml-notes.vxnote" "$VELOCITY_R11_GUI/notes.vnote"
fi

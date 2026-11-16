#!/bin/zsh
# Run outside the sandbox, with an unlocked macOS 14+ desktop.
set -euo pipefail
VELOCITY_R13_APP=${1:?Usage: review-round13-gui.zsh /absolute/Velocity.app [fixture-directory]}
VELOCITY_R13_FIXTURE=${2:-/tmp/velocity-round13-review-$(date +%s)}
VELOCITY_R13_EXE="$VELOCITY_R13_APP/Contents/MacOS/velocity"
[[ -x "$VELOCITY_R13_EXE" ]] || { print -u2 'Expected a packaged Velocity.app'; exit 1; }
mkdir -p "${VELOCITY_R13_FIXTURE:h}"
mkdir "$VELOCITY_R13_FIXTURE" # Refuse to replace a previous review profile.
python3 - "$VELOCITY_R13_FIXTURE" <<'PY'
import json
from pathlib import Path
import sys
import zipfile
root = Path(sys.argv[1]).resolve()
(root / 'project').mkdir(exist_ok=True)
(root / 'project/voiceover.txt').write_text('VoiceOver fixture\nHello 你好 😀 e\u0301\nPrimary selection and reversed selection\nFourth line\n')
(root / 'project/windowed.txt').write_text('你好 😀 e\u0301 windowed document line\n' * 50_000)
(root / 'project/second.txt').write_text('Second tab and sidebar entry\n')
(root / 'profile/Packages/User').mkdir(parents=True, exist_ok=True)
(root / 'profile/Packages/User/Preferences.sublime-settings').write_text('''{
  "font_size": 16,
  "word_wrap": true,
  "show_side_bar": true,
  "ignored_packages": ["KeepDisabled"],
  "package_index_url": null,
}
''')
(root / 'review.sublime-project').write_text(json.dumps({'folders': [{'path': str(root / 'project')}]}))
with zipfile.ZipFile(root / 'Reviewer Fixture.sublime-package', 'w') as archive:
    archive.writestr('package-metadata.json', '{"version":"1.0.0"}')
    archive.writestr('reviewer.sublime-syntax', 'name: Reviewer Fixture\nfile_extensions: [r13]\nscope: source.reviewer-fixture\ncontexts: {main: []}\n')
    archive.writestr('reviewer.sublime-snippet', '<snippet><content>fixture ${1:field}</content><tabTrigger>r13</tabTrigger><description>Reviewer Fixture</description></snippet>')
    archive.writestr('reviewer.sublime-build', '{"cmd":["/usr/bin/true"]}')
    archive.writestr('plugin.py', "raise AssertionError('This file must never execute')")
print('Fixture:', root)
print('See crates/velocity/MANUAL_TEST.md, Round 13 / VoiceOver.')
PY
VELOCITY_GUI_PROCESS=1 \
  "$VELOCITY_R13_EXE" --data-dir "$VELOCITY_R13_FIXTURE/profile" \
  --project "$VELOCITY_R13_FIXTURE/review.sublime-project" \
  "$VELOCITY_R13_FIXTURE/project/voiceover.txt" "$VELOCITY_R13_FIXTURE/project/second.txt"

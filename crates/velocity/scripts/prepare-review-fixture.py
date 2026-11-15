#!/usr/bin/env python3
"""Prepare a disposable Git/diff/spelling project and isolated Velocity profile."""
import argparse
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--profile', type=Path, required=True)
    args = parser.parse_args()
    root = args.output_dir.resolve()
    root.mkdir(parents=True, exist_ok=False)

    def git(*argv, source=None):
        return subprocess.check_output(['git', *argv], cwd=root, input=source, text=True).strip()

    def put(path, text):
        destination = root/path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(text)

    put('.gitignore', 'ignored/\n')
    put('main.rs', '// A mispelled comment. Another speling error.\nfn main() {\n    let identifierSpeling = "a mispelled string";\n    println!("hello 你好");\n}\n')
    for name in ['missing.txt', 'staged-mod.txt', 'staged-delete.txt', 'conflict.txt', 'mixed/modified.txt']:
        put(name, 'saved line\nsecond line\n')
    git('init', '-q')
    git('add', '.')
    git('-c', 'user.name=Velocity Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'review fixture')
    put('main.rs', (root/'main.rs').read_text().replace('hello 你好', 'hello revised 你好')+'// Added line with mispeling.\n')
    (root/'missing.txt').unlink()
    put('staged-mod.txt', 'staged changed line\n')
    git('add', 'staged-mod.txt')
    git('rm', '-q', 'staged-delete.txt')
    put('staged-add.txt', 'staged addition\n')
    git('add', 'staged-add.txt')
    put('untracked.txt', 'untracked\n')
    put('ignored/ignored.txt', 'ignored but visible and dimmed\n')
    put('mixed/modified.txt', 'changed\n')
    base = git('hash-object', '-w', '--stdin', source='base\n')
    ours = git('hash-object', '-w', '--stdin', source='ours\n')
    theirs = git('hash-object', '-w', '--stdin', source='theirs\n')
    git('update-index', '--index-info', source=f'0 {"0"*40}\tconflict.txt\n100644 {base} 1\tconflict.txt\n100644 {ours} 2\tconflict.txt\n100644 {theirs} 3\tconflict.txt\n')
    put('conflict.txt', '<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\n')
    project = {'folders':[{'path':'.'}], 'settings': {'spell_check': True, 'mini_diff': True, 'git_diff_target':'index', 'spelling_selector':'comment, string', 'auto_complete':False}}
    project_path = root/'review.sublime-project'
    project_path.write_text(json.dumps(project, indent=2)+'\n')
    user = args.profile.resolve()/'Packages/User'
    user.mkdir(parents=True, exist_ok=True)
    preferences = user/'Preferences.sublime-settings'
    if preferences.exists():
        raise SystemExit('Profile preferences already exist; choose a fresh profile.')
    preferences.write_text(json.dumps({'spell_check':True, 'mini_diff':True, 'show_git_status':True,
        'show_git_status_in_status_bar':True, 'sublime_merge_path':''}, indent=2)+'\n')
    (user/'review.aff').write_text('SET UTF-8\n')
    (user/'review.dic').write_text('3\nVelocity\ncustomword\nfixtureword\n')
    print(json.dumps({'project':str(project_path),'profile':str(args.profile.resolve()),'file':str(root/'main.rs'),
                      'git_status':git('status','--short')}, indent=2))


if __name__=='__main__':
    main()

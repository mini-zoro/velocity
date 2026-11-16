#!/usr/bin/env python3
"""Strict headless round-13 checks and twenty default-parallel workspace test passes."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[3]
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ)
    env.pop('RUST_TEST_THREADS', None)
    report = {'baseline': '85e6cc1', 'default_parallelism': True,
              'test_thread_override': None, 'consecutive_workspace_passes': 0,
              'gui_started': False, 'gpu_started': False, 'committed': False,
              'commands': [], 'passed': False}
    def fingerprint():
        extensions = {'.rs', '.toml', '.sublime-settings', '.sublime-keymap', '.sublime-commands', '.sublime-menu', '.png'}
        paths = [p for p in (repo / 'crates').rglob('*') if p.is_file() and p.suffix in extensions]
        paths += [repo / 'Cargo.lock', repo / '.github/workflows/ci.yml']
        return {str(p.relative_to(repo)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(paths)}
    report['source_sha256'] = fingerprint()
    report['runner_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()

    def write_report():
        (output / 'validation.json').write_text(json.dumps(report, indent=2) + '\n')

    def run(label, command, extra=None):
        log = output / (label + '.log')
        at = time.monotonic()
        print(label, 'running:', ' '.join(command), flush=True)
        with log.open('w') as stream:
            code = subprocess.run(command, cwd=repo, env=env | (extra or {}),
                                  stdout=stream, stderr=subprocess.STDOUT,
                                  timeout=900).returncode
        contents = log.read_text()
        entry = {'label': label, 'command': command, 'exit_code': code,
                 'elapsed_seconds': time.monotonic() - at, 'log': str(log),
                 'sha256': hashlib.sha256(log.read_bytes()).hexdigest()}
        if label.startswith('workspace-'):
            entry['test_totals'] = {key: sum(map(int, re.findall(r'(\d+) ' + key, contents)))
                                    for key in ['passed', 'failed', 'ignored']}
            entry['ipc_passed'] = ('test instance::tests::fresh_profile_socket_forwarding_waits_for_requested_sheet_close ... ok' in contents)
            if not entry['ipc_passed']:
                code = 1
        report['commands'].append(entry)
        write_report()
        print(label, 'exit', code, 'seconds', round(entry['elapsed_seconds'], 2), flush=True)
        if code:
            print(contents[-12000:], flush=True)
            raise SystemExit(code)
        return entry

    run('fmt', ['cargo', 'fmt', '--all', '--', '--check'])
    run('clippy', ['cargo', 'clippy', '--workspace', '--all-targets', '--locked', '--', '-D', 'warnings'])
    previous = fingerprint()
    attempts = 0
    while report['consecutive_workspace_passes'] < 20:
        before = fingerprint()
        if before != previous:
            report['consecutive_workspace_passes'] = 0
        attempts += 1
        entry = run(f'workspace-{attempts:02}', ['cargo', 'test', '--workspace', '--locked', '--release'])
        after = fingerprint()
        entry['sources_unchanged'] = before == after
        if before == after:
            report['consecutive_workspace_passes'] += 1
        else:
            report['consecutive_workspace_passes'] = 0
        previous = after
        report['source_sha256'] = after
        write_report()
    run('rustdoc', ['cargo', 'doc', '--workspace', '--no-deps', '--locked'],
        {'RUSTDOCFLAGS': '-D warnings'})
    run('release-build', ['cargo', 'build', '--release', '--locked', '-p', 'velocity'])
    report['passed'] = True
    write_report()


if __name__ == '__main__':
    main()

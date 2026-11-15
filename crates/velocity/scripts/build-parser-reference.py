#!/usr/bin/env python3
"""Build an isolated accepted editor with only the shared parser measurement hook added."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def main():
    repo = Path(__file__).resolve().parents[3]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--revision', default='5b128ba')
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--target-dir', type=Path)
    args = parser.parse_args()
    root = args.output_dir.resolve()
    root.mkdir(parents=True, exist_ok=False)
    revision = subprocess.check_output(['git','rev-parse',args.revision],cwd=repo,text=True).strip()
    archive = subprocess.Popen(['git','archive',revision],cwd=repo,stdout=subprocess.PIPE)
    try:
        subprocess.run(['tar','-x','-C',str(root)],stdin=archive.stdout,check=True)
    finally:
        archive.stdout.close()
    if archive.wait():
        raise RuntimeError('git archive failed')
    shutil.copytree(repo/'third_party/sublime-packages',root/'third_party/sublime-packages',
                    dirs_exist_ok=True,ignore=shutil.ignore_patterns('.git'))
    probe = repo/'crates/velocity/src/parse_bench.rs'
    shutil.copy2(probe,root/'crates/velocity/src/parse_bench.rs')
    path = root/'crates/velocity/src/cli.rs'
    source = path.read_text()
    if 'HeadlessParse' in source:
        raise RuntimeError('reference already has the probe; choose the accepted round-7 revision')
    for old,new in [
        ('    HeadlessWorkbench,','    HeadlessWorkbench,\n    HeadlessParse,'),
        ('            Self::HeadlessWorkbench => "headless-workbench",',
         '            Self::HeadlessWorkbench => "headless-workbench",\n            Self::HeadlessParse => "headless-parse-memory",'),
        ('                "--headless-workbench" => Some(Mode::HeadlessWorkbench),',
         '                "--headless-workbench" => Some(Mode::HeadlessWorkbench),\n                "--headless-parse-memory" => Some(Mode::HeadlessParse),')]:
        if old not in source:
            raise RuntimeError('reference CLI differs from the expected accepted editor')
        source = source.replace(old,new,1)
    path.write_text(source)
    path = root/'crates/velocity/src/main.rs'
    source = path.read_text().replace('mod panels;','mod panels;\nmod parse_bench;',1)
    marker = '        if options.mode == Mode::HeadlessFeatures {'
    if marker not in source:
        raise RuntimeError('reference main differs from the expected accepted editor')
    source = source.replace(marker,'        if options.mode == Mode::HeadlessParse { return parse_bench::run(&reporter,&options); }\n'+marker,1)
    path.write_text(source)
    target = args.target_dir.resolve() if args.target_dir else root/'target'
    subprocess.run(['cargo','build','--release','-p','velocity','--manifest-path',str(root/'Cargo.toml'),
                    '--target-dir',str(target)],cwd=repo,check=True)
    binary = target/'release/velocity'
    record = {'revision':revision,'probe_sha256':hashlib.sha256(probe.read_bytes()).hexdigest(),
              'binary':str(binary),'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
              'instrumented_files':['crates/velocity/src/cli.rs','crates/velocity/src/main.rs',
                                    'crates/velocity/src/parse_bench.rs'],
              'repository_history_changed':False}
    (root/'parser-reference.json').write_text(json.dumps(record,indent=2)+'\n')
    print(json.dumps(record,indent=2))


if __name__=='__main__':
    main()

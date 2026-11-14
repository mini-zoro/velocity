#!/usr/bin/env python3
"""Compare old styling and new scopes over identical complete-LF parser input, five paired runs."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[3]
reference = Path(tempfile.mkdtemp(prefix='velocity-highlight-reference-', dir='/private/tmp'))
source = subprocess.check_output(['git', 'show', '968f120:crates/vel-highlight/src/lib.rs'], cwd=ROOT, text=True)
source = source.replace('self.text.line(row).to_string()', 'self.text.slice(line_range(&self.text,row)).to_string()')
source = source.replace('self.text.line(n).to_string()', 'self.text.slice(line_range(&self.text,n)).to_string()')
source = source.replace('self.text.line(row).chunks()', 'self.text.slice(line_range(&self.text,row)).chunks()')
source = source.replace('self.text.line(row).len_bytes()', 'line_range(&self.text,row).len()')
source += '''
fn line_range(text: &Text, row: usize) -> Range<usize> {
    let start=text.line_to_byte(row);
    let end=if row+1<text.len_lines() { text.line_to_byte(row+1) } else { text.len_bytes() };
    start..end
}
'''
(reference / 'src').mkdir()
(reference / 'src/lib.rs').write_text(source)
manifest = '''[package]
name="velocity-highlight-round6-reference"
version="0.0.0"
edition="2024"
[workspace]
[lib]
name="before"
[dependencies]
thiserror="2"
'''
for name in ['vel-color-scheme', 'vel-syntax', 'vel-scope', 'vel-text', 'vel-buffer', 'vel-highlight']:
    manifest += f'{name}={{path={json.dumps(str(ROOT / "crates" / name))}}}\n'
(reference / 'Cargo.toml').write_text(manifest)
main = r'''
use std::{sync::Arc,time::Instant,path::PathBuf};
use vel_buffer::ChangeId;
use vel_color_scheme::ColorScheme;
use vel_text::Text;
use vel_syntax::SyntaxSet;
fn old(set:&Arc<SyntaxSet>,text:&Text,scheme:&Arc<ColorScheme>)->f64 {
    let mut engine=before::DocumentHighlighter::new(text.clone(),ChangeId(0),set.clone(),set.find_syntax_by_name("Rust").unwrap(),scheme.clone());
    let start=Instant::now(); engine.finish().unwrap(); start.elapsed().as_secs_f64()
}
fn new(set:&Arc<SyntaxSet>,text:&Text,scheme:&Arc<ColorScheme>)->f64 {
    let mut engine=vel_highlight::DocumentHighlighter::new(text.clone(),ChangeId(0),set.clone(),set.find_syntax_by_name("Rust").unwrap(),scheme.clone());
    let start=Instant::now(); engine.finish().unwrap(); start.elapsed().as_secs_f64()
}
fn main() {
    let root=PathBuf::from("REPOSITORY_ROOT");
    let source=std::fs::read_to_string(root.join("spikes/data/rust-10mb.rs")).unwrap();
    let text=Text::from(source.as_str());
    let set=Arc::new(SyntaxSet::load_cached(&root.join("third_party/sublime-packages"),&std::env::temp_dir().join("velocity-lane-b-r6-syntax-cache")).unwrap().0);
    let scheme=Arc::new(ColorScheme::velocity_dark());
    old(&set,&text,&scheme); new(&set,&text,&scheme);
    for run in 0..5 {
        let (before,after)=if run%2==0 { (old(&set,&text,&scheme),new(&set,&text,&scheme)) } else { let after=new(&set,&text,&scheme); (old(&set,&text,&scheme),after) };
        println!("{{\"run\":{run},\"before_complete_lf_s\":{before},\"after_scopes_s\":{after}}}");
    }
}
'''
(reference / 'src/main.rs').write_text(main.replace('REPOSITORY_ROOT', str(ROOT)))
environment = dict(os.environ)
environment['CARGO_TARGET_DIR'] = str(ROOT / 'target')
subprocess.run(['cargo', 'build', '--release', '--manifest-path', str(reference / 'Cargo.toml'), '--offline'], cwd=ROOT, env=environment, check=True)
result = subprocess.run([str(ROOT / 'target/release/velocity-highlight-round6-reference')], cwd=ROOT, text=True, capture_output=True, check=True)
records = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
path = ROOT / 'crates/vel-highlight/round6-comparison.json'
temporary = path.with_suffix('.tmp')
temporary.write_text(json.dumps(records, indent=2) + '\n')
temporary.replace(path)
print(result.stdout, end='')

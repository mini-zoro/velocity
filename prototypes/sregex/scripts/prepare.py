#!/usr/bin/env python3
"""Only reads Packages + the four explicitly allowed generated corpora."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPIKES = ROOT.parent.parent / "spikes" / "data"
PACKAGES = SPIKES / "Packages"
DATA = ROOT / "data"
DATA.mkdir(exist_ok=True)

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

manifest = {}
for language, filename in [("rust", "rust-10mb.rs"), ("javascript", "javascript-10mb.js"),
                           ("json", "json-20mb.json"), ("markdown", "markdown-5mb.md")]:
    path = SPIKES / filename
    if not path.exists():
        # Fall back to official syntax-test source, exactly as for missing languages.
        directory = {"rust":"Rust", "javascript":"JavaScript", "json":"JSON", "markdown":"Markdown"}[language]
        sources = sorted((PACKAGES / directory).rglob("syntax_test*"))
        text = "\n".join(p.read_text(errors="strict") for p in sources if p.is_file())
        assert text, language
        path = DATA / filename
        path.write_text((text * (10_000_000 // len(text.encode()) + 1))[:10_000_000])
    manifest[language] = {"path":str(path), "bytes":path.stat().st_size, "sha256":digest(path), "source":"existing generated corpus"}

for language, directory, suffix in [("python", "Python", "py"), ("cpp", "C++", "cpp"),
                                     ("yaml", "YAML", "yaml"), ("html", "HTML", "html")]:
    sources = sorted(p for p in (PACKAGES / directory).rglob("syntax_test*") if p.is_file())
    kept = []
    for source in sources:
        for line in source.read_text().splitlines(keepends=True):
            stripped = line.lstrip()
            # Keep actual source/comments, discard Sublime scope-assertion lines.
            assertion = (stripped.startswith(("#", "//", "<!--", "*"))
                         and ("SYNTAX TEST" in line or "<-" in line or "^" in line))
            if not assertion:
                kept.append(line)
    seed = "".join(kept).encode()
    assert seed and sources, language
    # Repeat whole seeds/lines; never truncate inside UTF-8 or a line.
    repetitions = max(1, (5_000_000 + len(seed) - 1) // len(seed))
    path = DATA / f"{language}-5mb.{suffix}"
    path.write_bytes(seed * repetitions)
    manifest[language] = {"path":str(path), "bytes":path.stat().st_size, "sha256":digest(path),
                          "source":"official syntax tests, scope assertions stripped, repeated whole seeds",
                          "seed_bytes":len(seed), "repetitions":repetitions,
                          "sources":[str(s.relative_to(PACKAGES)) for s in sources]}

(ROOT / "output" / "corpora.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
print(json.dumps({k:{"bytes":v["bytes"], "sha256":v["sha256"]} for k,v in manifest.items()}, indent=2))

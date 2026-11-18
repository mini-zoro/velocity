#!/usr/bin/env python3
"""Refresh the checked-in lookup tables; no network access is needed at build/run time."""

import argparse
from collections import defaultdict
from hashlib import sha256
from html.parser import HTMLParser
import io
import json
from pathlib import Path
import re
import tarfile
from urllib.request import urlopen

HTML_REV = "823898c0c763fb4026c1518208e1b60d7c18315e"  # 2026-11-17
HTML = f"https://html.spec.whatwg.org/commit-snapshots/{HTML_REV}/"
HTML_SHA256 = "f729a1298a82a64702ac1df6fc41e9264befaa6e53037212b6ca244a43f0ce53"
WEBREF_VERSION = "8.7.9"
WEBREF_REV = "b6b162291383bf6b2edb826114ab9dada7349dd8"  # npm release 2026-11-13
WEBREF = f"https://registry.npmjs.org/@webref/css/-/css-{WEBREF_VERSION}.tgz"
WEBREF_SHA256 = "0f90d9b103922a94bc23c6324c5c574b03caab5882f65c58f74d83cdf2d05a3f"
DEST = Path(__file__).resolve().parents[1] / "src" / "markup_data.rs"
NAME = re.compile(r"-?[a-z][a-z0-9-]*\Z")
HTML_KEYWORD = re.compile(r'"([-_a-zA-Z0-9:/.+]+)"')

# WHATWG HTML, Writing HTML Documents § Elements (#void-elements).
VOID = "area base br col embed hr img input link meta source track wbr".split()


class Tables(HTMLParser):
    """Read rows and cells, including the optional end tags used in living standards."""

    def __init__(self, data):
        super().__init__()
        self.tables = []
        self.parents = []
        self.caption = None
        self.table = self.row = self.cell = None
        self.feed(data.decode("utf-8"))
        self.close()

    def handle_starttag(self, tag, attrs):
        if tag == "table":
            self.parents.append((self.table, self.row, self.cell, self.caption))
            self.table = []
            self.caption = []
            self.tables.append((dict(attrs).get("id"), self.caption, self.table))
            self.row = self.cell = None
        elif self.table is not None:
            if tag == "caption":
                self.cell = self.caption
            elif tag == "tr":
                self.row = []
                self.table.append(self.row)
                self.cell = None
            elif tag in {"td", "th"} and self.row is not None:
                self.cell = []
                self.row.append(self.cell)

    def handle_endtag(self, tag):
        if tag == "table" and self.parents:
            self.table, self.row, self.cell, self.caption = self.parents.pop()
        elif tag == "tr":
            self.row = self.cell = None
        elif tag in {"td", "th", "caption"}:
            self.cell = None

    def handle_data(self, data):
        if self.cell is not None:
            self.cell.append(data)

    def with_heading(self, heading, *, table_id=None, caption=None, contains=()):
        for ident, title, table in self.tables:
            if table_id is not None and ident != table_id:
                continue
            if caption is not None and " ".join("".join(title).split()) != caption:
                continue
            rows = [[" ".join("".join(c).split()) for c in r] for r in table]
            if rows and heading in rows[0] and set(contains) <= {r[0] for r in rows if r}:
                return rows[1:]
        raise ValueError(f"Missing table with column {heading!r}")


def html_data(data):
    index = Tables(data)
    elements = defaultdict(set)
    for row in index.with_heading("Element", caption="List of elements"):
        if len(row) >= 6 and NAME.fullmatch(row[0]):
            # "globals" is a reference, not an attribute. HTML_GLOBAL below
            # supplies its expansion once for all elements.
            elements[row[0]].update(a for a in row[5].split("; ") if a != "globals" and NAME.fullmatch(a))
    globals_ = set()
    values = defaultdict(set)
    for row in (index.with_heading("Attribute", table_id="attributes-1")
                + index.with_heading("Attribute", table_id="ix-event-handlers")):
        if len(row) != 4 or not NAME.fullmatch(row[0]):
            continue
        attr, owners, _, value = row
        keywords = HTML_KEYWORD.findall(value)
        if "HTML elements" in owners:
            globals_.add(attr)
            values[f"*:{attr}"].update(keywords)
        for owner in owners.split("; "):
            if owner in elements:
                elements[owner].add(attr)
                values[f"{owner}:{attr}"].update(keywords)
    assert set(VOID) <= elements.keys() and len(elements) > 100 and "class" in globals_
    # The attribute index points to this keyword/state table instead of listing
    # individual values for input[type].
    values["input:type"].update(row[0] for row in index.with_heading(
                                "Keyword", contains=("text", "email", "checkbox"))
                                if row and NAME.fullmatch(row[0]))
    assert {"text", "email", "checkbox"} <= values["input:type"] and "onclick" in globals_
    return elements, globals_, values


# CSS Values & Units: expand named productions, and keep only bare keyword tokens
# (no functions, unit ranges or snippets). CSS-wide defaults are added at query time.
TOKEN = re.compile(r"<([^<>]+)>|'[^']*'|\"[^\"]*\"|(-?[a-z][a-z0-9-]*)(\()?", re.I)


def css_data(webref):
    rules = defaultdict(set)
    props = defaultdict(set)
    with tarfile.open(fileobj=io.BytesIO(webref), mode="r:gz") as archive:
        spec = json.load(archive.extractfile("package/css.json"))
    for group, table in (("properties", props), ("types", rules)):
        for entry in spec[group]:
            if entry.get("syntax"):
                table[entry["name"]].add(entry["syntax"])

    def keywords(grammar, seen):
        for typ, ident, call in TOKEN.findall(grammar):
            if typ:
                quoted = typ.startswith("'")
                ref = (quoted, typ.strip("'"))
                if ref not in seen:
                    for rule in (props if quoted else rules).get(ref[1], ()):
                        yield from keywords(rule, seen | {ref})
            elif ident and not call:
                yield ident

    # The curated WebRef property inventory tracks current specs. Unlike the
    # historical CSSWG combined index, it does not contain the old CSS2 aural
    # properties (azimuth, pitch, richness). There is no per-property status
    # field in this distribution; presence in the curated list is the filter.
    values = {name: set().union(*(keywords(rule, {(True, name)}) for rule in props[name]))
              for name in props if NAME.fullmatch(name)}
    assert len(values) > 400 and {"grid", "block", "flex"} <= values["display"]
    assert {"red", "AccentColor"} <= values["color"]
    assert not {"azimuth", "pitch", "richness"} & values.keys()
    return values


def emit_table(out, name, data):
    out.write(f"#[rustfmt::skip]\npub(super) static {name}: &[(&str, &str)] = &[\n")
    for key, values in sorted(data.items()):
        out.write(f"    ({json.dumps(key)}, {json.dumps(' '.join(sorted(values)))}),\n")
    out.write("];\n\n")


def load(local, url, digest):
    if local:
        data = local.read_bytes()
    else:
        with urlopen(url, timeout=60) as response:
            data = response.read()
    if sha256(data).hexdigest() != digest:
        raise ValueError(f"Snapshot does not match pinned SHA-256: {url}")
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ("html", "webref"):
        parser.add_argument(f"--{flag}", type=Path, help="optional local copy of the pinned input")
    parser.add_argument("--check", action="store_true", help="verify rather than replace the table")
    args = parser.parse_args()
    elems, globals_, html_values = html_data(load(args.html, HTML, HTML_SHA256))
    css = css_data(load(args.webref, WEBREF, WEBREF_SHA256))
    output = io.StringIO()
    output.write(f"// Generated by scripts/generate_markup.py. Identifiers only, no specification prose.\n"
                 f"// WHATWG HTML (2026-11-17): element, attribute and event indexes, input type states, void elements.\n"
                 f"// {HTML}\n"
                 "// Copyright WHATWG; CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/).\n"
                 f"// W3C/CSSWG property definitions and value grammars via @webref/css {WEBREF_VERSION} (2026-11-13):\n"
                 f"// https://github.com/w3c/webref/tree/{WEBREF_REV}/packages/css\n"
                 "// WebRef package: MIT. Underlying spec data: original spec terms, e.g.\n"
                 "// W3C Permissive Document License (https://www.w3.org/copyright/software-license-2023/).\n\n")
    output.write(f'pub(super) const HTML_VOID: &str = "{" ".join(VOID)}";\n')
    output.write(f'pub(super) const HTML_GLOBAL: &str = "{" ".join(sorted(globals_))}";\n\n')
    emit_table(output, "HTML_ELEMENTS", elems)
    emit_table(output, "HTML_VALUES", {k: v for k, v in html_values.items() if v})
    emit_table(output, "CSS_PROPERTIES", css)
    generated = output.getvalue().rstrip() + "\n"
    if args.check:
        if DEST.read_text(encoding="utf-8") != generated:
            parser.error("generated lookup table differs; rerun without --check")
    else:
        DEST.write_text(generated, encoding="utf-8")


if __name__ == "__main__":
    main()

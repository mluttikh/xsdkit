#!/usr/bin/env python3
"""Regenerate the rule half of `tests/conformance/spec-rules.tsv` from the RECs.

Appendix B of both XSD 1.1 Recommendations is normative and is, in effect, an
index of every individually named rule in the language: each one is a
`div.constraintnote` carrying a stable anchor. That makes the specification a
finite checklist — 143 rows — rather than a document you can only read.

This fetches both RECs and prints `spec, anchor, kind, name`, one rule per
line. It does *not* touch the status columns, which are a human judgement about
this crate; `--check` compares the four rule columns against the committed file
and says nothing about the rest.

    scripts/extract-spec-rules.py            # print the inventory
    scripts/extract-spec-rules.py --check     # diff it against the committed file

The RECs are frozen, so this is not wired into CI: it is what you run when you
suspect an erratum, or when adding a spec version. `tests/spec_rules.rs` is the
gate, and it needs no network.
"""

import html
import re
import sys
import urllib.request
from pathlib import Path

RECS = [
    ("structures", "https://www.w3.org/TR/xmlschema11-1/"),
    ("datatypes", "https://www.w3.org/TR/xmlschema11-2/"),
]

COMMITTED = Path(__file__).resolve().parent.parent / "tests" / "conformance" / "spec-rules.tsv"

# The label and the name live inside one <b>, but four Structures rules wrap
# part of their own name in <code> or <b><i> — `xmlns` Not Allowed, `xsi:` Not
# Allowed, and the two Effective Total Range rules. A name group of `[^<]+`
# silently drops exactly those, which is how a first pass at this counted 139
# rules and looked right; a non-greedy run to `</b>` keeps them but truncates
# the two nested ones at their inner tag ("Effective Total Range (all"). The
# `<br>` that follows every note is the only terminator that survives both.
NOTE = re.compile(
    r'<div class="constraintnote">'
    r'<a id="(?P<anchor>[^"]+)"[^>]*></a>'
    r"<b>(?P<kind>[^<:]+):\s*(?P<name>.*?)</b>\s*<br",
    re.S,
)


def rules(page: str):
    for m in NOTE.finditer(page):
        name = re.sub(r"<[^>]+>", "", m.group("name"))
        name = html.unescape(re.sub(r"\s+", " ", name)).strip()
        yield m.group("anchor"), html.unescape(m.group("kind")).strip(), name


def inventory() -> list[tuple[str, str, str, str]]:
    out = []
    for spec, url in RECS:
        page = urllib.request.urlopen(url).read().decode("utf-8", "replace")
        found = list(rules(page))
        if not found:
            sys.exit(f"no constraintnote blocks in {url} — has the markup changed?")
        # A rule the spec states twice is one rule; the anchor is its identity.
        seen = set()
        for anchor, kind, name in found:
            if anchor in seen:
                continue
            seen.add(anchor)
            out.append((spec, anchor, kind, name))
        print(f"{spec}: {len(seen)} rules", file=sys.stderr)
    return out


def committed() -> list[tuple[str, str, str, str]]:
    rows = []
    for line in COMMITTED.read_text(encoding="utf-8").splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        f = line.split("\t")
        rows.append((f[0], f[1], f[2], f[3]))
    return rows


def main() -> int:
    live = inventory()
    if "--check" not in sys.argv:
        for row in live:
            print("\t".join(row))
        return 0

    have, want = set(committed()), set(live)
    for row in sorted(want - have):
        print("+ in the REC, missing from spec-rules.tsv: " + "\t".join(row))
    for row in sorted(have - want):
        print("- in spec-rules.tsv, not in the REC: " + "\t".join(row))
    if have != want:
        print(f"\n{len(have ^ want)} differences. Regenerate the rule columns and")
        print("re-judge the status of anything new.")
        return 1
    print(f"spec-rules.tsv matches both RECs: {len(live)} rules")
    return 0


if __name__ == "__main__":
    sys.exit(main())

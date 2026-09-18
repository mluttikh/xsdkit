#!/usr/bin/env python3
"""Writes `src/regex/blocks.rs`, the Unicode block table behind `\\p{IsX}`.

XSD names a Unicode block in a pattern as `\\p{IsX}`, where X is the block's
name from the Unicode database with white space and underbars stripped out and
hyphens and case kept: `Latin-1 Supplement` is `Latin-1Supplement`. That is
the rule in XSD 1.1 Part 2, G.4.2.3, and the names below follow it exactly.

The table is generated from `Blocks.txt` for the Unicode version the `regex`
crate implements, so that `\\p{IsX}` and a category escape such as `\\p{Lu}`
agree about which characters exist. Bump the two together:

    curl -sSfLO https://www.unicode.org/Public/16.0.0/ucd/Blocks.txt
    python3 scripts/generate-unicode-blocks.py Blocks.txt

The same section asks processors to keep recognising the Unicode 3.1 names
that XSD 1.0 schemas use and later versions renamed. Those are added by hand,
below, because no version of `Blocks.txt` still has them.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parents[1] / "src" / "regex" / "blocks.rs"

# XSD 1.1 Part 2, G.4.2.3: "block names from Unicode 3.1 known to have been
# superseded". Private Use was three blocks under one name.
SUPERSEDED = {
    "Greek": [(0x0370, 0x03FF)],
    "CombiningMarksforSymbols": [(0x20D0, 0x20FF)],
    "PrivateUse": [(0xE000, 0xF8FF), (0xF0000, 0xFFFFD), (0x100000, 0x10FFFD)],
}


def read_blocks(path: Path) -> tuple[str, dict[str, list[tuple[int, int]]]]:
    text = path.read_text(encoding="utf-8")
    version = re.search(r"^# Blocks-(\d+\.\d+\.\d+)\.txt", text, flags=re.MULTILINE)
    if version is None:
        sys.exit(f"{path}: no `# Blocks-X.Y.Z.txt` header, so no version to record")
    blocks: dict[str, list[tuple[int, int]]] = {}
    for line in text.splitlines():
        m = re.match(r"([0-9A-F]+)\.\.([0-9A-F]+); (.+)$", line.strip())
        if m is None:
            continue
        name = re.sub(r"[\s_]", "", m.group(3))
        if name in blocks:
            sys.exit(f"{path}: block name {name!r} appears twice")
        blocks[name] = [(int(m.group(1), 16), int(m.group(2), 16))]
    return version.group(1), blocks


def render(version: str, blocks: dict[str, list[tuple[int, int]]]) -> str:
    rows = []
    for name in sorted(blocks):
        ranges = ", ".join(f"(0x{lo:04X}, 0x{hi:04X})" for lo, hi in blocks[name])
        rows.append(f'    ("{name}", &[{ranges}]),')
    return "\n".join(
        [
            "//! Unicode block names, for XSD's `\\p{IsX}` escapes.",
            "//!",
            f"//! Generated from Unicode {version}'s `Blocks.txt` by",
            "//! `scripts/generate-unicode-blocks.py` — the Unicode version the `regex`",
            "//! crate implements, so a block escape and a category escape agree about",
            "//! which characters exist. Regenerate rather than edit.",
            "//!",
            "//! Names are the block's name with white space and underbars removed and",
            "//! hyphens and case kept, which is how XSD spells them. The three Unicode",
            "//! 3.1 names that XSD 1.0 schemas use and later versions renamed are",
            "//! included, as XSD 1.1 asks. Sorted by name, for a binary search.",
            "",
            "/// The Unicode version this table was generated from.",
            f'pub(crate) const UNICODE_VERSION: &str = "{version}";',
            "",
            "#[rustfmt::skip]",
            "pub(crate) static BLOCKS: &[(&str, &[(u32, u32)])] = &[",
            *rows,
            "];",
            "",
        ]
    )


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit("usage: scripts/generate-unicode-blocks.py path/to/Blocks.txt")
    version, blocks = read_blocks(Path(sys.argv[1]))
    for name, ranges in SUPERSEDED.items():
        if name in blocks:
            sys.exit(f"{name!r} is a current block name again; drop it from SUPERSEDED")
        blocks[name] = ranges
    OUT.parent.mkdir(exist_ok=True)
    OUT.write_text(render(version, blocks), encoding="utf-8")
    print(f"wrote {OUT.relative_to(OUT.parents[2])}: {len(blocks)} block names, Unicode {version}")


if __name__ == "__main__":
    main()

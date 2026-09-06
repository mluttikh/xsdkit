#!/usr/bin/env python3
"""Run every Python snippet in the Markdown documentation.

The Rust half of the docs is compiled by `check-doc-snippets.py`; this is the
other half, and it can do better than compile — the examples all run against
`docs/examples/report.xsd`, so they can simply be executed.

A page is run as one program: blocks share a namespace in the order they
appear, because that is how a reader reads them. A block that assumes a name
no earlier block defined is a block nobody can paste, which is exactly the
failure worth catching.

Conventions:

- ```python is run. ```text is the *output* of the block above it and is
  never run.
- ```python,ignore is skipped, for a block that is deliberately a sketch
  rather than code — `head.substitutes` on a `head` no page defines. Use it
  sparingly; it switches the guard off.
- A block whose trailing comment names an exception — `# KeyError` — is
  expected to raise it, and failing to raise is then the error. That is the
  notation the docs already used.

Run it directly, or let CI do it:

    python3 scripts/check-doc-snippets-python.py
"""

from __future__ import annotations

import io
import re
import sys
import traceback
from contextlib import redirect_stdout
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Every example is written as though it were sitting next to the schema.
CWD = ROOT / "docs" / "examples"

FENCE = re.compile(r"^(?P<indent>[ \t]*)```python(?P<info>[^\n]*)$")
# The docs already say what a line raises, in a trailing comment:
#     schemas["{urn:example}nope"]    # KeyError
# so that is the notation, rather than one invented for this script.
RAISES = re.compile(r"#\s*(?:raises:\s*)?([A-Z]\w*(?:Error|Exception))\b")


def sources() -> list[Path]:
    files = [p for p in sorted(ROOT.joinpath("docs").rglob("*.md")) if "rust" not in p.parts]
    return [ROOT / "README.md", *files]


def blocks(path: Path) -> list[tuple[int, str, str]]:
    """Every ```python block: line number, info string, source."""
    lines = path.read_text().split("\n")
    found = []
    i = 0
    while i < len(lines):
        m = FENCE.match(lines[i])
        if not m:
            i += 1
            continue
        indent, info = m.group("indent"), m.group("info")
        start = i + 1
        j = start
        while j < len(lines) and lines[j].strip() != "```":
            j += 1
        body = [ln[len(indent) :] if ln.startswith(indent) else ln for ln in lines[start:j]]
        found.append((start + 1, info, "\n".join(body)))
        i = j + 1
    return found


def main() -> int:
    sys.path.insert(0, str(ROOT / "python"))
    failures: list[str] = []
    ran = skipped = 0

    for path in sources():
        found = blocks(path)
        if not found:
            continue
        rel = path.relative_to(ROOT)
        # One namespace per page: later blocks build on earlier ones, the way
        # a reader works through them.
        env: dict[str, object] = {"__name__": "__doc_snippet__"}
        for line, info, code in found:
            if "ignore" in info:
                skipped += 1
                continue
            expected = RAISES.search(code)
            ran += 1
            try:
                with redirect_stdout(io.StringIO()):
                    exec(compile(code, f"{rel}:{line}", "exec"), env)
            except Exception as exc:  # noqa: BLE001 — reporting, not handling
                if expected and type(exc).__name__ == expected.group(1):
                    continue
                failures.append(
                    f"{rel}:{line}\n"
                    + traceback.format_exc(limit=1).rstrip()
                    + f"\n    in:\n{code.rstrip()}\n"
                )
            else:
                if expected:
                    failures.append(
                        f"{rel}:{line}\n    expected {expected.group(1)}, nothing raised\n"
                    )

    print(f"ran {ran} Python snippets ({skipped} skipped)")
    for f in failures:
        print("\n" + f)
    return 1 if failures else 0


if __name__ == "__main__":
    import os

    os.chdir(CWD)
    sys.exit(main())

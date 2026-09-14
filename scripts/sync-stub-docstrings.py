#!/usr/bin/env python3
"""Writes the compiled module's docstrings into the type stub.

Editors show the stub, not the module: a hover over `SchemaSet.decode` in VS
Code or PyCharm reads `python/xsdkit/_xsdkit.pyi`, and most of its members had
no docstring there at all. The prose already exists — `#[pyo3]` turns each
`///` comment in `src/python.rs` into `__doc__` — so the Rust comment stays the
one place it is written, and this copies it into the stub, where it is
committed and reviewed with everything else.

    maturin develop          # or install a wheel built from this checkout
    python3 scripts/sync-stub-docstrings.py           # rewrite the stub
    python3 scripts/sync-stub-docstrings.py --check   # fail if it would change

`python/tests/test_stubs.py` runs the check, so a `///` edited without running
this fails CI rather than shipping a stale hover.
"""

from __future__ import annotations

import ast
import inspect
import sys
from pathlib import Path

STUB = Path(__file__).resolve().parents[1] / "python" / "xsdkit" / "_xsdkit.pyi"


def documented(name: str) -> bool:
    """Dunders carry CPython's slot docstrings, which say nothing; `_repr_html_`
    is what a notebook calls, and is worth a hover."""
    return not name.startswith("_") or name == "_repr_html_"


def runtime_doc(obj: object) -> str | None:
    text = getattr(obj, "__doc__", None)
    return inspect.cleandoc(text) if text and text.strip() else None


def targets(tree: ast.Module, native: object):
    """Every stub definition the compiled module has, with its qualified name
    and the module's docstring for it."""
    for node in tree.body:
        if isinstance(node, ast.ClassDef):
            cls = getattr(native, node.name, None)
            if cls is None:
                continue
            yield node.name, node, runtime_doc(cls)
            for member in node.body:
                if isinstance(member, ast.FunctionDef) and documented(member.name):
                    attr = inspect.getattr_static(cls, member.name, None)
                    yield f"{node.name}.{member.name}", member, runtime_doc(attr)
        elif isinstance(node, ast.FunctionDef) and documented(node.name):
            fn = getattr(native, node.name, None)
            if fn is not None:
                yield node.name, node, runtime_doc(fn)


def literal(text: str, indent: str) -> list[str]:
    """`text` as a docstring whose value is exactly `text` again."""
    prefix, body = "", text
    if '"""' in text or text.endswith(('"', "\\")):
        body = text.replace("\\", "\\\\").replace('"', '\\"')
    elif "\\" in text:
        prefix = "r"
    lines = body.split("\n")
    if len(lines) == 1:
        return [f'{indent}{prefix}"""{lines[0]}"""']
    return (
        [f'{indent}{prefix}"""{lines[0]}']
        + [f"{indent}{line}" if line else "" for line in lines[1:]]
        + [f'{indent}"""']
    )


def rewrite(source: str, native: object) -> tuple[str, list[str]]:
    """The stub with every docstring the module's, and what had to change."""
    lines = source.split("\n")
    edits: list[tuple[int, int, list[str]]] = []
    changed: list[str] = []
    for name, node, doc in targets(ast.parse(source), native):
        current = ast.get_docstring(node)
        if current == doc:
            continue
        changed.append(name)
        first = node.body[0]
        indent = " " * (node.col_offset + 4)
        is_docstring = (
            isinstance(first, ast.Expr)
            and isinstance(first.value, ast.Constant)
            and isinstance(first.value.value, str)
        )
        if doc is None:
            # Documented in the stub and not in the module: the module is
            # where it belongs, so the stub loses it rather than keeping a
            # text nothing else checks.
            if is_docstring:
                replacement = [f"{indent}..."] if len(node.body) == 1 else []
                edits.append((first.lineno - 1, first.end_lineno, replacement))
            continue
        new = literal(doc, indent)
        if is_docstring:
            edits.append((first.lineno - 1, first.end_lineno, new))
        elif isinstance(first, ast.Expr) and first.value.value is Ellipsis and len(node.body) == 1:
            line = lines[first.lineno - 1]
            if line[: first.col_offset].strip():
                # `def f(self) -> T: ...` on one line.
                edits.append((first.lineno - 1, first.lineno, [line[: first.col_offset].rstrip()] + new))
            else:
                edits.append((first.lineno - 1, first.lineno, new))
        else:
            start = min([first.lineno] + [d.lineno for d in getattr(first, "decorator_list", [])])
            blank = [""] if isinstance(node, ast.ClassDef) else []
            edits.append((start - 1, start - 1, new + blank))
    for start, end, replacement in sorted(edits, key=lambda e: e[0], reverse=True):
        lines[start:end] = replacement
    return "\n".join(lines), changed


def main() -> int:
    import xsdkit._xsdkit as native

    check = "--check" in sys.argv[1:]
    source = STUB.read_text(encoding="utf-8")
    updated, changed = rewrite(source, native)
    if not changed:
        print("the stub carries every docstring the module has")
        return 0
    if check:
        print(f"{len(changed)} docstring(s) in the stub differ from the module:")
        for name in changed:
            print(f"  {name}")
        print("run scripts/sync-stub-docstrings.py against a build of this checkout")
        return 1
    ast.parse(updated)
    STUB.write_text(updated, encoding="utf-8")
    again = rewrite(updated, native)[1]
    assert not again, f"not idempotent: {again}"
    print(f"wrote {len(changed)} docstring(s) into {STUB.name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

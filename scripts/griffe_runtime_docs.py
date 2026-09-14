"""Fill in documentation the type stub does not carry.

The Python API is a compiled extension module. `#[pyo3]` turns the Rust doc
comments into `__doc__`, and `scripts/sync-stub-docstrings.py` copies those
into `python/xsdkit/_xsdkit.pyi`, where editors read them — `test_stubs.py`
fails when the two differ. So the stub carries the types and the prose for
every public member, and griffe analysing it gets both.

What the stub still leaves out is the dunders the reference shows —
`__len__`, `__iter__` and the rest — whose docstrings the sync skips because
CPython's slot defaults say nothing. This extension reads the stub and
borrows `__doc__` from the imported module wherever the stub has none.
"""

from __future__ import annotations

import importlib
import inspect

import griffe


class RuntimeDocstrings(griffe.Extension):
    """Copies `__doc__` from the imported module onto stub objects that lack one."""

    def on_package(self, *, pkg: griffe.Module, **_: object) -> None:
        try:
            module = importlib.import_module(pkg.name)
        except ImportError as e:  # pragma: no cover - the docs build needs the wheel
            raise RuntimeError(
                f"cannot import `{pkg.name}` to read its docstrings; "
                "build the extension module first (`maturin develop`)"
            ) from e
        self._merge(pkg, module)

    def _merge(self, obj: griffe.Object | griffe.Alias, runtime: object) -> None:
        for name, member in obj.members.items():
            # Private names are not part of the reference. One may also be an
            # alias into another package — `_abc` for `collections.abc` — which
            # griffe has not loaded and cannot resolve.
            if name.startswith("_") and not (name.startswith("__") and name.endswith("__")):
                continue
            attr = getattr(runtime, name, None)
            if attr is None:
                continue
            try:
                self._adopt(member, attr)
            except (AttributeError, griffe.AliasResolutionError):
                # An alias whose target is not loaded has no docstring to set.
                continue
            if member.is_class:
                self._merge(member, attr)

    @staticmethod
    def _adopt(member: griffe.Object | griffe.Alias, attr: object) -> None:
        if member.docstring is not None and member.docstring.value.strip():
            return  # the stub said something; it wins
        text = getattr(attr, "__doc__", None)
        if not text or not text.strip():
            return
        # pyo3 puts the signature in `text_signature`, never in `__doc__`, so
        # unlike a C extension there is no signature line to strip here.
        member.docstring = griffe.Docstring(inspect.cleandoc(text), parent=member)

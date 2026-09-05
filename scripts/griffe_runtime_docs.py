"""Fill in documentation the type stub does not carry.

The Python API is a compiled extension module, so its documentation exists
twice over and neither copy is complete. `python/xsdkit/_xsdkit.pyi` has the
type annotations — the thing a reference is mostly *for* — and almost no
prose. The built module has the full prose, because `#[pyo3]` turns the Rust
doc comments into `__doc__`, and no annotations at all: a property compiled
from Rust has no return type to inspect.

Griffe reads one or the other. Asked to analyse the stub it gets the types;
asked to inspect the module it gets the text. This extension reads the stub
and borrows the text, so the reference has both without the doc comments
being written out a second time in the stub, where they would drift.
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
            attr = getattr(runtime, name, None)
            if attr is None:
                continue
            try:
                self._adopt(member, attr)
            except AttributeError:
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

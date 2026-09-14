"""Type aliases for code that uses xsdkit.

The names the type stub uses in its signatures, importable at runtime so that
annotated code can name them too::

    from xsdkit.typing import XsdValue

    def total(values: list[XsdValue]) -> int: ...

They lived only in the stub, where a type checker could read them and a
program could not import them.
"""

import datetime
import decimal
import os
from collections.abc import Callable
from typing import Any, Literal

__all__ = [
    "Conformance",
    "ContentKind",
    "EventKind",
    "Instance",
    "ModelKind",
    "Name",
    "Resolver",
    "Severity",
    "Use",
    "Variety",
    "XsdValue",
    "XsdVersion",
]

Conformance = Literal["strict", "lax"]
"""How strictly a schema is read: ``"strict"`` reports errors, ``"lax"``
downgrades what it can to warnings."""

XsdVersion = Literal["1.0", "1.1"]
"""Which XSD to read the documents as."""

Severity = Literal["error", "warning", "note"]
"""How serious a diagnostic is."""

Variety = Literal["atomic", "list", "union"]
"""What kind of simple type a type is."""

ContentKind = Literal["empty", "simple", "element-only", "mixed"]
"""What a complex type may contain."""

ModelKind = Literal["empty", "automaton", "all"]
"""How a complex type's content model is checked."""

Use = Literal["required", "optional", "prohibited"]
"""Whether an attribute must, may or must not appear."""

EventKind = Literal["start", "text", "end"]
"""What a PSVI event is."""

Name = str | tuple[str | None, str]
"""A name as Clark notation (``{ns}local``), a bare local name, or a
``(namespace, local)`` pair."""

Resolver = Callable[[str, str | None], bytes | str | tuple[str, bytes | str]]
"""Resolves a schema location to a document.

Called with ``(location, base)``, where ``base`` is the URI of the document
containing the reference, or ``None``. Return the document as ``bytes`` —
leaving the encoding to xsdkit, which reads the byte-order mark and the XML
declaration — or as ``str``, or as ``(uri, document)`` to say where it was
actually found. Raise to report that it could not be resolved; the exception
becomes the diagnostic, and the first one raised is the ``SchemaError``'s
``__cause__``. ``KeyboardInterrupt`` and ``SystemExit`` end the build and
propagate as themselves.

Replaces the filesystem rather than adding to it, so it is an alternative to
``search_paths``, not a layer on top."""

Instance = str | bytes | bytearray | os.PathLike[str]
"""A document: XML as text, as bytes whose encoding is detected, or a path to
read it from. A ``str`` is always content — a path and a document cannot be
told apart once both are strings — so pass ``pathlib.Path`` for a file."""

XsdValue = (
    str
    | bool
    | int
    | float
    | decimal.Decimal
    | bytes
    | datetime.datetime
    | datetime.date
    | datetime.time
    | datetime.timedelta
    | list[Any]
)
"""An XSD value as its closest native Python type.

Durations and gregorian fragments stay as their canonical lexical strings —
``xs:duration`` has no lossless Python counterpart, since months and seconds
are not commensurable. ``xs:dayTimeDuration`` alone becomes a ``timedelta``.
A value ``datetime`` cannot hold exactly stays lexical too: a year outside 1 to
9999, an ``xs:date`` with a timezone, or digits below the microsecond.

An ``xs:QName`` arrives as Clark notation (``{namespace}local``) with its
prefix already resolved, since the prefix is a spelling rather than part of
the value."""

"""A generic XSD reader: parse W3C XML Schema into a queryable component model.

    >>> import xsdkit
    >>> schemas = xsdkit.SchemaSet.from_file("report.xsd")
    >>> report = schemas.element("urn:example", "report")
    >>> for child in report.children:
    ...     print(child.local_name, child.repeats)
    title False
    issued False
    item True

Reading a document as it is validated, with values arriving as native Python
types::

    >>> events = schemas.iter_typed(open("report.xml").read())
    >>> [ev.value for ev in events if ev.kind == "text"][:2]
    ['November orders', datetime.date(2024, 12, 1)]
    >>> events.report.is_valid
    True
"""

import collections.abc as _abc

from ._xsdkit import (
    AppInfo,
    Attribute,
    AttributeUse,
    AttributeValue,
    Child,
    ChildIterator,
    Diagnostic,
    Document,
    DocumentError,
    Element,
    Facets,
    InvalidValueError,
    NamedComponents,
    NameIterator,
    PsviEvent,
    PsviEvents,
    SchemaError,
    SchemaSet,
    Span,
    Tree,
    Type,
    ValidationReport,
    XsdError,
    __version__,
    load,
    load_bytes,
    load_files,
    load_string,
)

__all__ = [
    "AppInfo",
    "Attribute",
    "AttributeUse",
    "AttributeValue",
    "Child",
    "ChildIterator",
    "Diagnostic",
    "Document",
    "DocumentError",
    "Element",
    "Facets",
    "InvalidValueError",
    "NameIterator",
    "NamedComponents",
    "PsviEvent",
    "PsviEvents",
    "SchemaError",
    "SchemaSet",
    "Span",
    "Tree",
    "Type",
    "ValidationReport",
    "XsdError",
    "__version__",
    "load",
    "load_bytes",
    "load_files",
    "load_string",
]

# A mapping in full, so `isinstance(schemas, Mapping)` holds as `dict(schemas)` does.
_abc.Mapping.register(SchemaSet)

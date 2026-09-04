"""A generic XSD reader: parse W3C XML Schema into a queryable component model.

    >>> import xsdkit
    >>> schemas = xsdkit.SchemaSet.from_file("report.xsd")
    >>> report = schemas.element("urn:example", "report")
    >>> for child in report.type.children:
    ...     print(child.local_name, report.type.repeats(child))

Validating a document, with values arriving as native Python types::

    >>> events, outcome = schemas.read_typed(open("report.xml").read())
    >>> outcome.is_valid
    True
"""

from ._xsdkit import (
    AppInfo,
    Attribute,
    AttributeUse,
    AttributeValue,
    Diagnostic,
    Document,
    Element,
    Facets,
    PsviEvent,
    SchemaError,
    SchemaSet,
    Span,
    Type,
    ValidationReport,
    XsdError,
    __version__,
    load,
    load_string,
)

__all__ = [
    "AppInfo",
    "Attribute",
    "AttributeUse",
    "AttributeValue",
    "Diagnostic",
    "Document",
    "Element",
    "Facets",
    "PsviEvent",
    "SchemaError",
    "SchemaSet",
    "Span",
    "Type",
    "ValidationReport",
    "XsdError",
    "__version__",
    "load",
    "load_string",
]

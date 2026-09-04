"""A generic XSD reader: parse W3C XML Schema into a queryable component model.

    >>> import xsdkit
    >>> schemas = xsdkit.SchemaSet.from_file("report.xsd")
    >>> report = schemas.element("urn:example", "report")
    >>> for child in report.type.children:
    ...     print(child.local_name, report.type.repeats(child))
"""

from ._xsdkit import (
    AppInfo,
    Attribute,
    AttributeUse,
    Diagnostic,
    Document,
    Element,
    Facets,
    SchemaError,
    SchemaSet,
    Span,
    Type,
    XsdError,
    __version__,
    load,
    load_string,
)

__all__ = [
    "AppInfo",
    "Attribute",
    "AttributeUse",
    "Diagnostic",
    "Document",
    "Element",
    "Facets",
    "SchemaError",
    "SchemaSet",
    "Span",
    "Type",
    "XsdError",
    "__version__",
    "load",
    "load_string",
]

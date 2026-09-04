"""Type stubs for the native module.

Names here mirror `src/python.rs`. Keep them in step: `test_stubs.py` checks
that every exported name exists at runtime.
"""

import datetime
import decimal
from typing import Any, Callable, Iterable, Literal, Sequence

__version__: str

Conformance = Literal["strict", "lax"]
Severity = Literal["error", "warning", "note"]
Variety = Literal["atomic", "list", "union"]
ContentKind = Literal["empty", "simple", "element-only", "mixed"]
ModelKind = Literal["empty", "automaton", "all"]
Use = Literal["required", "optional", "prohibited"]

#: A name as Clark notation (``{ns}local``), a bare local name, or a pair.
Name = str | tuple[str | None, str]

#: An XSD value as its closest native Python type.
#:
#: Durations and gregorian fragments stay as their canonical lexical strings —
#: ``xs:duration`` has no lossless Python counterpart, since months and seconds
#: are not commensurable. ``xs:dayTimeDuration`` alone becomes a ``timedelta``.
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
EventKind = Literal["start", "text", "end"]

class XsdError(Exception): ...

class SchemaError(XsdError):
    #: Every diagnostic from the failed build, not just the first.
    diagnostics: list[Diagnostic]

class Span:
    uri: str
    line: int
    label: str | None

class Diagnostic:
    @property
    def code(self) -> str: ...
    @property
    def severity(self) -> Severity: ...
    @property
    def message(self) -> str: ...
    @property
    def spans(self) -> list[Span]: ...
    @property
    def help(self) -> str | None: ...
    @property
    def is_error(self) -> bool: ...

class Document:
    uri: str
    target_namespace: str | None
    #: True when the document had no ``targetNamespace`` of its own and was
    #: absorbed into its includer's.
    chameleon: bool

class AppInfo:
    source: str | None
    #: The ``appinfo`` children, re-serialized with names in Clark notation.
    xml: str

class Facets:
    @property
    def length(self) -> int | None: ...
    @property
    def min_length(self) -> int | None: ...
    @property
    def max_length(self) -> int | None: ...
    @property
    def patterns(self) -> list[list[str]]:
        """Outer list: one entry per restriction step, ANDed.

        Inner list: alternatives declared at that step, ORed.
        """
    @property
    def enumeration(self) -> list[str] | None: ...
    @property
    def white_space(self) -> str | None: ...
    @property
    def max_inclusive(self) -> str | None: ...
    @property
    def max_exclusive(self) -> str | None: ...
    @property
    def min_inclusive(self) -> str | None: ...
    @property
    def min_exclusive(self) -> str | None: ...
    @property
    def total_digits(self) -> int | None: ...
    @property
    def fraction_digits(self) -> int | None: ...

class Attribute:
    @property
    def name(self) -> tuple[str | None, str]: ...
    @property
    def qname(self) -> str: ...
    @property
    def local_name(self) -> str: ...
    @property
    def type(self) -> Type: ...
    @property
    def default(self) -> str | None: ...
    @property
    def fixed(self) -> str | None: ...
    @property
    def doc(self) -> str | None: ...
    @property
    def appinfo(self) -> list[AppInfo]: ...

class AttributeUse:
    @property
    def attribute(self) -> Attribute: ...
    @property
    def name(self) -> tuple[str | None, str]: ...
    @property
    def local_name(self) -> str: ...
    @property
    def type(self) -> Type: ...
    @property
    def required(self) -> bool: ...
    @property
    def use(self) -> Use: ...
    @property
    def fixed(self) -> str | None: ...
    @property
    def default(self) -> str | None: ...

class Element:
    @property
    def name(self) -> tuple[str | None, str]: ...
    @property
    def qname(self) -> str: ...
    @property
    def local_name(self) -> str: ...
    @property
    def namespace(self) -> str | None: ...
    @property
    def type(self) -> Type: ...
    @property
    def nillable(self) -> bool: ...
    @property
    def abstract(self) -> bool: ...
    @property
    def is_global(self) -> bool: ...
    @property
    def substitutes(self) -> list[Element]:
        """Every element that may appear here, transitively.

        Includes this one unless it is abstract.
        """
    @property
    def default(self) -> str | None: ...
    @property
    def fixed(self) -> str | None: ...
    @property
    def doc(self) -> str | None: ...
    @property
    def appinfo(self) -> list[AppInfo]: ...

class Type:
    @property
    def name(self) -> tuple[str | None, str] | None: ...
    @property
    def qname(self) -> str | None: ...
    @property
    def is_complex(self) -> bool: ...
    @property
    def is_simple(self) -> bool: ...
    @property
    def abstract(self) -> bool: ...
    @property
    def base(self) -> Type | None: ...
    @property
    def derivation(self) -> Literal["extension", "restriction"] | None: ...
    def derives_from(self, other: Type, /) -> bool: ...
    @property
    def base_chain(self) -> list[Type]: ...
    @property
    def attributes(self) -> list[AttributeUse]: ...
    @property
    def children(self) -> list[Element]:
        """Elements that may appear directly inside, substitution groups
        expanded and inherited content included."""
    def repeats(self, child: Element, /) -> bool: ...
    def optional(self, child: Element, /) -> bool: ...
    @property
    def content(self) -> ContentKind | None: ...
    @property
    def content_model(self) -> ModelKind | None: ...
    def accepts(self, names: Iterable[Name], /) -> bool: ...
    def validate(self, lexical: str, /) -> XsdValue:
        """Raises ``ValueError`` with the reason when not valid."""
    def is_valid(self, lexical: str, /) -> bool: ...
    @property
    def variety(self) -> Variety | None: ...
    @property
    def primitive(self) -> str | None: ...
    @property
    def builtin(self) -> str | None: ...
    @property
    def item_type(self) -> Type | None: ...
    @property
    def member_types(self) -> list[Type]: ...
    @property
    def facets(self) -> Facets | None: ...
    @property
    def doc(self) -> str | None: ...
    @property
    def appinfo(self) -> list[AppInfo]: ...

class AttributeValue:
    @property
    def name(self) -> tuple[str | None, str]: ...
    @property
    def local_name(self) -> str: ...
    @property
    def declaration(self) -> Attribute | None: ...
    @property
    def value(self) -> XsdValue | None:
        """The typed value, or ``None`` when it did not validate."""
    @property
    def lexical(self) -> str: ...
    @property
    def from_schema(self) -> bool:
        """True when the document did not spell this attribute out and the
        schema supplied it from a ``default`` or ``fixed`` value."""

class PsviEvent:
    @property
    def kind(self) -> EventKind: ...
    @property
    def name(self) -> tuple[str | None, str]: ...
    @property
    def local_name(self) -> str: ...
    @property
    def declaration(self) -> Element | None: ...
    @property
    def type(self) -> Type | None:
        """The type in force, after any ``xsi:type`` override."""
    @property
    def type_from_instance(self) -> bool: ...
    @property
    def nil(self) -> bool: ...
    @property
    def attributes(self) -> list[AttributeValue]: ...
    @property
    def value(self) -> XsdValue | None:
        """The typed value, on a ``"text"`` event."""
    @property
    def lexical(self) -> str | None: ...
    @property
    def line(self) -> int: ...

class ValidationReport:
    @property
    def is_valid(self) -> bool: ...
    @property
    def diagnostics(self) -> list[Diagnostic]: ...
    @property
    def errors(self) -> list[Diagnostic]: ...
    def __bool__(self) -> bool: ...

class SchemaSet:
    @classmethod
    def from_file(
        cls,
        path: str,
        *,
        search_paths: Sequence[str] | None = ...,
        conformance: Conformance = ...,
        nodes_limit: int | None = ...,
    ) -> SchemaSet:
        """Raises `SchemaError` on any error diagnostic."""
    @classmethod
    def from_string(
        cls,
        xsd: str,
        *,
        uri: str = ...,
        search_paths: Sequence[str] | None = ...,
        conformance: Conformance = ...,
        nodes_limit: int | None = ...,
    ) -> SchemaSet: ...
    @classmethod
    def from_bytes(
        cls,
        data: bytes,
        *,
        uri: str = ...,
        search_paths: Sequence[str] | None = ...,
        conformance: Conformance = ...,
        nodes_limit: int | None = ...,
    ) -> SchemaSet:
        """Detects the encoding: byte-order mark, then the XML declaration,
        then UTF-8."""
    @property
    def documents(self) -> list[Document]: ...
    @property
    def elements(self) -> list[tuple[str, Element]]: ...
    @property
    def types(self) -> list[tuple[str, Type]]: ...
    @property
    def counts(self) -> list[tuple[str, int]]: ...
    def element(self, namespace: Name | None, local: str | None = ..., /) -> Element | None: ...
    def type(self, namespace: Name | None, local: str | None = ..., /) -> Type | None: ...
    def attribute(self, namespace: Name | None, local: str | None = ..., /) -> Attribute | None: ...
    def validate(self, xml: str, *, uri: str = ...) -> ValidationReport:
        """Validates a document. Never raises for an invalid one — that is an
        answer, not an error."""
    def read_typed(
        self,
        xml: str,
        *,
        on_event: Callable[[PsviEvent], None] | None = ...,
        uri: str = ...,
    ) -> tuple[list[PsviEvent] | None, ValidationReport]:
        """Reads a document into typed PSVI events.

        Returns the events as a list, or feeds them to ``on_event`` and
        returns ``None`` in their place.
        """

def load(
    path: str,
    *,
    search_paths: Sequence[str] | None = ...,
    conformance: Conformance = ...,
    nodes_limit: int | None = ...,
) -> tuple[SchemaSet, list[Diagnostic]]:
    """Loads a schema and returns it *with* its diagnostics, rather than
    raising. For schemas expected to be imperfect."""

def load_string(
    xsd: str,
    *,
    uri: str = ...,
    search_paths: Sequence[str] | None = ...,
    conformance: Conformance = ...,
    nodes_limit: int | None = ...,
) -> tuple[SchemaSet, list[Diagnostic]]: ...

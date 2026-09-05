"""Type stubs for the native module.

Names here mirror `src/python.rs`. Keep them in step: `test_stubs.py` checks
that every exported name exists at runtime.
"""

import datetime
import decimal
import os
from typing import Any, Callable, Iterable, Iterator, Literal, Sequence

__version__: str

Conformance = Literal["strict", "lax"]
#: Which XSD to read the documents as.
XsdVersion = Literal["1.0", "1.1"]
Severity = Literal["error", "warning", "note"]
Variety = Literal["atomic", "list", "union"]
ContentKind = Literal["empty", "simple", "element-only", "mixed"]
ModelKind = Literal["empty", "automaton", "all"]
Use = Literal["required", "optional", "prohibited"]

#: A name as Clark notation (``{ns}local``), a bare local name, or a pair.
Name = str | tuple[str | None, str]

#: Resolves a schema location to a document.
#:
#: Called with ``(location, base)``, where ``base`` is the URI of the document
#: containing the reference, or ``None``. Return the document as ``bytes`` —
#: leaving the encoding to xsdkit, which reads the byte-order mark and the XML
#: declaration — or as ``str``, or as ``(uri, document)`` to say where it was
#: actually found. Raise to report that it could not be resolved; the exception
#: becomes the diagnostic.
#:
#: Replaces the filesystem rather than adding to it, so it is an alternative to
#: ``search_paths``, not a layer on top.
Resolver = Callable[[str, str | None], bytes | str | tuple[str, bytes | str]]

#: A document to validate: text, or bytes whose encoding xsdkit detects.
Instance = str | bytes

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
    #: The ``xs:schema`` ``version`` attribute, verbatim. The specification
    #: declares it as a bare token with no processing role, so it is reported
    #: rather than interpreted.
    version: str | None

class AppInfo:
    source: str | None
    #: The ``appinfo`` children, re-serialized with names in Clark notation.
    xml: str

class Facets:
    """A set of facets on a simple type.

    Bounds and enumerations are the lexical forms the schema wrote, not typed
    values: a facet constrains the lexical space as much as the value space.
    Pass one through ``Type.validate`` for the value.
    """

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
    """An element declaration: a name, a type, and how it may appear.

    An element behaves as its children — iterable, sized, and subscriptable by
    name — so a schema is walked without a ``.type`` hop at every level::

        report["item"]["price"].qname
        [child.local_name for child in report]
    """

    def __len__(self) -> int:
        """How many children this element may have, by name."""
    def __iter__(self) -> Iterator[Element]:
        """The children, so ``for child in element`` reads."""
    def __getitem__(self, name: Name, /) -> Element:
        """The child of that name, raising ``KeyError`` when there is none.

        A local name is enough, since a child is almost always in its parent's
        namespace.
        """
    @property
    def children(self) -> list[Element]:
        """Elements that may appear directly inside this one.

        The same as ``element.type.children``, without the hop.
        """
    @property
    def attributes(self) -> list[AttributeUse]:
        """The attributes this element may carry, with how it may carry them."""
    def repeats(self, child: Element, /) -> bool:
        """Whether ``child`` may appear here more than once."""
    def optional(self, child: Element, /) -> bool:
        """Whether ``child`` may be left out."""
    def _repr_html_(self) -> str:
        """Shows the tree in a notebook, shallower than ``tree()``."""
    def tree(self, depth: int = ...) -> Tree:
        """A readable tree of what may appear inside.

        ``?`` optional, ``+`` one or more, ``*`` any number, nothing for
        exactly once; ``@name`` for attributes. Recursion stops where the shape
        repeats::

            report
              title: xs:string
              item+
                @sku
                price: xs:decimal
                note?: xs:string
        """
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
    """A type definition, simple or complex."""

    def tree(self, depth: int = ...) -> Tree:
        """A readable tree of what may appear inside this type."""
    def _repr_html_(self) -> str: ...

    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[Element]: ...
    def __getitem__(self, name: Name, /) -> Element:
        """The child element of that name, raising ``KeyError`` when absent."""
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
    def facets(self) -> Facets | None:
        """The facets in force, composed down the whole restriction chain.

        A restriction inherits everything its base constrained, so a type that
        declares only ``maxLength`` still has its base's ``minLength``. This is
        what ``validate`` applies.
        """
    @property
    def declared_facets(self) -> Facets | None:
        """The facets *this type* declares, without its base's — what the
        restriction step wrote."""
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

class PsviEvents:
    """An iterator over one document's typed events."""

    def __iter__(self) -> Iterator[PsviEvent]: ...
    def __next__(self) -> PsviEvent: ...
    def __len__(self) -> int:
        """How many events are left."""
    @property
    def report(self) -> ValidationReport:
        """The outcome, available before the events are consumed as well as
        after — a document can be read for its values and still be invalid."""

class Tree:
    """Rendered text that knows how to show itself.

    A plain ``str`` is the wrong type for something meant to be *looked at*: a
    notebook displays ``repr()`` of the last expression, and ``repr`` of a
    string escapes every newline. This renders as itself in a REPL, in a
    notebook and through ``print``, while still behaving as the text it is.
    """

    def __str__(self) -> str: ...
    def __repr__(self) -> str: ...
    def __len__(self) -> int: ...
    def __contains__(self, needle: str, /) -> bool: ...
    def __eq__(self, other: object, /) -> bool: ...
    def _repr_html_(self) -> str: ...
    def splitlines(self) -> list[str]: ...
    def count(self, needle: str, /) -> int: ...

class ElementIterator:
    def __iter__(self) -> Iterator[Element]: ...
    def __next__(self) -> Element: ...
    def __len__(self) -> int: ...

class NameIterator:
    def __iter__(self) -> Iterator[str]: ...
    def __next__(self) -> str: ...
    def __len__(self) -> int: ...

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
        path: str | os.PathLike[str],
        *,
        search_paths: Sequence[str] | None = ...,
        conformance: Conformance = ...,
        version: XsdVersion = ...,
        nodes_limit: int | None = ...,
        resolver: Resolver | None = ...,
    ) -> SchemaSet:
        """Raises `SchemaError` on any error diagnostic.

        ``version="1.1"`` turns on XSD 1.1: open content, conditional
        inclusion, assertions on wildcards, ``xs:precisionDecimal`` and the
        relaxed Unique Particle Attribution rule.
        """
    @classmethod
    def from_string(
        cls,
        xsd: str,
        *,
        uri: str = ...,
        search_paths: Sequence[str] | None = ...,
        conformance: Conformance = ...,
        version: XsdVersion = ...,
        nodes_limit: int | None = ...,
        resolver: Resolver | None = ...,
    ) -> SchemaSet: ...
    @classmethod
    def from_bytes(
        cls,
        data: bytes,
        *,
        uri: str = ...,
        search_paths: Sequence[str] | None = ...,
        conformance: Conformance = ...,
        version: XsdVersion = ...,
        nodes_limit: int | None = ...,
        resolver: Resolver | None = ...,
    ) -> SchemaSet:
        """Detects the encoding: byte-order mark, then the XML declaration,
        then UTF-8."""
    def __len__(self) -> int:
        """How many global elements and types *this schema* declares.

        The XSD built-ins are excluded here, from ``in``, from iteration and
        from ``types``: they are present in every schema set and would bury
        what the documents actually declared. ``type()`` still resolves them.
        """
    def __contains__(self, name: Name, /) -> bool: ...
    def __getitem__(self, name: Name, /) -> Element | Type:
        """The element or type of that name, raising ``KeyError`` when there
        is none. The lookup methods return ``None`` instead, for when absence
        is an ordinary answer rather than a mistake."""
    def __iter__(self) -> Iterator[str]:
        """The global names in Clark notation, elements before types."""
    @property
    def documents(self) -> list[Document]: ...
    @property
    def elements(self) -> list[Element]:
        """Every global element declaration, by name.

        The declarations themselves, not ``(name, declaration)`` pairs — the
        name is on the declaration, and pairs made every caller write
        ``[0][1]``.
        """
    @property
    def types(self) -> list[Type]:
        """Every global type *this schema* declares, by name."""
    @property
    def counts(self) -> dict[str, int]:
        """Component tallies — types, elements, particles and the rest. Counts
        a great deal more than the globals ``len()`` reports."""
    def element(self, namespace: Name | None, local: str | None = ..., /) -> Element | None: ...
    def type(self, namespace: Name | None, local: str | None = ..., /) -> Type | None: ...
    def attribute(self, namespace: Name | None, local: str | None = ..., /) -> Attribute | None: ...
    def validate(self, xml: Instance, *, uri: str = ...) -> ValidationReport:
        """Validates a document. Never raises for an invalid one — that is an
        answer, not an error."""
    def iter_typed(self, xml: Instance, *, uri: str = ...) -> PsviEvents:
        """Reads a document into typed PSVI events, one at a time.

        The iterator form of ``read_typed``, and the one to reach for::

            for ev in schemas.iter_typed(xml):
                ...

        The outcome is on the iterator's ``report``, before or after the loop.
        """
    def read_typed(
        self,
        xml: Instance,
        *,
        on_event: Callable[[PsviEvent], None] | None = ...,
        uri: str = ...,
    ) -> tuple[list[PsviEvent] | None, ValidationReport]:
        """Reads a document into typed PSVI events.

        Returns the events as a list, or feeds them to ``on_event`` and
        returns ``None`` in their place.
        """

def load(
    path: str | os.PathLike[str],
    *,
    search_paths: Sequence[str] | None = ...,
    conformance: Conformance = ...,
    version: XsdVersion = ...,
    nodes_limit: int | None = ...,
    resolver: Resolver | None = ...,
) -> tuple[SchemaSet, list[Diagnostic]]:
    """Loads a schema and returns it *with* its diagnostics, rather than
    raising. For schemas expected to be imperfect."""

def load_string(
    xsd: str,
    *,
    uri: str = ...,
    search_paths: Sequence[str] | None = ...,
    conformance: Conformance = ...,
    version: XsdVersion = ...,
    nodes_limit: int | None = ...,
    resolver: Resolver | None = ...,
) -> tuple[SchemaSet, list[Diagnostic]]: ...

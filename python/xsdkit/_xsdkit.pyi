"""Type stubs for the native module.

Names here mirror `src/python.rs`. Keep them in step: `test_stubs.py` checks
that every exported name exists at runtime.
"""

import datetime
import decimal
import os
from typing import Any, Callable, Generic, Iterable, Iterator, Literal, Sequence, TypeVar

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
#: becomes the diagnostic, and the first one raised is the ``SchemaError``'s
#: ``__cause__``. ``KeyboardInterrupt`` and ``SystemExit`` end the build and
#: propagate as themselves.
#:
#: Replaces the filesystem rather than adding to it, so it is an alternative to
#: ``search_paths``, not a layer on top.
Resolver = Callable[[str, str | None], bytes | str | tuple[str, bytes | str]]

#: A document to validate: text, or bytes whose encoding xsdkit detects.
Instance = str | bytes | os.PathLike[str]
"""A document: XML as text, as bytes whose encoding is detected, or a path to
read it from. A ``str`` is always content — a path and a document cannot be
told apart once both are strings — so pass ``pathlib.Path`` for a file."""

#: An XSD value as its closest native Python type.
#:
#: Durations and gregorian fragments stay as their canonical lexical strings —
#: ``xs:duration`` has no lossless Python counterpart, since months and seconds
#: are not commensurable. ``xs:dayTimeDuration`` alone becomes a ``timedelta``.
#: A date or duration Python cannot hold stays lexical too: a year after 9999
#: or before 1, or 999,999,999 days or more.
#:
#: An ``xs:QName`` arrives as Clark notation (``{namespace}local``) with its
#: prefix already resolved against the document, since the prefix is a spelling
#: rather than part of the value.
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

class XsdError(Exception):
    #: Every diagnostic behind the error — from a failed build, or from a
    #: document `decode` refused. Empty rather than absent where a path
    #: raised without any, so reading it is always safe. That empty default
    #: is one tuple shared by every such error, so it cannot be appended to.
    diagnostics: Sequence[Diagnostic]

class SchemaError(XsdError):
    #: Every diagnostic from the failed build, not just the first.
    diagnostics: Sequence[Diagnostic]

class Span:
    @property
    def uri(self) -> str:
        """The document the diagnostic points into."""
    @property
    def line(self) -> int:
        """1-based line number, or 0 when unknown."""
    @property
    def label(self) -> str | None:
        """What this span is, when a diagnostic carries more than one."""

class Diagnostic:
    def _repr_html_(self) -> str:
        """The diagnostic as a compiler would print it."""

    def __eq__(self, other: object, /) -> bool:
        """Two diagnostics saying the same thing about the same place are equal."""
    def __hash__(self) -> int: ...

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
    @property
    def uri(self) -> str:
        """Where the document was read from."""
    @property
    def target_namespace(self) -> str | None:
        """Its ``targetNamespace``, absent when it declares none."""
    @property
    def chameleon(self) -> bool:
        """True when it had no ``targetNamespace`` of its own and was
        absorbed into its includer's."""
    @property
    def version(self) -> str | None:
        """The ``xs:schema`` ``version`` attribute, verbatim.

        The specification declares it as a bare token with no processing
        role, so it is reported rather than interpreted.
        """

class AppInfo:
    @property
    def source(self) -> str | None:
        """The ``source`` attribute, if the ``xs:appinfo`` carried one."""
    @property
    def xml(self) -> str:
        """The ``appinfo`` children, re-serialized with names in Clark
        notation."""

class Facets:
    """A set of facets on a simple type.

    Bounds and enumerations are the lexical forms the schema wrote, not typed
    values: a facet constrains the lexical space as much as the value space.
    Pass one through ``Type.validate`` for the value.
    """

    def _repr_html_(self) -> str:
        """The constraints in force, as a table."""

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
    def __iter__(self) -> Iterator[Child]:
        """The children, so ``for child in element`` reads."""
    def __getitem__(self, name: Name, /) -> Child:
        """The child of that name, raising ``KeyError`` when there is none.

        A local name is enough, since a child is almost always in its parent's
        namespace.
        """
    @property
    def children(self) -> list[Child]:
        """Elements that may appear directly inside this one.

        The same as ``element.type.children``, without the hop.
        """
    @property
    def attributes(self) -> list[AttributeUse]:
        """The attributes this element may carry, with how it may carry them."""
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

        Includes this one unless it is abstract, and excludes any the head's
        ``block`` bars from standing in — so this is what a document may name,
        not merely who is in the substitution group.
        """
    @property
    def default(self) -> str | None: ...
    @property
    def fixed(self) -> str | None: ...
    @property
    def doc(self) -> str | None: ...
    @property
    def appinfo(self) -> list[AppInfo]: ...

class Child:
    """An element as a child of one particular type.

    Everything :class:`Element` answers, this answers too, plus how often it
    may appear *here*. ``minOccurs`` and ``maxOccurs`` are written on the use
    rather than on the declaration, so one global element may be a repeating
    child of one type and a required single child of another::

        for child in report:
            print(child.local_name, child.repeats, child.optional)

        report["item"]["note"].optional      # True
    """

    @property
    def repeats(self) -> bool:
        """Whether it may appear here more than once."""
    @property
    def optional(self) -> bool:
        """Whether some valid content leaves it out."""
    @property
    def element(self) -> Element:
        """The declaration on its own, without this parent's occurrence."""
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
    def substitutes(self) -> list[Element]: ...
    @property
    def default(self) -> str | None: ...
    @property
    def fixed(self) -> str | None: ...
    @property
    def doc(self) -> str | None: ...
    @property
    def appinfo(self) -> list[AppInfo]: ...
    @property
    def children(self) -> list[Child]: ...
    @property
    def attributes(self) -> list[AttributeUse]: ...
    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[Child]: ...
    def __getitem__(self, name: Name, /) -> Child: ...
    def tree(self, depth: int = ...) -> Tree: ...
    def _repr_html_(self) -> str: ...

class Type:
    """A type definition, simple or complex."""

    def tree(self, depth: int = ...) -> Tree:
        """A readable tree of what may appear inside this type."""
    def _repr_html_(self) -> str: ...

    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[Child]: ...
    def __getitem__(self, name: Name, /) -> Child:
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
    def derives_from(self, other: Type, /) -> bool:
        """Whether this type is, or derives from, ``other``. Always ``False``
        for a type from another ``SchemaSet``."""
    @property
    def base_chain(self) -> list[Type]: ...
    @property
    def attributes(self) -> list[AttributeUse]: ...
    @property
    def children(self) -> list[Child]:
        """Elements that may appear directly inside, substitution groups
        expanded and inherited content included."""
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
    def name(self) -> tuple[str | None, str] | None:
        """``(namespace, local)``; ``None`` on a ``"text"`` event, which belongs
        to the element around it."""
    @property
    def local_name(self) -> str | None:
        """The local part of the name; ``None`` on a ``"text"`` event."""
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
    def from_schema(self) -> bool:
        """Whether the schema supplied this text.

        True on a ``"text"`` event when the element was empty and its
        declaration carried a ``default`` or ``fixed`` value, the same
        distinction :attr:`AttributeValue.from_schema` draws.
        """
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
    def __hash__(self) -> int:
        """Hashes as the string it equals."""
    def __add__(self, other: str, /) -> str: ...
    def __radd__(self, other: str, /) -> str: ...
    def _repr_html_(self) -> str: ...
    def splitlines(self) -> list[str]: ...
    def count(self, needle: str, /) -> int: ...

_C = TypeVar("_C")

class NamedComponents(Generic[_C]):
    """A schema's global components of one kind, in name order and by name.

    Iterates and indexes by position like a list, and looks up by name like a
    mapping::

        for t in schemas.types: ...
        schemas.types[0]
        schemas.types["{urn:example}Money"]
    """

    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[_C]:
        """The components, in name order."""
    def __getitem__(self, key: int | Name, /) -> _C:
        """By position, as in a list, or by name, raising ``KeyError`` — which
        says so when the name belongs to another kind of component."""
    def __contains__(self, item: object, /) -> bool:
        """Whether a name, or a component of this kind, is in the view."""
    def keys(self) -> list[str]:
        """The names, in Clark notation and in order."""
    def values(self) -> list[_C]:
        """The components, in the same order as ``keys``."""
    def items(self) -> list[tuple[str, _C]]:
        """``(name, component)`` pairs, in the same order as ``keys``."""
    def get(self, name: Name, default: Any = ...) -> _C | Any:
        """The component of that name, or ``default`` when there is none."""

class ChildIterator:
    def __iter__(self) -> Iterator[Child]: ...
    def __next__(self) -> Child: ...
    def __len__(self) -> int: ...

class NameIterator:
    def __iter__(self) -> Iterator[str]: ...
    def __next__(self) -> str: ...
    def __len__(self) -> int: ...

class ValidationReport:
    def _repr_html_(self) -> str:
        """A summary line and a table of what was found."""

    @property
    def is_valid(self) -> bool: ...
    @property
    def diagnostics(self) -> list[Diagnostic]: ...
    @property
    def errors(self) -> list[Diagnostic]: ...
    def __bool__(self) -> bool: ...

class SchemaSet:
    def _repr_html_(self) -> str:
        """Its documents and globals at a glance."""

    @classmethod
    def from_file(
        cls,
        path: str | os.PathLike[str],
        *,
        search_paths: Sequence[str] | None = ...,
        conformance: Conformance = ...,
        version: XsdVersion = ...,
        nodes_limit: int | None = ...,
        max_depth: int | None = ...,
        resolver: Resolver | None = ...,
    ) -> SchemaSet:
        """Raises `SchemaError` on any error diagnostic.

        ``version="1.1"`` turns on XSD 1.1: open content, conditional
        inclusion, assertions on wildcards, ``xs:precisionDecimal`` and the
        relaxed Unique Particle Attribution rule.

        ``max_depth`` caps how deeply elements nest in each schema document,
        256 by default. A deeper document is refused with ``XSD1001`` rather
        than parsed, because parsing recurses once per level.
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
        max_depth: int | None = ...,
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
        max_depth: int | None = ...,
        resolver: Resolver | None = ...,
    ) -> SchemaSet:
        """Detects the encoding: byte-order mark, then the XML declaration,
        then UTF-8."""
    def __len__(self) -> int:
        """How many global elements *this schema* declares.

        ``SchemaSet`` is a mapping of global elements. Types and attributes
        are separate symbol spaces — an element and a type often share a name
        — and have views of their own in ``types`` and ``attributes``.
        """
    def __contains__(self, name: object, /) -> bool: ...
    def __getitem__(self, name: Name, /) -> Element:
        """The global element of that name, raising ``KeyError`` when there is
        none — and saying so when the name belongs to a type or an attribute.
        The lookup methods return ``None`` instead, for when absence is an
        ordinary answer rather than a mistake."""
    def __iter__(self) -> Iterator[str]:
        """The global element names in Clark notation, sorted."""
    def get(self, name: Name, default: Any = ...) -> Element | Any:
        """The global element of that name, or ``default`` when there is none."""
    def keys(self) -> list[str]:
        """The global element names, sorted."""
    def values(self) -> list[Element]:
        """The global elements, in the same order as ``keys``."""
    def items(self) -> list[tuple[str, Element]]:
        """``(name, element)`` pairs, in the same order as ``keys``."""
    @property
    def documents(self) -> list[Document]: ...
    @property
    def elements(self) -> NamedComponents[Element]:
        """Every global element declaration, in name order and by name."""
    @property
    def types(self) -> NamedComponents[Type]:
        """Every global type *this schema* declares, in name order and by name.

        The XSD built-ins are excluded; ``type()`` still resolves them.
        """
    @property
    def attributes(self) -> NamedComponents[Attribute]:
        """Every global attribute *this schema* declares, in name order and by
        name. The ``xml:`` and ``xsi:`` attributes are excluded; ``attribute()``
        still resolves them."""
    @property
    def counts(self) -> dict[str, int]:
        """Component tallies — types, elements, particles and the rest. Counts
        a great deal more than the globals ``len()`` reports."""
    def element(self, namespace: Name | None, local: str | None = ..., /) -> Element | None: ...
    def type(self, namespace: Name | None, local: str | None = ..., /) -> Type | None: ...
    def attribute(self, namespace: Name | None, local: str | None = ..., /) -> Attribute | None: ...
    def validate(self, xml: Instance, *, uri: str | None = ...) -> ValidationReport:
        """Validates a document. Never raises for an invalid one — that is an
        answer, not an error.

        Diagnostics name ``uri``, or the file when ``xml`` is a path.
        """
    def decode(self, xml: Instance, *, uri: str | None = ..., lax: bool = ...) -> Any:
        """Decodes a document into Python data.

        Elements become dictionaries and values arrive in their value space::

            {"@id": "r-1",
             "title": "November orders",
             "issued": datetime.date(2024, 12, 1),
             "item": [{"@sku": "AB-1042",
                       "price": {"@currency": "EUR", "$": Decimal("19.95")}}]}

        A child the schema allows more than once is *always* a list — with two
        entries, one, or none — because the shape comes from the schema rather
        than from the document in front of you.

        Keys are local names, in Clark notation only where two names under one
        parent would collide. Attributes carry an ``@``; where an element has
        both a value and attributes the value sits under ``$``; ``xsi:nil``
        decodes to ``None``, or to ``None`` under ``$`` when the element also
        carries attributes.

        Raises ``XsdError`` if the document is invalid. Pass ``lax=True`` to
        take the data anyway.
        """
    def iter_typed(self, xml: Instance, *, uri: str | None = ...) -> PsviEvents:
        """Reads a document into typed PSVI events, as an iterator.

        The iterator form of ``read_typed``, and the one to reach for::

            for ev in schemas.iter_typed(xml):
                ...

        The outcome is on the iterator's ``report``, before or after the loop.
        Every event is built before the first is returned; for a document too
        large to hold that way, pass ``on_event`` to ``read_typed``.
        """
    def read_typed(
        self,
        xml: Instance,
        *,
        on_event: Callable[[PsviEvent], None] | None = ...,
        uri: str | None = ...,
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
    max_depth: int | None = ...,
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
    max_depth: int | None = ...,
    resolver: Resolver | None = ...,
) -> tuple[SchemaSet, list[Diagnostic]]: ...

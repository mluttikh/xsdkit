"""Type stubs for the native module.

Names here mirror `src/python.rs`. Keep them in step: `test_stubs.py` checks
that every exported name exists at runtime.
"""

import os
from typing import Any, Generic, Iterable, Iterator, Literal, Mapping, Sequence, TypeVar, final

# The aliases live in a runtime module, so code annotated with them can import
# them; imported here without `as`, so they are not re-exported from this one.
from .typing import (
    Conformance,
    ContentKind,
    EventKind,
    Instance,
    ModelKind,
    Name,
    Resolver,
    Severity,
    Use,
    Variety,
    XsdValue,
    XsdVersion,
)

__version__: str
# What the module exports, which stubtest checks against the runtime list.
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


class XsdError(Exception):
    """The base of the errors about schemas, documents and values. A wrong argument type raises `TypeError`, and a path that cannot be read raises `OSError`."""

    #: Every diagnostic behind the error — from a failed build, or from a
    #: document `decode` refused. Empty rather than absent where a path
    #: raised without any, so reading it is always safe. That empty default
    #: is one tuple shared by every such error, so it cannot be appended to.
    diagnostics: Sequence[Diagnostic]

class SchemaError(XsdError):
    """Raised when a schema cannot be built. Carries every diagnostic on `.diagnostics`."""

    #: Every diagnostic from the failed build, not just the first.
    diagnostics: Sequence[Diagnostic]

class DocumentError(XsdError):
    """Raised by `decode` for a document that does not satisfy its schema. Carries every diagnostic on `.diagnostics`."""

    #: Every diagnostic that made the document invalid.
    diagnostics: Sequence[Diagnostic]

class InvalidValueError(XsdError, ValueError):
    """Raised by `Type.validate` for a lexical form its type does not admit. Also a `ValueError`."""

@final
class Span:
    """Where in a document a diagnostic points, and what that place is.

    One diagnostic may carry several — an ambiguous content model names both
    particles that could match, and the labels say which is which.
    """

    @property
    def uri(self) -> str:
        """The document this points into."""
    @property
    def line(self) -> int:
        """The line, counting from one. Zero when the position is not known."""
    @property
    def label(self) -> str | None:
        """What this place *is*, when a diagnostic names more than one — "one
        candidate" and "the other", say.
        """

@final
class Diagnostic:
    """Something the reader found wrong, or worth saying.

    Carries a stable `code` to match on, a `message` for people, `spans` for
    where, and often `help` for what to do. `str()` renders the lot the way a
    compiler would.
    """

    def _repr_html_(self) -> str:
        """The diagnostic as a compiler would print it, coloured by severity."""

    def __eq__(self, other: object, /) -> bool:
        """Two diagnostics saying the same thing about the same place are equal."""
    def __hash__(self) -> int: ...

    @property
    def code(self) -> str:
        """The stable code, e.g. `"XSD1201"`."""
    @property
    def severity(self) -> Severity:
        """`"error"`, `"warning"` or `"note"`."""
    @property
    def message(self) -> str:
        """What is wrong, in a sentence."""
    @property
    def spans(self) -> list[Span]:
        """Where it is, sometimes in more than one place.

        An ambiguous content model names both particles that could match, and
        the labels say which is which.
        """
    @property
    def help(self) -> str | None:
        """What to do about it, when there is something useful to say."""
    @property
    def is_error(self) -> bool:
        """Whether this stops the schema loading, as opposed to a warning or a note."""

@final
class Document:
    """One schema document that went into the set.

    A schema is often many files — `xs:include` and `xs:import` pull in more —
    and this is the record of each, including which namespace it ended up in.
    """

    @property
    def uri(self) -> str:
        """Where this document was read from — a path, a URL, or whatever a
        custom resolver called it.
        """
    @property
    def target_namespace(self) -> str | None:
        """The namespace its declarations landed in, `None` for a no-namespace
        schema.
        """
    @property
    def chameleon(self) -> bool:
        """True when this document had no `targetNamespace` of its own and was
        absorbed into its includer's.
        """
    @property
    def version(self) -> str | None:
        """The `xs:schema` `version` attribute, verbatim. The specification gives
        it no structure and no meaning, so it is reported, not interpreted.
        """

@final
class AppInfo:
    """Machine-readable annotation content, kept verbatim."""

    @property
    def source(self) -> str | None:
        """The `source` attribute — a URI naming what convention the payload
        follows, when the schema said.
        """
    @property
    def xml(self) -> str:
        """The `appinfo` element's children, re-serialized. Element and attribute
        names are in Clark notation, so no prefix can be lost.
        """

@final
class Facets:
    """A set of facets on a simple type.

    The bounds and enumerations are kept as the lexical forms the schema wrote,
    not as typed values: a facet constrains the lexical space as much as the
    value space, and the string is what the document said. Pass one through
    `Type.validate` to get the value.
    """

    def _repr_html_(self) -> str:
        """The constraints in force, as a table of the ones that are set."""

    @property
    def length(self) -> int | None:
        """Exact length. Characters, or *items* for a list type."""
    @property
    def min_length(self) -> int | None:
        """Least length, in characters or list items."""
    @property
    def max_length(self) -> int | None:
        """Greatest length, in characters or list items."""
    @property
    def patterns(self) -> list[list[str]]:
        """Patterns as declared: the outer list is one entry per restriction
        step, **ANDed**; the inner alternatives at that step are **ORed**.
        """
    @property
    def enumeration(self) -> list[str] | None:
        """The permitted values, as the lexical forms the schema wrote.

        Compared in the value space, so an enumeration listing `1.0` admits
        `1.00`.
        """
    @property
    def white_space(self) -> str | None:
        """`"preserve"`, `"replace"` or `"collapse"` when stated explicitly."""
    @property
    def max_inclusive(self) -> str | None:
        """Upper bound, inclusive."""
    @property
    def max_exclusive(self) -> str | None:
        """Upper bound, exclusive."""
    @property
    def min_inclusive(self) -> str | None:
        """Lower bound, inclusive."""
    @property
    def min_exclusive(self) -> str | None:
        """Lower bound, exclusive."""
    @property
    def total_digits(self) -> int | None:
        """Most significant digits a decimal may have."""
    @property
    def fraction_digits(self) -> int | None:
        """Most digits a decimal may have after the point."""

@final
class Attribute:
    """An attribute declaration.

    The declaration itself, shared by every type that uses it. How a particular
    type uses it — required, optional, prohibited, with what default — is on
    `AttributeUse`, which is what `Type.attributes` returns.
    """

    @property
    def name(self) -> tuple[str | None, str]:
        """The name as a `(namespace, local)` pair."""
    @property
    def qname(self) -> str:
        """The name in Clark notation, `{namespace}local`."""
    @property
    def local_name(self) -> str:
        """The local part of the name, without its namespace."""
    @property
    def type(self) -> Type:
        """The simple type of this attribute's value."""
    @property
    def default(self) -> str | None:
        """The `default` value the schema supplies when the attribute is absent."""
    @property
    def fixed(self) -> str | None:
        """A schema-declared constant value — the case that can be resolved
        without seeing an instance document.
        """
    @property
    def doc(self) -> str | None:
        """The `xs:documentation` text, entries joined."""
    @property
    def appinfo(self) -> list[AppInfo]:
        """The `xs:appinfo` blocks, with their XML kept verbatim."""

@final
class AttributeUse:
    """An attribute declaration as used by one complex type."""

    @property
    def attribute(self) -> Attribute:
        """The declaration this use refers to.

        Several types may use one declaration, each with its own `use` and
        value constraint.
        """
    @property
    def name(self) -> tuple[str | None, str]:
        """The name as a `(namespace, local)` pair."""
    @property
    def local_name(self) -> str:
        """The local part of the name, without its namespace."""
    @property
    def type(self) -> Type:
        """The simple type of this attribute's value."""
    @property
    def required(self) -> bool:
        """Whether an instance must carry this attribute.

        The same question as `use == "required"`, asked the way it is usually
        asked.
        """
    @property
    def use(self) -> Use:
        """`"required"`, `"optional"` or `"prohibited"`."""
    @property
    def fixed(self) -> str | None:
        """The use's own fixed value, falling back to the declaration's."""
    @property
    def default(self) -> str | None:
        """The `default` for this use, which overrides the declaration's."""

@final
class Element:
    """An element declaration: a name, a type, and how it may appear.

    An element behaves as its children — iterable, sized, and subscriptable by
    name — so a schema is walked without a `.type` hop at every level:
    `report["item"]["price"]`, or `[child.local_name for child in report]`.

    A handle into the schema, not a copy — holding ten thousand of them costs
    ten thousand refcounts. Two handles to the same declaration compare equal
    and hash alike, so they work as dict keys and set members.

        >>> report = schemas.element("urn:example", "report")
        >>> report.children               # what may appear inside
        >>> report.substitutes            # what may appear *instead*
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

        The same as `element.type.children`, without the hop — an element's
        children are its type's, and browsing a schema should not have to say
        so at every level. Empty for a simple type.
        """
    @property
    def attributes(self) -> list[AttributeUse]:
        """The attributes this element may carry, with how it may carry them.

        The same as `element.type.attributes`, without the hop.
        """
    def _repr_html_(self) -> str:
        """Shows the tree in a notebook, where evaluating a value is how you look
        at it.

        Shallower than `tree()` on purpose: this fires on any cell that ends in
        an element, including by accident, so it shows the shape rather than
        the whole schema. Call `tree(depth=...)` for more.
        """
    def tree(self, depth: int = 3) -> Tree:
        """A readable tree of what may appear inside, for looking at a schema.

        Regular-expression markers for how often a child may appear — `?`
        optional, `+` one or more, `*` any number, nothing for exactly once —
        and `@name` for attributes, `?` when they are not required. Recursion
        stops where the shape repeats, so a section containing sections prints
        once rather than to the depth limit.

            >>> print(schemas["{urn:example}report"].tree())
            report
              title: xs:string
              item+
                @sku
                price: xs:decimal
                note?: xs:string
        """
    @property
    def name(self) -> tuple[str | None, str]:
        """`(namespace, local)`; the namespace is `None` when unqualified."""
    @property
    def qname(self) -> str:
        """The name in Clark notation, `{ns}local`."""
    @property
    def local_name(self) -> str:
        """The local part of the name, without its namespace."""
    @property
    def namespace(self) -> str | None:
        """The namespace URI, or `None` when the name is unqualified."""
    @property
    def type(self) -> Type:
        """The type in force for this element."""
    @property
    def nillable(self) -> bool:
        """Whether an instance may be empty by saying `xsi:nil="true"`.

        Nil is not the same as absent, and not the same as empty: it says the
        element is *present and has no value*.
        """
    @property
    def abstract(self) -> bool:
        """Whether this element may not appear itself.

        An abstract head exists to be substituted for — see `substitutes`.
        """
    @property
    def is_global(self) -> bool:
        """Whether this is a global declaration rather than one scoped to a type."""
    @property
    def substitutes(self) -> list[Element]:
        """Every element that may appear where this one is permitted, including
        itself when it is not abstract.

        Transitive, and with `block` applied — so this is what a document may
        actually name here, not merely who is in the substitution group. A
        head that blocks substitution has members that this does not list.
        """
    @property
    def default(self) -> str | None:
        """The `default` value, supplied when the element is present but empty."""
    @property
    def fixed(self) -> str | None:
        """The `fixed` value, which an instance may repeat but not contradict."""
    @property
    def doc(self) -> str | None:
        """The `xs:documentation` text, entries joined."""
    @property
    def appinfo(self) -> list[AppInfo]:
        """The `xs:appinfo` blocks, with their XML kept verbatim."""

@final
class Child:
    """An element as a child of one particular type.

    Everything `Element` answers, this answers too, plus how often it may
    appear *here*. That pairing is the point: `maxOccurs` and `minOccurs` are
    written on the use, not on the declaration, so one global element may be
    a repeating child of one type and a required single child of another.

    Both flags come from the same pass over the content model that produced
    the child list, so reading them costs nothing beyond the walk that was
    already done.
    """

    @property
    def repeats(self) -> bool:
        """Whether it may appear here more than once, which makes it a list when
        a document is decoded.
        """
    @property
    def optional(self) -> bool:
        """Whether some valid content leaves it out."""
    @property
    def element(self) -> Element:
        """The declaration on its own, without this parent's occurrence.

        Rarely needed — a `Child` answers everything an `Element` does — but
        it is what to compare against `SchemaSet["{ns}name"]`, which has no
        parent to have occurrence in.
        """
    @property
    def name(self) -> tuple[str | None, str]:
        """`(namespace, local)`; the namespace is `None` when unqualified."""
    @property
    def qname(self) -> str:
        """The name in Clark notation, `{ns}local`."""
    @property
    def local_name(self) -> str:
        """The local part of the name, without its namespace."""
    @property
    def namespace(self) -> str | None:
        """The namespace URI, or `None` when the name is unqualified."""
    @property
    def type(self) -> Type:
        """The type in force for this element."""
    @property
    def nillable(self) -> bool:
        """Whether an instance may say `xsi:nil="true"` here."""
    @property
    def abstract(self) -> bool:
        """Whether this element may not appear itself, only a substitute."""
    @property
    def is_global(self) -> bool:
        """Whether the declaration is global rather than scoped to a type."""
    @property
    def substitutes(self) -> list[Element]:
        """Every element that may stand in for this one. Transitive."""
    @property
    def default(self) -> str | None:
        """The `default` value, supplied when the element is present but empty."""
    @property
    def fixed(self) -> str | None:
        """The `fixed` value, which an instance may repeat but not contradict."""
    @property
    def doc(self) -> str | None:
        """The `xs:documentation` text, entries joined."""
    @property
    def appinfo(self) -> list[AppInfo]:
        """The `xs:appinfo` blocks, with their XML kept verbatim."""
    @property
    def children(self) -> list[Child]:
        """The elements that may appear inside this child, in turn."""
    @property
    def attributes(self) -> list[AttributeUse]:
        """The attributes this child may carry, with how it may carry them."""
    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[Child]: ...
    def __getitem__(self, name: Name, /) -> Child: ...
    def tree(self, depth: int = 3) -> Tree:
        """The shape below this child, `depth` levels deep."""
    def _repr_html_(self) -> str:
        """The element's tree, as a notebook shows it."""

@final
class Type:
    """A type definition, simple or complex.

    The centre of the model. A complex type answers what may appear inside it
    (`children`, `attributes`, `accepts`); a simple type answers what its
    values may be (`validate`, `facets`, `variety`). `is_complex` says which
    you have.

        >>> t = schemas.type("urn:example", "Sku")
        >>> t.validate("AB-1042")         # the typed value, or ValueError
        >>> t.facets.patterns             # the constraints in force
    """

    def tree(self, depth: int = 3) -> Tree:
        """A readable tree of what may appear inside this type.

        The same rendering as `Element.tree`, rooted at the type rather than at
        a declaration — so the first line is the type's name and the rest is
        its content.
        """
    def _repr_html_(self) -> str:
        """Shows the tree in a notebook. Shallower than `tree()`, as on `Element`."""

    def __len__(self) -> int: ...
    def __iter__(self) -> Iterator[Child]: ...
    def __getitem__(self, name: Name, /) -> Child:
        """The child element of that name, raising ``KeyError`` when absent."""
    @property
    def name(self) -> tuple[str | None, str] | None:
        """`(namespace, local)`, or `None` for an anonymous inline type."""
    @property
    def qname(self) -> str | None:
        """The name in Clark notation, or `None` for an anonymous type.

        A type declared inline inside an element has no name to report.
        """
    @property
    def is_complex(self) -> bool:
        """Whether this type may have attributes and child elements."""
    @property
    def is_simple(self) -> bool:
        """Whether this type has a value space — something `validate` can parse."""
    @property
    def abstract(self) -> bool:
        """Whether an instance may not use this type directly, only one derived from it."""
    @property
    def base(self) -> Type | None:
        """The type this one derives from, or `None` at `xs:anyType`."""
    @property
    def derivation(self) -> Literal["extension", "restriction"] | None:
        """`"extension"` or `"restriction"`; `None` for simple types."""
    def derives_from(self, other: Type, /) -> bool:
        """Whether this type is, or derives from, `other`.

        Always `False` for a type from another `SchemaSet`: a type in one set says
        nothing about a type in another.
        """
    @property
    def base_chain(self) -> list[Type]:
        """The base chain, from this type up to `xs:anyType`."""
    @property
    def attributes(self) -> list[AttributeUse]:
        """Attribute uses, with inherited attribute groups already flattened in."""
    @property
    def children(self) -> list[Child]:
        """Every element that may appear directly inside this type, with
        substitution groups expanded and inherited content included.

        Each one is a `Child`: the declaration, plus whether it may repeat and
        whether it may be left out. Those two belong to the pair rather than
        to the declaration — one global element may be used by several types
        under different bounds — and they come from the same single pass over
        the content model that found the children.
        """
    @property
    def content(self) -> ContentKind | None:
        """`"empty"`, `"simple"`, `"element-only"` or `"mixed"`; `None` for a
        simple type.
        """
    @property
    def content_model(self) -> ModelKind | None:
        """How the content model was compiled: `"empty"`, `"automaton"` or
        `"all"`.
        """
    def accepts(self, names: Iterable[Name], /) -> bool:
        """Whether a sequence of child names satisfies this type's content model.

        Names may be Clark notation, `(ns, local)` pairs, or bare local names,
        resolved against this type's children exactly as `type[name]` resolves
        them. A single `str` is refused rather than read one character at a
        time.
        """
    def validate(self, lexical: str, /, *, namespaces: Mapping[str, str] | None = None) -> XsdValue:
        """Validates a lexical form against this type, returning its typed value.

        `namespaces` maps prefixes to namespace URIs, `""` for the default
        namespace, for an `xs:QName`, whose value is whatever its prefix is
        bound to where it was written. A complex type with simple content
        validates against the simple type of that content.

        Raises `InvalidValueError`, which is also a `ValueError`, with the
        reason when the value is not valid.
        """
    def is_valid(self, lexical: str, /, *, namespaces: Mapping[str, str] | None = None) -> bool:
        """Whether a lexical form is valid against this type."""
    @property
    def variety(self) -> Variety | None:
        """`"atomic"`, `"list"` or `"union"`; `None` for a complex type."""
    @property
    def primitive(self) -> str | None:
        """The primitive this simple type reduces to, e.g. `"string"`."""
    @property
    def builtin(self) -> str | None:
        """The built-in this type *is*, if it is one."""
    @property
    def item_type(self) -> Type | None:
        """A list type's item type."""
    @property
    def member_types(self) -> list[Type]:
        """A union's member types, in the order they are tried."""
    @property
    def facets(self) -> Facets | None:
        """The facets in force, composed down the whole restriction chain.

        Not the ones this type declares — those are on
        `declared_facets`. A restriction inherits everything its base
        constrained, so a type that says only `maxLength` still has its base's
        `minLength`, and reporting the declared set alone disagrees with what
        `validate` does. For a complex type with simple content, the facets of
        that content's simple type.
        """
    @property
    def declared_facets(self) -> Facets | None:
        """The facets *this type* declares, without its base's.

        What the restriction step wrote, which is what a tool rendering a
        schema back wants. `facets` is what a validator applies.
        """
    @property
    def doc(self) -> str | None:
        """The `xs:documentation` text, entries joined."""
    @property
    def appinfo(self) -> list[AppInfo]:
        """The `xs:appinfo` blocks, with their XML kept verbatim.

        Kept as written, because a summary cannot be un-summarised: this is
        where a schema hides labels, mappings and anything else its authors agreed
        on.
        """

@final
class AttributeValue:
    """An attribute after validation."""

    @property
    def name(self) -> tuple[str | None, str]:
        """The name as a `(namespace, local)` pair."""
    @property
    def local_name(self) -> str:
        """The local part of the name, without its namespace."""
    @property
    def declaration(self) -> Attribute | None:
        """The declaration this matched, absent under a `skip` wildcard."""
    @property
    def value(self) -> XsdValue | None:
        """The typed value, or `None` when it did not validate."""
    @property
    def lexical(self) -> str:
        """The attribute exactly as the document wrote it."""
    @property
    def from_schema(self) -> bool:
        """True when the document did not spell this out and the schema supplied
        it from a `default` or `fixed` value.
        """

@final
class PsviEvent:
    """One post-schema-validation event.

    A single class with a `kind` discriminator rather than three, because the
    consuming loop is invariably a dispatch on kind.
    """

    @property
    def kind(self) -> EventKind:
        """`"start"`, `"text"` or `"end"`."""
    @property
    def name(self) -> tuple[str | None, str] | None:
        """The element's name as a `(namespace, local)` pair; `None` on a
        `"text"` event, which belongs to the element around it.
        """
    @property
    def local_name(self) -> str | None:
        """The local part of the name, without its namespace; `None` on a
        `"text"` event.
        """
    @property
    def declaration(self) -> Element | None:
        """The declaration this element matched.

        Absent under a `skip` wildcard, or a `lax` one with nothing to match.
        """
    @property
    def type(self) -> Type | None:
        """The type in force, after any `xsi:type` override."""
    @property
    def type_from_instance(self) -> bool:
        """Whether `xsi:type` chose the type, rather than the declaration."""
    @property
    def nil(self) -> bool:
        """Whether the element said `xsi:nil="true"`."""
    @property
    def attributes(self) -> list[AttributeValue]:
        """The attributes, typed, including any the schema supplied."""
    @property
    def value(self) -> XsdValue | None:
        """The typed value, on a `"text"` event."""
    @property
    def from_schema(self) -> bool:
        """Whether the schema supplied this text, because the element was empty
        and its declaration had a `default` or `fixed` value.
        """
    @property
    def lexical(self) -> str | None:
        """The character content exactly as the document wrote it."""
    @property
    def line(self) -> int:
        """The line the element started on, counting from one."""

@final
class PsviEvents:
    """An iterator over one document's typed events, read as it is validated.

    Validation runs on a thread of its own, so memory stays flat however large
    the document is, and an iterator dropped part way stops the reading. The
    outcome is on `report` once every event has been read.
    """

    def __iter__(self) -> Iterator[PsviEvent]: ...
    def __next__(self) -> PsviEvent: ...
    @property
    def report(self) -> ValidationReport:
        """The outcome, once every event has been read.

        Whether a document is valid is only known at its end, which is where
        this is too. Raises `RuntimeError` before then; to know first, call
        `validate`.
        """

@final
class Tree:
    r"""Rendered text that knows how to show itself.

    A plain `str` is the wrong return type for something meant to be *looked
    at*: a notebook displays `repr()` of the last expression, and `repr` of a
    string escapes every newline into `\n`. This renders as itself in a REPL,
    in a notebook, and through `print`.
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
    def _repr_html_(self) -> str:
        """Monospaced and with the whitespace kept, for Jupyter.

        The leading underscore is IPython's convention, not ours.
        """
    def splitlines(self) -> list[str]:
        """The lines, so a tree can be sliced and searched like the text it is."""
    def count(self, needle: str, /) -> int:
        """How many times `needle` occurs, as `str.count` would say."""

_C = TypeVar("_C")

@final
class NamedComponents(Generic[_C]):
    """A schema's global components of one kind, in name order and by name.

    Elements, types and attributes are separate symbol spaces, and an element
    and a type sharing a name is one of the most common patterns in XSD, so
    each kind has a view of its own rather than one mapping over all three. A
    view iterates its components and indexes them by position the way a list
    does, and looks them up by name the way a mapping does.
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
        """The components, in the same order as `keys`."""
    def items(self) -> list[tuple[str, _C]]:
        """`(name, component)` pairs, in the same order as `keys`."""
    def get(self, name: Name, default: Any = None) -> _C | Any:
        """The component of that name, or `default` when there is none."""

@final
class ChildIterator:
    """Walks a type's children."""

    def __iter__(self) -> Iterator[Child]: ...
    def __next__(self) -> Child: ...
    def __len__(self) -> int: ...

@final
class NameIterator:
    """Walks the global names of a `SchemaSet`.

    A snapshot rather than a live cursor: the model is immutable, so there is
    nothing to invalidate, and holding the names costs one allocation against
    the alternative of keeping an index into two maps in step.
    """

    def __iter__(self) -> Iterator[str]: ...
    def __next__(self) -> str: ...
    def __len__(self) -> int: ...

@final
class ValidationReport:
    """The outcome of validating a document."""

    def _repr_html_(self) -> str:
        """A summary line and a table of what was found.

        Long lists are cut off rather than filling the notebook: a document that
        is wrong in four hundred ways is not usefully read four hundred lines at a
        time.
        """

    @property
    def is_valid(self) -> bool:
        """Whether the document satisfied the schema.

        The report is falsy when it did not, so `if not report:` reads.
        """
    @property
    def diagnostics(self) -> list[Diagnostic]:
        """Everything found, warnings and notes included."""
    @property
    def errors(self) -> list[Diagnostic]:
        """Only the diagnostics that are errors."""
    def __bool__(self) -> bool: ...

@final
class SchemaSet:
    """A compiled set of schema components."""

    def _repr_html_(self) -> str:
        """What is in this schema set, at a glance.

        The documents it was built from and the globals they declare, which is
        what you want to see first on opening a schema you did not write.
        """

    def __copy__(self) -> SchemaSet:
        """A schema set cannot change, so a copy is the same object."""
    def __deepcopy__(self, memo: Any, /) -> SchemaSet: ...

    @classmethod
    def from_file(
        cls,
        path: str | os.PathLike[str],
        *,
        search_paths: Sequence[str | os.PathLike[str]] | None = None,
        conformance: Conformance = "strict",
        version: XsdVersion = "1.0",
        nodes_limit: int | None = None,
        max_depth: int | None = None,
        resolver: Resolver | None = None,
    ) -> SchemaSet:
        """Loads a schema from a file, following its includes and imports.

        Raises `SchemaError`, carrying every diagnostic, when the schema has
        errors; `load` returns them instead. `version="1.1"` reads XSD 1.1: open
        content, conditional inclusion, assertions on wildcards,
        `xs:precisionDecimal` and the relaxed Unique Particle Attribution rule.
        `max_depth` caps how deeply elements nest in each schema document, 256 by
        default; a deeper document is refused with `XSD1001` rather than parsed.
        """
    @classmethod
    def from_string(
        cls,
        xsd: str,
        *,
        uri: str = "<string>",
        search_paths: Sequence[str | os.PathLike[str]] | None = None,
        conformance: Conformance = "strict",
        version: XsdVersion = "1.0",
        nodes_limit: int | None = None,
        max_depth: int | None = None,
        resolver: Resolver | None = None,
    ) -> SchemaSet:
        """Loads a schema from a string. The text must already be decoded.

        Relative `schemaLocation` hints resolve against `uri`; with the default
        `uri`, against the working directory and `search_paths`.
        """
    @classmethod
    def from_bytes(
        cls,
        data: bytes,
        *,
        uri: str = "<bytes>",
        search_paths: Sequence[str | os.PathLike[str]] | None = None,
        conformance: Conformance = "strict",
        version: XsdVersion = "1.0",
        nodes_limit: int | None = None,
        max_depth: int | None = None,
        resolver: Resolver | None = None,
    ) -> SchemaSet:
        """Loads a schema from raw bytes, detecting the encoding.

        Prefer this over `from_string` when the encoding is not known to be
        UTF-8: a byte-order mark or the XML declaration decides it.
        """
    @classmethod
    def from_files(
        cls,
        paths: Iterable[str | os.PathLike[str]],
        *,
        search_paths: Sequence[str | os.PathLike[str]] | None = None,
        conformance: Conformance = "strict",
        version: XsdVersion = "1.0",
        nodes_limit: int | None = None,
        max_depth: int | None = None,
        resolver: Resolver | None = None,
    ) -> SchemaSet:
        """Loads a schema from several files at once, following each one's
        includes and imports into one set.

        For a schema with no single root document: a vendor bundle, or a
        directory of XSDs that import one another. A single path, or an empty
        list, is refused.
        """
    @classmethod
    def deserialize(cls, data: bytes) -> SchemaSet:
        """Reads back what `serialize` wrote.

        Raises `ValueError` for bytes that are not a serialized schema set, or
        that another xsdkit version wrote: a name is an index into the
        interner, so a schema set from another build means nothing here. Read
        only bytes you trust, as with `pickle`.
        """
    def serialize(self) -> bytes:
        """The compiled schema set as bytes, for `deserialize` to read back
        without compiling the schema again.

        For a cache, and for handing a schema set to another process: pickling
        goes through this. Only the xsdkit version that wrote the bytes reads
        them back, so key a cache on `xsdkit.__version__`.
        """
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
    def get(self, name: Name, default: Any = None) -> Element | Any:
        """The global element of that name, or `default` when there is none."""
    def keys(self) -> list[str]:
        """The global element names, sorted.

        Present so this really is a mapping: `dict(schemas)` needs `keys`
        alongside `__getitem__`.
        """
    def values(self) -> list[Element]:
        """The global elements, in the same order as `keys`."""
    def items(self) -> list[tuple[str, Element]]:
        """`(name, element)` pairs, in the same order as `keys`."""
    @property
    def documents(self) -> list[Document]:
        """The documents this schema set was built from."""
    @property
    def elements(self) -> NamedComponents[Element]:
        """Every global element declaration, in name order and by name.

        A view: iterate it or index it by position, as a list, or look a
        declaration up by name, as a mapping — `schemas.elements["{ns}name"]`.
        """
    @property
    def types(self) -> NamedComponents[Type]:
        """Every global type definition *this schema* declares, in name order and
        by name.

        An element and a type may share a name, which is why types have a view
        of their own. The XSD built-ins are excluded: they are in every schema
        set and would bury the ones the documents wrote. `type("{...}string")`
        still resolves them.
        """
    @property
    def attributes(self) -> NamedComponents[Attribute]:
        """Every global attribute declaration *this schema* declares, in name
        order and by name.

        The `xml:` and `xsi:` attributes every schema set carries are
        excluded; `attribute()` still resolves them.
        """
    @property
    def counts(self) -> dict[str, int]:
        """Component tallies — types, elements, particles and the rest — for
        diagnostics and smoke tests. Counts a great deal more than the globals
        `len()` reports.
        """
    def element(self, namespace: Name | None, local: str | None = None, /) -> Element | None:
        """Looks up a global element. `None` if there is none."""
    def type(self, namespace: Name | None, local: str | None = None, /) -> Type | None:
        """Looks up a global type. `None` if there is none."""
    def attribute(self, namespace: Name | None, local: str | None = None, /) -> Attribute | None:
        """Looks up a global attribute. `None` if there is none."""
    def validate(self, xml: Instance, *, uri: str | None = None) -> ValidationReport:
        """Validates an instance document against this schema.

        Never raises for an invalid document — an invalid document is an
        answer, not an error. Inspect `.is_valid` and `.diagnostics`.

        Diagnostics name `uri`, or the file when the document was given as a
        path.
        """
    def decode(
        self, xml: Instance, *, uri: str | None = None, lax: bool = False, root: bool = False
    ) -> Any:
        """Decodes a document into Python data.

        Elements become dictionaries, values arrive in their value space —
        `Decimal`, `datetime`, `int` — and a child the schema allows more than
        once is always a list, whether the document carries two of them, one,
        or none. That shape comes from the schema, so it does not change under
        you when a document leaves something out.

        Keys are local names, spelled out in Clark notation only where two
        names under one parent would otherwise collide. Attributes are
        prefixed with `@`, and where an element has both a value and
        attributes the value sits under `$`. `xsi:nil` decodes to `None`, or
        to `None` under `$` when the element carries attributes too.

        Raises `DocumentError` if the document is invalid; pass `lax=True` to take
        the data anyway. Unlike `validate`, this one raises, because a caller
        asking for data has said what it wants and silently handing back data
        from a document that does not fit its schema is the trap this is meant
        to remove. Text that is not XML at all raises even with `lax=True`:
        there is nothing in it to take.

        The result is the root element's content. Pass `root=True` for
        `{root name: content}`, which says which global element the document
        was; the key follows the same rule as every other.
        """
    def iter_typed(self, xml: Instance, *, uri: str | None = None) -> PsviEvents:
        """Reads a document into typed PSVI events, as it is validated.

        `for ev in schemas.iter_typed(xml)` composes with everything Python has
        for iterables. Validation runs on a thread of its own and hands events
        over a batch at a time, so memory stays flat however large the
        document is, and an iterator dropped part way stops the reading.

        The outcome is on `report` once every event has been read. To know
        whether a document is valid before reading it, call `validate`.
        """
    def read_typed(
        self, xml: Instance, *, uri: str | None = None
    ) -> tuple[list[PsviEvent], ValidationReport]:
        """Reads a document into typed PSVI events, all at once.

        Returns `(events, report)`: every event, as a list, and the outcome.
        Memory grows with the document; `iter_typed` reads one of any size.
        """

def load(
    path: str | os.PathLike[str],
    *,
    search_paths: Sequence[str | os.PathLike[str]] | None = None,
    conformance: Conformance = "lax",
    version: XsdVersion = "1.0",
    nodes_limit: int | None = None,
    max_depth: int | None = None,
    resolver: Resolver | None = None,
) -> tuple[SchemaSet, list[Diagnostic]]:
    """Loads a schema and returns it **with** its diagnostics, rather than
    raising.

    Use this when a schema is expected to be imperfect — a vendor schema with
    dangling imports, say — and you want the components anyway.
    """

def load_files(
    paths: Iterable[str | os.PathLike[str]],
    *,
    search_paths: Sequence[str | os.PathLike[str]] | None = None,
    conformance: Conformance = "lax",
    version: XsdVersion = "1.0",
    nodes_limit: int | None = None,
    max_depth: int | None = None,
    resolver: Resolver | None = None,
) -> tuple[SchemaSet, list[Diagnostic]]:
    """The same, from several root documents at once."""

def load_string(
    xsd: str,
    *,
    uri: str = "<string>",
    search_paths: Sequence[str | os.PathLike[str]] | None = None,
    conformance: Conformance = "lax",
    version: XsdVersion = "1.0",
    nodes_limit: int | None = None,
    max_depth: int | None = None,
    resolver: Resolver | None = None,
) -> tuple[SchemaSet, list[Diagnostic]]:
    """The same, from a string."""

def load_bytes(
    data: bytes,
    *,
    uri: str = "<bytes>",
    search_paths: Sequence[str | os.PathLike[str]] | None = None,
    conformance: Conformance = "lax",
    version: XsdVersion = "1.0",
    nodes_limit: int | None = None,
    max_depth: int | None = None,
    resolver: Resolver | None = None,
) -> tuple[SchemaSet, list[Diagnostic]]:
    """The same, from bytes whose encoding is detected."""

"""xsdkit against two other implementations, on generated schemas.

Hypothesis builds schemas from a subset of XSD 1.0 — builtin and restricted
simple types, sequences and choices nested and repeated, attributes with
defaults, simple content, nillable elements, `xs:anyType` and wildcards — and
documents for them, some broken on purpose. `validate` has to reach lxml's
verdict (libxml2), and `decode` has to give what `xmlschema.to_dict` gives
once the two conventions are lined up.

The first run found two bugs in the content model that no hand-written test
had: `minOccurs="2" maxOccurs="unbounded"` refused three children, and a
child repeated only by a bounded group decoded as a value.

Neither of the others is an oracle. What they get wrong is kept out of the
generated subset, or checked another way, with a note where it is — not let
through by the comparison — so a disagreement is an xsdkit bug or a new note:

- libxml2 rejects whitespace around a date, a dateTime or INF, though those
  types collapse it; accepts an empty `xs:NMTOKENS`, though the type needs a
  token; and does not hold a list to a fixed value when either is empty;
- xmlschema rejects some valid counted content — four `e` against
  `(e{2,3})*` — so it is not asked for verdicts, and a document it will not
  decode is not compared;
- xmlschema runs repeated elements of a list type together — two empty `e`
  and one `<e>0</e>` come out as `[[None, None], [0]]` — so the decode
  comparison gives elements no list types;
- xmlschema does not list every child that repeats only through its groups —
  `e4` in `(e1*, (e2*, e4?)*)?` is a list when it appears twice and a value
  when it appears once — so whether a child comes back as a list is checked
  against the generated schema instead;
- a QName default or fixed value resolves in the schema's namespaces and the
  attribute in the document's, and the engines disagree on comparing them.

CI runs the same examples every time, so a failure there reproduces. For a
longer search with fresh ones:

    XSDKIT_DIFFERENTIAL=thorough pytest python/tests/test_differential.py
"""

import datetime
import decimal
import math
import os
import re
import sysconfig
from dataclasses import dataclass, field
from typing import NamedTuple
from xml.sax.saxutils import escape, quoteattr

import pytest
import xmlschema
from hypothesis import HealthCheck, assume, given, settings
from hypothesis import strategies as st

import xsdkit
from conftest import NS

# lxml is imported by the one test that uses it, and that test does not run on
# a free-threaded build: importing lxml there turns the GIL back on for the
# rest of the run, which is the one thing that build is here to test without.
FREE_THREADED = bool(sysconfig.get_config_var("Py_GIL_DISABLED"))

OTHER = "urn:other"
XSI = "http://www.w3.org/2001/XMLSchema-instance"
# The prefixes every generated document declares on its root.
PREFIXES = {"": NS, "o": OTHER, "xsi": XSI}

UNCHECKED = [HealthCheck.too_slow, HealthCheck.data_too_large, HealthCheck.large_base_example]
settings.register_profile(
    "ci",
    # The same examples on every run, so a failure in CI fails the same way
    # on a laptop.
    derandomize=True,
    max_examples=150,
    deadline=None,
    database=None,
    suppress_health_check=UNCHECKED,
)
settings.register_profile("thorough", max_examples=3000, deadline=None, suppress_health_check=UNCHECKED)
PROFILE = settings.get_profile(os.environ.get("XSDKIT_DIFFERENTIAL", "ci"))

# ---------------------------------------------------------------------------
# Simple types
# ---------------------------------------------------------------------------

WORD = st.text(alphabet="abcdefXYZ", min_size=1, max_size=6)
TEXT = st.text(alphabet="abc XYZ019-_.", max_size=10)


@dataclass
class Simple:
    """A simple type: a builtin named with `type="…"`, or one spelled out."""

    ref: str | None
    inline: str
    valid: st.SearchStrategy[str]
    invalid: st.SearchStrategy[str] | None


def padded(values: st.SearchStrategy[str]) -> st.SearchStrategy[str]:
    """Whitespace around a value, which every type but `xs:string` collapses."""
    return st.tuples(st.sampled_from(["", " ", "\n "]), values, st.sampled_from(["", " "])).map("".join)


def dates() -> st.SearchStrategy[str]:
    return st.tuples(
        st.dates(datetime.date(1000, 1, 1), datetime.date(9999, 12, 31)).map(datetime.date.isoformat),
        st.sampled_from(["", "", "Z", "+01:00", "-05:30"]),
    ).map("".join)


def date_times() -> st.SearchStrategy[str]:
    return st.tuples(
        st.dates(datetime.date(1000, 1, 1), datetime.date(9999, 12, 31)).map(datetime.date.isoformat),
        st.just("T"),
        st.times().map(lambda t: t.isoformat("seconds")),
        st.sampled_from(["", ".5", ".123456"]),
        st.sampled_from(["", "", "Z", "+01:00", "-05:30", "+14:00"]),
    ).map("".join)


def decimals() -> st.SearchStrategy[str]:
    return st.tuples(
        st.sampled_from(["", "-", "+"]),
        st.integers(0, 10**12).map(str),
        st.sampled_from(["", ".", ".0", ".50", ".125"]),
    ).map("".join)


def doubles() -> st.SearchStrategy[str]:
    return st.one_of(
        padded(st.floats(allow_nan=False, allow_infinity=False).map(repr)),
        padded(st.sampled_from(["1E3", "-0", ".5", "5."])),
        # No whitespace around INF: libxml2 rejects it.
        st.sampled_from(["INF", "-INF"]),
    )


# Valid lexical forms, and forms that are not.
BUILTINS: dict[str, tuple[st.SearchStrategy[str], st.SearchStrategy[str] | None]] = {
    "xs:string": (TEXT, None),
    "xs:token": (TEXT, None),
    "xs:int": (
        padded(st.integers(-(2**31), 2**31 - 1).map(str)),
        st.sampled_from(["", "1.5", "abc", "2147483648", "-2147483649"]),
    ),
    "xs:integer": (padded(st.integers(-(10**20), 10**20).map(str)), st.sampled_from(["1.0", "x", "", "1e3"])),
    "xs:nonNegativeInteger": (padded(st.integers(0, 10**12).map(str)), st.sampled_from(["-1", "x"])),
    "xs:positiveInteger": (padded(st.integers(1, 10**12).map(str)), st.sampled_from(["0", "-3", "+0"])),
    "xs:decimal": (padded(decimals()), st.sampled_from(["1.2.3", "1e3", "abc", "", "INF"])),
    "xs:boolean": (padded(st.sampled_from(["true", "false", "1", "0"])), st.sampled_from(["yes", "True", "", "2"])),
    "xs:double": (doubles(), st.sampled_from(["inf", "1,5", "", "e3"])),
    # No whitespace around dates: libxml2 rejects it.
    "xs:date": (dates(), st.sampled_from(["2024-13-01", "2024-02-30", "24-01-01", "2024-1-1"])),
    "xs:dateTime": (date_times(), st.sampled_from(["2024-01-01", "2024-01-01T25:00:00", "2024-01-01T12:00"])),
    "xs:NMTOKENS": (
        st.lists(st.from_regex(r"[A-Za-z0-9._-]{1,5}", fullmatch=True), min_size=1, max_size=3).map(" ".join),
        # Never empty: libxml2 accepts that.
        st.just("a b!"),
    ),
    "xs:QName": (st.sampled_from(["local", "o:thing"]), st.sampled_from(["1abc", "a:b:c", "undeclared:x"])),
}

# XSD patterns, and the Python expression for their values. XSD's `\w` leaves
# out punctuation, `_` included, so the values stay alphanumeric.
PATTERNS = [
    (r"[A-Z]{2}-[0-9]{3}", r"[A-Z]{2}-[0-9]{3}"),
    (r"\d{1,3}(\.\d{2})?", r"[0-9]{1,3}(\.[0-9]{2})?"),
    (r"[a-z]+(_[a-z]+)*", r"[a-z]{1,6}(_[a-z]{1,6}){0,3}"),
    (r"\w+@\w+\.[a-z]{2,3}", r"[a-zA-Z0-9]{1,6}@[a-zA-Z0-9]{1,6}\.[a-z]{2,3}"),
]


def builtin(name: str) -> Simple:
    valid, invalid = BUILTINS[name]
    return Simple(name, "", valid, invalid)


def builtin_names(lists: bool) -> list[str]:
    return [n for n in sorted(BUILTINS) if lists or n != "xs:NMTOKENS"]


def restricted(base: str, facets: str) -> str:
    return f'<xs:simpleType><xs:restriction base="{base}">{facets}</xs:restriction></xs:simpleType>'


@st.composite
def simple_types(draw, lists: bool = True) -> Simple:
    kinds = ["builtin"] * 4 + ["enumeration", "range", "length", "pattern"] + (["list"] if lists else [])
    kind = draw(st.sampled_from(kinds))
    if kind == "builtin":
        return builtin(draw(st.sampled_from(builtin_names(lists))))
    if kind == "enumeration":
        values = draw(st.lists(WORD, min_size=1, max_size=4, unique=True))
        facets = "".join(f"<xs:enumeration value={quoteattr(v)}/>" for v in values)
        return Simple(None, restricted("xs:string", facets), st.sampled_from(values), WORD.filter(lambda w: w not in values))
    if kind == "range":
        lo = draw(st.integers(-1000, 1000))
        hi = draw(st.integers(lo, lo + 2000))
        facets = f'<xs:minInclusive value="{lo}"/><xs:maxInclusive value="{hi}"/>'
        return Simple(None, restricted("xs:int", facets), st.integers(lo, hi).map(str), st.sampled_from([str(lo - 1), str(hi + 1)]))
    if kind == "length":
        lo = draw(st.integers(0, 4))
        hi = draw(st.integers(max(lo, 1), 8))
        facets = f'<xs:minLength value="{lo}"/><xs:maxLength value="{hi}"/>'
        return Simple(
            None,
            restricted("xs:string", facets),
            st.text(alphabet="abcXYZ", min_size=lo, max_size=hi),
            st.text(alphabet="abc", min_size=hi + 1, max_size=hi + 3),
        )
    if kind == "pattern":
        pattern, python = draw(st.sampled_from(PATTERNS))
        facets = f"<xs:pattern value={quoteattr(pattern)}/>"
        return Simple(None, restricted("xs:string", facets), st.from_regex(python, fullmatch=True), st.sampled_from(["", "!!", "ab-12"]))
    return Simple(
        None,
        '<xs:simpleType><xs:list itemType="xs:int"/></xs:simpleType>',
        st.lists(st.integers(-99, 99).map(str), max_size=3).map(" ".join),
        st.sampled_from(["1 x", "1.5"]),
    )


# ---------------------------------------------------------------------------
# Complex types, as a model the documents are generated from
# ---------------------------------------------------------------------------


@dataclass
class Attribute:
    name: str
    type: Simple
    use: str
    default: str | None = None
    fixed: str | None = None


@dataclass
class Wildcard:
    min: int
    max: int | None
    process: str


@dataclass
class Element:
    name: str
    min: int
    max: int | None
    # A simple type, a complex one, or None for `xs:anyType`.
    type: "Simple | Complex | None"
    nillable: bool = False


@dataclass
class Group:
    compositor: str
    min: int
    max: int | None
    items: list["Element | Group | Wildcard"]


@dataclass
class Complex:
    # Element content, or simple content with a builtin base.
    content: Group | None
    simple: Simple | None
    attributes: list[Attribute]
    any_attribute: str | None = None


@dataclass
class Model:
    """What is being generated, and the names used so far.

    Every element and attribute gets a name of its own, so no content model
    is ambiguous and no two children collide.
    """

    wild: bool
    names: dict[str, int] = field(default_factory=dict)

    def fresh(self, prefix: str) -> str:
        self.names[prefix] = self.names.get(prefix, 0) + 1
        return f"{prefix}{self.names[prefix]}"


def occurs(draw, lo_max: int = 2) -> tuple[int, int | None]:
    lo = draw(st.integers(0, lo_max))
    # Small bounds: libxml2 unrolls counted content, and a large one is slow.
    return lo, draw(st.one_of(st.none(), st.integers(max(lo, 1), 3)))


@st.composite
def attributes(draw, model: Model) -> list[Attribute]:
    out = []
    for _ in range(draw(st.integers(0, 3))):
        t = draw(simple_types())
        use = draw(st.sampled_from(["optional", "optional", "required"]))
        default = fixed = None
        # A QName default resolves in the schema's namespaces, where the
        # attribute resolves in the document's; engines disagree on that.
        if use == "optional" and t.ref != "xs:QName":
            constraint = draw(st.sampled_from([None, None, "default", "fixed"]))
            if constraint == "default":
                default = draw(t.valid)
            elif constraint == "fixed":
                # Never empty: libxml2 does not hold a list to an empty fixed value.
                fixed = draw(t.valid.filter(str.strip))
        out.append(Attribute(model.fresh("a"), t, use, default, fixed))
    return out


@st.composite
def complex_types(draw, model: Model, depth: int) -> Complex:
    attrs = draw(attributes(model))
    any_attribute = draw(st.sampled_from([None] * 4 + ["lax", "skip"])) if model.wild else None
    if draw(st.integers(0, 3)) == 0:
        return Complex(None, builtin(draw(st.sampled_from(builtin_names(model.wild)))), attrs, any_attribute)
    return Complex(draw(groups(model, depth)), None, attrs, any_attribute)


@st.composite
def groups(draw, model: Model, depth: int) -> Group:
    compositor = draw(st.sampled_from(["sequence", "sequence", "choice"]))
    lo, hi = occurs(draw, lo_max=1)
    items: list[Element | Group | Wildcard] = []
    for _ in range(draw(st.integers(1, 4))):
        pick = draw(st.integers(0, 9))
        if pick == 0 and depth < 2:
            items.append(draw(groups(model, depth + 1)))
        elif pick == 1 and model.wild and compositor == "sequence" and not any(isinstance(i, Wildcard) for i in items):
            # `##other` only: the elements are all in the target namespace,
            # so a wildcard never competes with one.
            wlo, whi = occurs(draw, lo_max=1)
            items.append(Wildcard(wlo, whi, draw(st.sampled_from(["lax", "skip"]))))
        else:
            items.append(draw(elements(model, depth + 1)))
    return Group(compositor, lo, hi, items)


@st.composite
def elements(draw, model: Model, depth: int) -> Element:
    lo, hi = occurs(draw)
    pick = draw(st.integers(0, 9))
    t: Simple | Complex | None
    if pick < 5 or depth >= 3:
        t = draw(simple_types(lists=model.wild))
    elif pick == 5 and model.wild:
        t = None
    else:
        t = draw(complex_types(model, depth))
    return Element(model.fresh("e"), lo, hi, t, nillable=draw(st.integers(0, 6)) == 0)


# ---------------------------------------------------------------------------
# Schema text
# ---------------------------------------------------------------------------


def occurs_attrs(lo: int, hi: int | None) -> str:
    return f' minOccurs="{lo}" maxOccurs="{"unbounded" if hi is None else hi}"'


def render_attribute(a: Attribute) -> str:
    kind = f' type="{a.type.ref}"' if a.type.ref else ""
    constraint = "".join(f" {k}={quoteattr(v)}" for k, v in (("default", a.default), ("fixed", a.fixed)) if v is not None)
    return f'<xs:attribute name="{a.name}"{kind} use="{a.use}"{constraint}>{a.type.inline}</xs:attribute>'


def render_complex(c: Complex) -> str:
    attrs = "".join(render_attribute(a) for a in c.attributes)
    if c.any_attribute:
        attrs += f'<xs:anyAttribute namespace="##other" processContents="{c.any_attribute}"/>'
    if c.simple is not None:
        return (
            f'<xs:complexType><xs:simpleContent><xs:extension base="{c.simple.ref}">{attrs}'
            "</xs:extension></xs:simpleContent></xs:complexType>"
        )
    assert c.content is not None
    return f"<xs:complexType>{render_group(c.content)}{attrs}</xs:complexType>"


def render_group(g: Group) -> str:
    inner = "".join(render_particle(i) for i in g.items)
    return f"<xs:{g.compositor}{occurs_attrs(g.min, g.max)}>{inner}</xs:{g.compositor}>"


def render_particle(p: "Element | Group | Wildcard") -> str:
    if isinstance(p, Group):
        return render_group(p)
    if isinstance(p, Wildcard):
        return f'<xs:any namespace="##other" processContents="{p.process}"{occurs_attrs(p.min, p.max)}/>'
    return render_element(p, occurs_attrs(p.min, p.max))


def render_element(e: Element, occurs: str = "") -> str:
    extra = occurs + (' nillable="true"' if e.nillable else "")
    if e.type is None:
        return f'<xs:element name="{e.name}" type="xs:anyType"{extra}/>'
    if isinstance(e.type, Simple):
        kind = f' type="{e.type.ref}"' if e.type.ref else ""
        return f'<xs:element name="{e.name}"{kind}{extra}>{e.type.inline}</xs:element>'
    return f'<xs:element name="{e.name}"{extra}>{render_complex(e.type)}</xs:element>'


# ---------------------------------------------------------------------------
# Documents
# ---------------------------------------------------------------------------


class Writer:
    """Writes a document for a model, breaking it here and there if asked."""

    def __init__(self, draw, broken: bool):
        self.draw = draw
        self.broken = broken

    def breaks(self) -> bool:
        return self.broken and self.draw(st.integers(0, 9)) == 0

    def value(self, t: Simple) -> str:
        if t.invalid is not None and self.breaks():
            return self.draw(t.invalid)
        return self.draw(t.valid)

    def count(self, lo: int, hi: int | None) -> int:
        top = 3 if hi is None else hi
        if self.breaks():
            return self.draw(st.sampled_from([n for n in (lo - 1, top + 1) if n >= 0]))
        return self.draw(st.integers(lo, min(top, lo + 2)))

    def attributes(self, c: Complex) -> str:
        out = []
        for a in c.attributes:
            present = a.use == "required" or self.draw(st.booleans())
            if a.use == "required" and self.breaks():
                present = False
            if present:
                if a.fixed is None:
                    value = self.value(a.type)
                elif self.breaks():
                    # Some other value, and not an empty one: libxml2 does not
                    # hold a list to a fixed value when either is empty.
                    value = self.draw(a.type.valid.filter(str.strip))
                else:
                    value = a.fixed
                out.append(f" {a.name}={quoteattr(value)}")
        if c.any_attribute and self.draw(st.booleans()):
            out.append(' o:w="1"')
        if self.breaks():
            out.append(' undeclared="1"')
        return "".join(out)

    def element(self, e: Element) -> str:
        name = e.name
        if e.nillable and self.draw(st.integers(0, 3)) == 0:
            return f'<{name} xsi:nil="true"/>'
        if e.type is None:
            return f'<{name} q="1">t<{name}x>u</{name}x><o:k>v</o:k></{name}>'
        if isinstance(e.type, Simple):
            return f"<{name}>{escape(self.value(e.type))}</{name}>"
        c = e.type
        if c.simple is not None:
            return f"<{name}{self.attributes(c)}>{escape(self.value(c.simple))}</{name}>"
        assert c.content is not None
        body = self.group(c.content)
        if self.breaks():
            body += "<stray/>"
        return f"<{name}{self.attributes(c)}>{body}</{name}>"

    def group(self, g: Group) -> str:
        out = []
        for _ in range(self.count(g.min, g.max)):
            chosen = g.items if g.compositor == "sequence" else [self.draw(st.sampled_from(g.items))]
            out.extend(self.particle(p) for p in chosen)
        if g.compositor == "sequence" and len(out) > 1 and self.breaks():
            out.reverse()
        return "".join(out)

    def particle(self, p: "Element | Group | Wildcard") -> str:
        if isinstance(p, Group):
            return self.group(p)
        if isinstance(p, Wildcard):
            return "<o:w>x</o:w>" * self.count(p.min, p.max)
        return "".join(self.element(p) for _ in range(self.count(p.min, p.max)))


class Case(NamedTuple):
    xsd: str
    doc: str
    # The children the schema lets appear more than once, worked out from the
    # model rather than asked of xsdkit.
    repeating: frozenset[str]


def repeating(c: Complex) -> set[str]:
    """The children a type and those inside it may repeat.

    A child repeats when its own `maxOccurs` or that of a group around it is
    more than one. With every element named once in the whole schema, that is
    all there is to it.
    """
    names: set[str] = set()

    def walk(p: "Element | Group | Wildcard", repeated: bool) -> None:
        if isinstance(p, Group):
            for item in p.items:
                walk(item, repeated or p.max != 1)
        elif isinstance(p, Element):
            if repeated or p.max != 1:
                names.add(p.name)
            if isinstance(p.type, Complex) and p.type.content is not None:
                walk(p.type.content, False)

    if c.content is not None:
        walk(c.content, False)
    return names


@st.composite
def cases(draw, broken: bool, wild: bool) -> Case:
    """A schema and a document for it.

    `broken` lets the document go wrong in small ways — a bad value, a count
    out of range, a missing or undeclared attribute, children out of order.
    `wild` lets in wildcards, `xs:anyType` and elements of a list type, which
    the two decoders spell differently.
    """
    model = Model(wild)
    top = draw(complex_types(model, 0))
    root = Element("root", 1, 1, top)
    xsd = (
        f'<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{NS}" '
        f'elementFormDefault="qualified">{render_element(root)}</xs:schema>'
    )
    namespaces = "".join(f' xmlns{":" + p if p else ""}="{uri}"' for p, uri in PREFIXES.items())
    doc = Writer(draw, broken).element(root).replace("<root", f"<root{namespaces}", 1)
    return Case(xsd, doc, frozenset(repeating(top)))


# ---------------------------------------------------------------------------
# Comparing decoded values
# ---------------------------------------------------------------------------

DATE = re.compile(r"\s*(-?\d{4,})-(\d\d)-(\d\d)(?:T(\d\d):(\d\d):(\d\d)(?:\.(\d+))?)?(Z|[+-]\d\d:\d\d)?\s*")


def moment(v) -> tuple | None:
    """A date or dateTime as comparable fields, from a value or its lexical form."""
    if isinstance(v, datetime.datetime):
        offset = v.utcoffset()
        minutes = None if offset is None else int(offset.total_seconds() // 60)
        return (v.year, v.month, v.day, v.hour, v.minute, v.second, v.microsecond, minutes)
    if isinstance(v, datetime.date):
        return (v.year, v.month, v.day, None, None, None, None, None)
    m = DATE.fullmatch(v) if isinstance(v, str) else None
    if m is None:
        return None
    y, mo, d, h, mi, s, fraction, zone = m.groups()
    minutes = None
    if zone == "Z":
        minutes = 0
    elif zone:
        minutes = (-1 if zone[0] == "-" else 1) * (int(zone[1:3]) * 60 + int(zone[4:6]))
    clock = (int(h), int(mi), int(s), int((fraction or "").ljust(6, "0")[:6])) if h else (None,) * 4
    return (int(y), int(mo), int(d), *clock, minutes)


def clark(lexical: str) -> str | None:
    """A QName's lexical form in Clark notation, with the documents' prefixes."""
    prefix, _, local = lexical.rpartition(":")
    uri = PREFIXES.get(prefix)
    return None if uri is None else f"{{{uri}}}{local}"


def misshapen(value, repeating: frozenset[str], path: str = "") -> str | None:
    """Where a child's shape does not follow the schema, or None.

    A child the schema lets repeat is a list however many times it appears,
    and any other child is a value. That is checked against the model the
    schema was generated from, so it tests xsdkit's rule rather than
    restating it.
    """
    if isinstance(value, list):
        for i, item in enumerate(value):
            why = misshapen(item, repeating, f"{path}[{i}]")
            if why:
                return why
    if isinstance(value, dict):
        for key, item in value.items():
            if key.startswith(("@", "$")):
                continue
            if isinstance(item, list) != (key in repeating):
                shape = "a list" if isinstance(item, list) else "a value"
                rule = "repeats" if key in repeating else "does not repeat"
                return f"{path}/{key}: {shape}, where the schema says it {rule}"
            why = misshapen(item, repeating, f"{path}/{key}")
            if why:
                return why
    return None


def differs(ours, theirs, path: str = "") -> str | None:
    """Where xsdkit's decoded value and xmlschema's part, or None.

    The two differ by convention in five places, lined up here and nowhere
    else:

    - an element with nothing in it is None to xmlschema, and the empty value
      of its type here: `""`, `[]`, or a dict of empty lists;
    - an absent repeating child is `[]` here and has no key there, and nor
      does empty simple content under `"$"`;
    - nil is None here, or None under `"$"` beside attributes, and an
      `"@nil"` key there;
    - dates, times and QNames are values or Clark notation here, and lexical
      forms there;
    - a child that repeats is always a list here, and at times a value there
      when it appears once; `misshapen` checks the shape against the schema.
    """
    if theirs is None and (ours in ("", []) or (isinstance(ours, dict) and all(v == [] for v in ours.values()))):
        return None
    if ours == [] and theirs == [None]:
        return None
    if ours is None and theirs in ({"@nil": "true"}, [{"@nil": "true"}]):
        return None
    if isinstance(ours, list) and len(ours) == 1 and not isinstance(theirs, list):
        return differs(ours[0], theirs, f"{path}[0]")
    if isinstance(ours, dict) and isinstance(theirs, dict):
        theirs = dict(theirs)
        if theirs.pop("@nil", None) == "true":
            theirs.setdefault("$", None)
        for key in ours.keys() | theirs.keys():
            if key not in theirs:
                if ours[key] == [] or (key == "$" and ours[key] == ""):
                    continue
                return f"{path}/{key}: only xsdkit has it, {ours[key]!r}"
            if key not in ours:
                return f"{path}/{key}: only xmlschema has it, {theirs[key]!r}"
            why = differs(ours[key], theirs[key], f"{path}/{key}")
            if why:
                return why
        return None
    if isinstance(ours, list) and isinstance(theirs, list):
        if len(ours) != len(theirs):
            return f"{path}: {len(ours)} items from xsdkit, {len(theirs)} from xmlschema"
        for i, (a, b) in enumerate(zip(ours, theirs)):
            why = differs(a, b, f"{path}[{i}]")
            if why:
                return why
        return None
    if type(ours) is type(theirs):
        if isinstance(ours, float) and math.isnan(ours) and math.isnan(theirs):
            return None
        # A decimal keeps the scale it was written with, so 4.50 is not 4.5.
        # xs:decimal has no negative zero, and xsdkit drops the sign of -0.0.
        if (
            isinstance(ours, decimal.Decimal)
            and ours == theirs
            and ours.as_tuple().exponent == theirs.as_tuple().exponent
        ):
            return None
        if not isinstance(ours, decimal.Decimal) and ours == theirs:
            return None
    if isinstance(ours, str) and isinstance(theirs, str) and ours.startswith("{") and ours == clark(theirs):
        return None
    if moment(ours) is not None and moment(ours) == moment(theirs):
        return None
    return f"{path}: xsdkit {ours!r} ({type(ours).__name__}), xmlschema {theirs!r} ({type(theirs).__name__})"


# ---------------------------------------------------------------------------
# The tests
# ---------------------------------------------------------------------------


@pytest.mark.skipif(FREE_THREADED, reason="importing lxml turns the GIL back on for the whole run")
@PROFILE
@given(cases(broken=True, wild=True))
def test_validate_agrees_with_lxml(case):
    from lxml import etree

    ours = theirs = None
    why = ""
    try:
        ours = xsdkit.SchemaSet.from_string(case.xsd)
    except xsdkit.SchemaError as e:
        why = f"xsdkit: {e}"
    try:
        theirs = etree.XMLSchema(etree.fromstring(case.xsd.encode()))
    except etree.XMLSchemaParseError as e:
        why = f"lxml: {e}"
    assert (ours is None) == (theirs is None), f"only one of them compiles the schema; {why}"
    if ours is None or theirs is None:
        return
    report = ours.validate(case.doc)
    valid = theirs.validate(etree.fromstring(case.doc.encode()))
    assert report.is_valid == valid, "\n".join(
        [
            f"xsdkit says {'valid' if report.is_valid else 'invalid'}, lxml says {'valid' if valid else 'invalid'}",
            *(f"  xsdkit: {d.code} {d.message}" for d in report.diagnostics),
            *(f"  lxml: {e.message}" for e in theirs.error_log),
        ]
    )


@PROFILE
@given(cases(broken=False, wild=False))
def test_decode_agrees_with_xmlschema(case):
    try:
        ours = xsdkit.SchemaSet.from_string(case.xsd)
    except xsdkit.SchemaError:
        assume(False)
    assume(ours.validate(case.doc).is_valid)
    decoded = ours.decode(case.doc)
    shape = misshapen(decoded, case.repeating)
    assert shape is None, f"{shape}\n  xsdkit: {decoded!r}"
    try:
        expected = xmlschema.XMLSchema10(case.xsd).to_dict(case.doc, strip_namespaces=True)
    except xmlschema.XMLSchemaValidationError:
        # xmlschema rejects some valid counted content; whether the document
        # is valid is for the test above to settle, against lxml.
        assume(False)
    why = differs(decoded, expected)
    assert why is None, f"{why}\n  xsdkit:    {decoded!r}\n  xmlschema: {expected!r}"

"""The Python surface, exercised the way a user would."""

import pytest
import xsdkit
from conftest import NS, XS, build


# --- loading ---------------------------------------------------------------


def test_from_string_builds():
    s = build('<xs:element name="a" type="xs:string"/>')
    assert s.element(NS, "a") is not None
    assert "SchemaSet" in repr(s)


def test_errors_carry_every_diagnostic():
    with pytest.raises(xsdkit.SchemaError) as excinfo:
        build(
            '<xs:element name="a" type="tns:NopeOne"/>'
            '<xs:element name="b" type="tns:NopeTwo"/>'
        )
    diags = excinfo.value.diagnostics
    unresolved = [d for d in diags if d.code == "XSD1201"]
    assert len(unresolved) == 2, "schema authors need the whole list"
    assert unresolved[0].severity == "error"
    assert unresolved[0].spans
    assert unresolved[0].help


def test_load_returns_diagnostics_instead_of_raising():
    s, diags = xsdkit.load_string(
        '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">'
        '<xs:include schemaLocation="missing.xsd"/></xs:schema>',
        conformance="lax",
    )
    assert s is not None
    assert any(d.code == "XSD1101" for d in diags)
    assert all(d.severity == "warning" for d in diags), "lax must not error"


def test_conformance_is_validated():
    with pytest.raises(ValueError, match="strict"):
        build('<xs:element name="a" type="xs:string"/>', conformance="sloppy")


def test_from_bytes_detects_encoding():
    xsd = (
        '<?xml version="1.0" encoding="ISO-8859-1"?>'
        '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" '
        f'targetNamespace="{NS}">'
        '<xs:element name="groesse" type="xs:double">'
        "<xs:annotation><xs:documentation>Größe</xs:documentation></xs:annotation>"
        "</xs:element></xs:schema>"
    )
    s = xsdkit.SchemaSet.from_bytes(xsd.encode("iso-8859-1"))
    assert s.element(NS, "groesse").doc == "Größe"


# --- name lookup -----------------------------------------------------------


def test_names_accept_three_spellings():
    s = build('<xs:element name="a" type="xs:string"/>')
    by_pair = s.element(NS, "a")
    by_clark = s.element(f"{{{NS}}}a")
    assert by_pair == by_clark
    assert by_pair.qname == f"{{{NS}}}a"
    assert by_pair.name == (NS, "a")
    assert by_pair.local_name == "a"
    assert by_pair.namespace == NS


def test_unknown_names_are_none_not_errors():
    s = build('<xs:element name="a" type="xs:string"/>')
    assert s.element(NS, "nope") is None
    assert s.type(NS, "nope") is None
    assert s.attribute(NS, "nope") is None


def test_malformed_clark_notation_is_rejected():
    s = build('<xs:element name="a" type="xs:string"/>')
    with pytest.raises(ValueError, match="unterminated"):
        s.element("{urn:example")


# --- content models --------------------------------------------------------


def test_children_repeat_and_optionality():
    s = build(
        '<xs:complexType name="T"><xs:sequence>'
        '<xs:element name="required" type="xs:string"/>'
        '<xs:element name="many" type="xs:string" maxOccurs="unbounded"/>'
        '<xs:element name="maybe" type="xs:string" minOccurs="0"/>'
        "</xs:sequence></xs:complexType>"
    )
    t = s.type(NS, "T")
    kids = {c.local_name: c for c in t.children}
    assert set(kids) == {"required", "many", "maybe"}

    assert not t.repeats(kids["required"]) and not t.optional(kids["required"])
    assert t.repeats(kids["many"])
    assert t.optional(kids["maybe"]) and not t.repeats(kids["maybe"])
    assert t.content == "element-only"
    assert t.content_model == "automaton"


def test_accepts_matches_a_child_sequence():
    s = build(
        '<xs:complexType name="T"><xs:sequence>'
        '<xs:element name="a" type="xs:string"/>'
        '<xs:element name="b" type="xs:string" maxOccurs="unbounded"/>'
        "</xs:sequence></xs:complexType>"
    )
    t = s.type(NS, "T")
    q = lambda n: f"{{{NS}}}{n}"
    assert t.accepts([q("a"), q("b")])
    assert t.accepts([q("a"), q("b"), q("b")])
    assert not t.accepts([q("a")])
    assert not t.accepts([q("b"), q("a")])
    assert not t.accepts([(NS, "nonexistent")])


def test_xs_all_is_order_independent():
    s = build(
        '<xs:complexType name="T"><xs:all>'
        '<xs:element name="a" type="xs:string"/>'
        '<xs:element name="b" type="xs:string"/>'
        "</xs:all></xs:complexType>"
    )
    t = s.type(NS, "T")
    assert t.content_model == "all"
    assert t.accepts([(NS, "b"), (NS, "a")])
    assert not t.accepts([(NS, "a")])


def test_extension_inherits_base_content():
    s = build(
        '<xs:complexType name="Base"><xs:sequence>'
        '<xs:element name="a" type="xs:string"/></xs:sequence></xs:complexType>'
        '<xs:complexType name="T"><xs:complexContent>'
        '<xs:extension base="tns:Base"><xs:sequence>'
        '<xs:element name="b" type="xs:string"/></xs:sequence></xs:extension>'
        "</xs:complexContent></xs:complexType>"
    )
    t = s.type(NS, "T")
    assert [c.local_name for c in t.children] == ["a", "b"]
    assert t.derivation == "extension"
    assert t.base.qname == f"{{{NS}}}Base"
    assert t.derives_from(s.type(NS, "Base"))


# --- substitution groups ---------------------------------------------------


def test_substitutes_are_transitive_and_skip_abstract_heads():
    s = build(
        '<xs:element name="feature" type="xs:string" abstract="true"/>'
        '<xs:element name="point" type="xs:string" substitutionGroup="tns:feature"/>'
        '<xs:element name="curve" type="xs:string" substitutionGroup="tns:feature"/>'
        '<xs:element name="arc" type="xs:string" substitutionGroup="tns:curve"/>'
    )
    head = s.element(NS, "feature")
    assert head.abstract
    names = sorted(e.local_name for e in head.substitutes)
    assert names == ["arc", "curve", "point"]
    assert sorted(e.local_name for e in s.element(NS, "curve").substitutes) == ["arc", "curve"]


# --- simple types ----------------------------------------------------------


def test_facets_expose_the_and_or_pattern_structure():
    s = build(
        '<xs:simpleType name="Code"><xs:restriction base="xs:string">'
        '<xs:pattern value="[A-Z]+"/><xs:pattern value="[0-9]+"/>'
        '<xs:maxLength value="4"/></xs:restriction></xs:simpleType>'
    )
    t = s.type(NS, "Code")
    assert t.is_simple and t.variety == "atomic"
    assert t.primitive == "string"
    f = t.facets
    assert f.max_length == 4
    # One step, two alternatives — ORed with each other.
    assert f.patterns == [["[A-Z]+", "[0-9]+"]]


def test_list_and_union_varieties():
    s = build(
        '<xs:simpleType name="Ints"><xs:list itemType="xs:int"/></xs:simpleType>'
        '<xs:simpleType name="Either">'
        '<xs:union memberTypes="xs:int xs:string"/></xs:simpleType>'
    )
    ints = s.type(NS, "Ints")
    assert ints.variety == "list"
    assert ints.item_type.builtin == "int"

    either = s.type(NS, "Either")
    assert either.variety == "union"
    # Order is load-bearing: members are tried in declaration order.
    assert [m.builtin for m in either.member_types] == ["int", "string"]


# --- attributes and annotations -------------------------------------------


def test_attribute_uses_carry_use_and_fixed():
    s = build(
        '<xs:complexType name="Measure"><xs:simpleContent>'
        '<xs:extension base="xs:double">'
        '<xs:attribute name="uom" type="xs:string" use="required" fixed="m"/>'
        '<xs:attribute name="note" type="xs:string"/>'
        "</xs:extension></xs:simpleContent></xs:complexType>"
    )
    uses = {a.local_name: a for a in s.type(NS, "Measure").attributes}
    uom = uses["uom"]
    assert uom.required and uom.use == "required"
    # A schema-declared constant unit — resolvable without an instance.
    assert uom.fixed == "m"
    assert uses["note"].use == "optional"
    assert uses["note"].fixed is None


def test_appinfo_is_verbatim():
    s = build(
        '<xs:element name="pressure" type="xs:double"><xs:annotation>'
        "<xs:documentation>Ambient pressure.</xs:documentation>"
        '<xs:appinfo source="urn:units">'
        '<u:unit xmlns:u="urn:u">hPa</u:unit></xs:appinfo>'
        "</xs:annotation></xs:element>"
    )
    e = s.element(NS, "pressure")
    assert e.doc == "Ambient pressure."
    (info,) = e.appinfo
    assert info.source == "urn:units"
    assert "hPa" in info.xml
    assert "{urn:u}unit" in info.xml, "prefixes are resolved so none can be lost"


# --- the real schema -------------------------------------------------------


def test_the_schema_for_schemas_is_queryable(schema_for_schemas):
    s = schema_for_schemas
    assert len(s.documents) == 1
    assert s.documents[0].target_namespace == XS

    keybase = s.type(XS, "keybase")
    kids = {c.local_name: c for c in keybase.children}
    assert not keybase.optional(kids["selector"])
    assert keybase.repeats(kids["field"])
    assert keybase.accepts([f"{{{XS}}}selector", f"{{{XS}}}field"])
    assert not keybase.accepts([f"{{{XS}}}field"])


def test_globals_are_listed_and_sorted(schema_for_schemas):
    names = [n for n, _ in schema_for_schemas.elements]
    assert names == sorted(names)
    assert f"{{{XS}}}schema" in names
    assert dict(schema_for_schemas.counts)["types"] > 100

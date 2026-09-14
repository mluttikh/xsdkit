"""The Python surface, exercised the way a user would."""

import collections.abc
import copy
import datetime
import math
import pickle
import weakref
from decimal import Decimal

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

    # A child carries its own occurrence: it is a fact about this pair, and
    # asking the parent about it separately walked the content model again.
    assert not kids["required"].repeats and not kids["required"].optional
    assert kids["many"].repeats
    assert kids["maybe"].optional and not kids["maybe"].repeats

    # And everything the declaration answers, it answers too.
    assert kids["many"].type.qname == f"{{{XS}}}string"
    assert kids["many"].qname == f"{{{NS}}}many"
    assert kids["many"].element == s.type(NS, "T").children[1].element
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
    assert not kids["selector"].optional
    assert kids["field"].repeats
    assert keybase.accepts([f"{{{XS}}}selector", f"{{{XS}}}field"])
    assert not keybase.accepts([f"{{{XS}}}field"])


def test_globals_are_listed_and_sorted(schema_for_schemas):
    names = [e.qname for e in schema_for_schemas.elements]
    assert names == sorted(names)
    assert f"{{{XS}}}schema" in names
    assert schema_for_schemas.counts["types"] > 100


# --- ergonomics -------------------------------------------------------------


def test_xsd_11_is_reachable():
    """Everything the 1.1 reader adds was Rust-only until `version=` existed."""
    xsd = (
        f'<xs:schema xmlns:xs="{XS}" xmlns:tns="{NS}" targetNamespace="{NS}">'
        '<xs:element name="e" type="xs:precisionDecimal"/>'
        "</xs:schema>"
    )
    s = xsdkit.SchemaSet.from_string(xsd, version="1.1")
    assert s.element(NS, "e").type.qname == f"{{{XS}}}precisionDecimal"

    with pytest.raises(ValueError, match="1.0"):
        xsdkit.SchemaSet.from_string(xsd, version="1.2")


def test_schema_set_is_a_mapping_of_global_elements():
    s = build('<xs:element name="a" type="xs:string"/>'
              '<xs:simpleType name="T"><xs:restriction base="xs:string"/></xs:simpleType>')
    assert len(s) == 1
    assert list(s) == [f"{{{NS}}}a"]
    assert f"{{{NS}}}a" in s and f"{{{NS}}}T" not in s
    assert 42 not in s, "not a name, so not there — as for a dict"
    assert s[f"{{{NS}}}a"] == s.element(NS, "a")
    assert s.get(f"{{{NS}}}nope") is None

    # A type has a view of its own, and the error says where to look.
    with pytest.raises(KeyError, match="schemas.types"):
        s[(NS, "T")]
    assert s.types[(NS, "T")] == s.type(NS, "T")

    # A built-in is not one of this schema's declarations, but is still
    # resolvable by name.
    assert f"{{{XS}}}string" not in s.types
    assert s.type(XS, "string") is not None

    with pytest.raises(KeyError):
        s[f"{{{NS}}}nope"]


def test_counts_is_a_dict():
    s = build('<xs:element name="a" type="xs:string"/>')
    assert isinstance(s.counts, dict)
    assert s.counts["elements"] >= 1


def test_handles_compare_by_identity_of_the_component():
    """Two handles to one declaration are one declaration.

    Without `__eq__`/`__hash__` these fall back to object identity, so a set of
    them silently holds duplicates and nothing raises.
    """
    s = build(
        '<xs:element name="e"><xs:complexType>'
        '<xs:attribute name="k" type="xs:string"/></xs:complexType></xs:element>'
    )
    t = s.element(NS, "e").type
    a1, a2 = t.attributes[0].attribute, t.attributes[0].attribute
    assert a1 == a2 and len({a1, a2}) == 1
    assert t.attributes[0] == t.attributes[0]
    assert s.documents[0] == s.documents[0]
    assert len(set(s.documents)) == 1


def test_from_file_takes_a_path(tmp_path):
    p = tmp_path / "s.xsd"
    p.write_text(f'<xs:schema xmlns:xs="{XS}" xmlns:tns="{NS}" targetNamespace="{NS}">'
                 '<xs:element name="a" type="xs:string"/></xs:schema>')
    assert xsdkit.SchemaSet.from_file(p).element(NS, "a") is not None
    assert xsdkit.load(p)[0].element(NS, "a") is not None


def test_a_resolver_can_serve_documents_from_anywhere():
    """Schemas in a zip, a database or an HTTP cache were unreachable."""
    main = (
        f'<xs:schema xmlns:xs="{XS}" xmlns:tns="{NS}" targetNamespace="{NS}">'
        '<xs:include schemaLocation="part.xsd"/>'
        '<xs:element name="root" type="tns:T"/></xs:schema>'
    )
    part = (
        f'<xs:schema xmlns:xs="{XS}" targetNamespace="{NS}">'
        '<xs:simpleType name="T"><xs:restriction base="xs:int"/></xs:simpleType>'
        "</xs:schema>"
    )
    asked = []

    def resolve(location, base):
        asked.append((location, base))
        return part.encode()  # bytes: the encoding is xsdkit's problem

    s = xsdkit.SchemaSet.from_string(main, resolver=resolve)
    assert asked and asked[0][0] == "part.xsd"
    assert s.type(NS, "T") is not None

    # `(uri, document)` says where it was really found.
    s = xsdkit.SchemaSet.from_string(
        main, resolver=lambda loc, base: (f"zip://{loc}", part)
    )
    assert any(d.uri.startswith("zip://") for d in s.documents)

    # Raising is how a resolver says no, and the exception becomes the message.
    def missing(location, base):
        raise FileNotFoundError("not in the archive")

    with pytest.raises(xsdkit.SchemaError) as excinfo:
        xsdkit.SchemaSet.from_string(main, resolver=missing)
    assert "not in the archive" in excinfo.value.diagnostics[0].message


def test_documents_may_be_bytes():
    s = build('<xs:element name="a" type="xs:int"/>')
    doc = f'<a xmlns="{NS}">1</a>'
    assert s.validate(doc.encode()).is_valid
    assert [e.kind for e in s.iter_typed(doc.encode())] == ["start", "text", "end"]
    # And the encoding is detected rather than assumed.
    latin = f'<?xml version="1.0" encoding="ISO-8859-1"?><a xmlns="{NS}">1</a>'
    assert s.validate(latin.encode("iso-8859-1")).is_valid
    with pytest.raises(TypeError, match="str, bytes or a path, not int"):
        s.validate(42)


def test_facets_are_the_ones_in_force():
    """A restriction inherits what its base constrained.

    Reporting only the declared set disagrees with `validate`, which composes
    the chain.
    """
    s = build(
        '<xs:simpleType name="A"><xs:restriction base="xs:string">'
        '<xs:minLength value="2"/></xs:restriction></xs:simpleType>'
        '<xs:simpleType name="B"><xs:restriction base="tns:A">'
        '<xs:maxLength value="8"/></xs:restriction></xs:simpleType>'
    )
    b = s.type(NS, "B")
    assert (b.facets.min_length, b.facets.max_length) == (2, 8)
    assert not b.is_valid("a") and b.is_valid("abc")

    # What this step wrote, for a tool rendering the schema back out.
    assert b.declared_facets.min_length is None
    assert b.declared_facets.max_length == 8


def test_browsing_needs_no_index_arithmetic():
    """The shape a reader actually walks a schema in.

    `elements` used to hand back `(name, element)` pairs and every child hop
    went through `.type`, so reaching a grandchild read
    `x.elements[0][1].type.children[1].type.children[0]`.
    """
    s = build(
        '<xs:element name="report"><xs:complexType><xs:sequence>'
        '<xs:element name="title" type="xs:string"/>'
        '<xs:element name="item" maxOccurs="unbounded"><xs:complexType><xs:sequence>'
        '<xs:element name="price" type="xs:decimal"/>'
        '<xs:element name="note" type="xs:string" minOccurs="0"/>'
        '</xs:sequence><xs:attribute name="sku" type="xs:string" use="required"/>'
        "</xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>"
    )
    report = s.elements[0]
    assert report.local_name == "report"
    assert report.children[1].children[0].qname == f"{{{NS}}}price"
    # Or by name, which is what browsing usually means.
    assert s[f"{{{NS}}}report"]["item"]["price"].qname == f"{{{NS}}}price"
    assert report["item"]["note"].local_name == "note"

    # An element is its children: iterable, sized, subscriptable.
    assert [c.local_name for c in report] == ["title", "item"]
    assert len(report) == 2
    assert report.attributes == report.type.attributes
    with pytest.raises(KeyError):
        report["nope"]

    # Occurrence is a fact about the pair, and a child knows its own.
    assert report["item"].repeats
    assert report["item"]["note"].optional
    # The same declaration under a different parent may say something else,
    # which is why the flags live on the child and not on the element.
    assert report["item"].element != report["item"]

    # `types` yields types, not pairs.
    assert all(hasattr(t, "is_simple") for t in s.types)


def test_tree_renders_a_schema_for_reading():
    s = build(
        '<xs:element name="report"><xs:complexType><xs:sequence>'
        '<xs:element name="item" maxOccurs="unbounded"><xs:complexType><xs:sequence>'
        '<xs:element name="note" type="xs:string" minOccurs="0"/>'
        '</xs:sequence><xs:attribute name="sku" type="xs:string" use="required"/>'
        "</xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>"
    )
    lines = s.elements[0].tree().splitlines()
    assert lines[0] == "report"
    assert "item+" in lines[1], "one or more"
    assert "@sku" in lines[2], "required attributes carry no marker"
    assert "note?: xs:string" in lines[3], "optional, and the built-in is abbreviated"


def test_a_recursive_schema_prints_once():
    s = build(
        '<xs:complexType name="Node"><xs:sequence>'
        '<xs:element name="child" type="tns:Node" minOccurs="0"/>'
        "</xs:sequence></xs:complexType>"
        '<xs:element name="root" type="tns:Node"/>'
    )
    out = s[f"{{{NS}}}root"].tree(depth=50)
    assert out.count("child") < 10, "recursion has to stop where the shape repeats"
    assert "..." in out


def test_a_tree_shows_itself_rather_than_escaping_itself():
    """A notebook displays `repr()` of the last expression.

    `repr` of a `str` escapes every newline into `\\n`, so returning one made
    `element.tree()` unreadable in the place people most want to read it.
    """
    s = build(
        '<xs:element name="report"><xs:complexType><xs:sequence>'
        '<xs:element name="item" type="xs:string" maxOccurs="unbounded"/>'
        "</xs:sequence></xs:complexType></xs:element>"
    )
    t = s.elements[0].tree()
    assert repr(t) == str(t), "repr is the tree, not an escaped one-liner"
    assert "\\n" not in repr(t)
    assert repr(t).splitlines()[0] == "report"

    # Jupyter picks the HTML up, structured and monospaced.
    html = t._repr_html_()
    assert "item" in html and "jp-code-font-family" in html

    # And it still behaves as the text it is.
    assert "item+" in t
    assert t.splitlines()[0] == "report"
    assert len(t) == len(str(t))
    assert t == str(t)


def test_markup_in_a_namespace_is_escaped():
    """A namespace URI may hold an ampersand, and the HTML must survive it."""
    s = xsdkit.SchemaSet.from_string(
        f'<xs:schema xmlns:xs="{XS}" xmlns:tns="urn:a&amp;b" targetNamespace="urn:a&amp;b">'
        '<xs:element name="e" type="tns:T"/>'
        '<xs:complexType name="T"><xs:sequence/></xs:complexType></xs:schema>'
    )
    tree = s.elements[0].tree()
    assert "urn:a&b" in tree, "the text keeps the URI as it is"
    assert "urn:a&amp;b" in tree._repr_html_(), "the HTML escapes it"


def test_evaluating_a_component_shows_it():
    """The notebook gesture is to evaluate, not to print."""
    s = build(
        '<xs:element name="report" type="tns:R"/>'
        '<xs:complexType name="R"><xs:sequence>'
        '<xs:element name="item" type="xs:string"/>'
        "</xs:sequence></xs:complexType>"
    )
    for component in (s.elements[0], s.types[0]):
        html = component._repr_html_()
        assert "item" in html and html.startswith("<div")
    # `repr` stays short, so a list of them is still readable.
    assert repr(s.elements) == "[<Element {urn:example}report>]"


# --- notebook rendering -----------------------------------------------------


def _html(obj):
    """What IPython would actually put in a cell's output."""
    from IPython.core.formatters import DisplayFormatter

    data, _ = DisplayFormatter().format(obj)
    return data.get("text/html", "")


@pytest.fixture
def rich():
    return build(
        '<xs:element name="report"><xs:complexType><xs:sequence>'
        '<xs:element name="item" maxOccurs="unbounded"><xs:complexType><xs:sequence>'
        '<xs:element name="amount" type="tns:Money"/>'
        '<xs:element name="note" type="xs:string" minOccurs="0"/>'
        '</xs:sequence><xs:attribute name="sku" type="xs:string" use="required"/>'
        "</xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>"
        '<xs:simpleType name="Money"><xs:restriction base="xs:decimal">'
        '<xs:fractionDigits value="2"/><xs:minInclusive value="0"/>'
        "</xs:restriction></xs:simpleType>"
    )


def test_every_rendering_reaches_ipython_and_is_well_formed(rich):
    report = rich.validate(f'<report xmlns="{NS}"><item><amount>x</amount></item></report>')
    subjects = {
        "SchemaSet": rich,
        "Element": rich.elements[0],
        "Type": rich.types[0],
        "Facets": rich.type(NS, "Money").facets,
        "ValidationReport": report,
        "Diagnostic": report.diagnostics[0],
        "Tree": rich.elements[0].tree(),
    }
    for label, obj in subjects.items():
        html = _html(obj)
        assert html, f"{label} gives IPython no text/html"
        for open_tag, close in (("<div", "</div>"), ("<table", "</table>"),
                                ("<tr>", "</tr>"), ("<details", "</details>")):
            assert html.count(open_tag) == html.count(close), f"{label}: {open_tag} unbalanced"


def test_renderings_adapt_to_the_theme(rich):
    """Hard-coded colours are unreadable in half of all notebooks.

    Every colour goes through a `--jp-*` variable, which JupyterLab redefines
    per theme, with a literal fallback for the classic Notebook and VS Code,
    which define none of them.
    """
    for obj in (rich, rich.elements[0], rich.type(NS, "Money").facets):
        html = _html(obj)
        colours = [c for c in html.split("color:")[1:]]
        assert colours, "nothing coloured at all"
        for c in colours:
            assert c.startswith("var(--jp-"), f"hard-coded colour: {c[:40]}"
            assert "," in c.split(")")[0], "no fallback for a non-Jupyter host"


def test_a_tree_is_collapsible(rich):
    """`<details>` gives a big schema a way to be explored rather than dumped."""
    html = rich.elements[0].tree()._repr_html_()
    assert "<details" in html and "<summary" in html
    # Open near the root, closed deeper, so a large schema does not arrive
    # fully expanded.
    assert "<details open>" in html


def test_a_long_report_is_cut_off(rich):
    many = "".join(f"<item><amount>x{i}</amount></item>" for i in range(60))
    report = rich.validate(f'<report xmlns="{NS}">{many}</report>')
    html = _html(report)
    assert len(report.diagnostics) > 40
    assert "and" in html and "more" in html, "a long list has to say it was cut"
    assert html.count("<tr>") <= 41


def test_markup_from_a_schema_cannot_escape_into_the_page(rich):
    s = xsdkit.SchemaSet.from_string(
        f'<xs:schema xmlns:xs="{XS}" xmlns:tns="urn:a&amp;b" targetNamespace="urn:a&amp;b">'
        '<xs:element name="e" type="tns:T"/>'
        '<xs:complexType name="T"><xs:sequence/></xs:complexType></xs:schema>'
    )
    for obj in (s, s.elements[0], s.elements[0].tree()):
        html = _html(obj)
        assert "urn:a&amp;b" in html
        assert "urn:a&b<" not in html


def test_a_child_is_an_element_that_knows_where_it_is():
    s = build(
        '<xs:complexType name="T"><xs:sequence>'
        '<xs:element name="one" type="xs:string" maxOccurs="unbounded"/>'
        "</xs:sequence></xs:complexType>"
        '<xs:element name="e" type="tns:T"/>'
    )
    e = s.element(NS, "e")
    (child,) = e.children

    # Everything the declaration answers.
    assert child.local_name == "one"
    assert child.qname == f"{{{NS}}}one"
    assert child.namespace == NS
    assert child.type.qname == f"{{{XS}}}string"
    assert child.is_global is False
    assert child.nillable is False and child.abstract is False
    assert child.default is None and child.fixed is None
    assert child.children == [] and child.attributes == []
    assert len(child) == 0
    assert list(child) == []

    # Plus where it is.
    assert child.repeats and not child.optional
    assert "one+" in repr(child)

    # Iteration, subscripting and `children` all agree.
    assert list(e) == e.children == [child]
    assert e["one"] == child
    assert child == e.children[0]

    # The declaration underneath is reachable, and is deliberately a
    # different thing: it has no parent to have occurrence in.
    assert child.element.local_name == "one"
    assert child.element != child


def test_a_substitution_group_member_is_optional():
    """Its sibling may stand in its place, so no content requires it.

    A single position in the content model admits every member of the group,
    and treating that position as the element made each member look required
    even though a document naming only the other one validates.
    """
    s = build(
        '<xs:element name="shape" type="xs:string" abstract="true"/>'
        '<xs:element name="circle" type="xs:string" substitutionGroup="tns:shape"/>'
        '<xs:element name="square" type="xs:string" substitutionGroup="tns:shape"/>'
        '<xs:complexType name="T"><xs:sequence>'
        '<xs:element ref="tns:shape"/>'
        "</xs:sequence></xs:complexType>"
        '<xs:element name="e" type="tns:T"/>'
    )
    e = s.element(NS, "e")
    kids = {c.local_name: c for c in e.children}
    assert set(kids) == {"circle", "square"}, "the abstract head cannot appear"
    assert all(c.optional for c in kids.values())

    # And the validator agrees, which is the point.
    assert s.validate(f'<e xmlns="{NS}"><circle>o</circle></e>').is_valid


def test_block_excludes_a_substitute_that_is_still_in_the_group():
    """`substitutes` says what may appear, not who is in the group.

    Both are one call away from each other and both return elements, so the
    only guard against reaching for the wrong one is that they disagree
    visibly — and that this one agrees with the validator.
    """
    s = build(
        '<xs:element name="shape" type="xs:string" block="substitution"/>'
        '<xs:element name="circle" type="xs:string" substitutionGroup="tns:shape"/>'
        '<xs:complexType name="Holder">'
        '<xs:sequence><xs:element ref="tns:shape"/></xs:sequence>'
        "</xs:complexType>"
        '<xs:element name="holder" type="tns:Holder"/>'
    )
    shape = s.element(NS, "shape")
    assert [e.local_name for e in shape.substitutes] == ["shape"]

    holder = s.element(NS, "holder")
    assert [c.local_name for c in holder.children] == ["shape"]
    assert not s.validate(
        f'<holder xmlns="{NS}"><circle>o</circle></holder>'
    ).is_valid


def test_a_schema_set_is_a_mapping_in_full():
    """`__len__`, `__contains__`, `__getitem__` and `__iter__` make something
    that *looks* like a mapping; `dict()` needs `keys` as well."""
    s = build(
        '<xs:element name="a" type="xs:string"/>'
        '<xs:complexType name="T"><xs:sequence/></xs:complexType>'
    )
    assert s.keys() == list(s)
    assert [k for k, _ in s.items()] == s.keys()
    assert [type(v).__name__ for v in s.values()] == ["Element"]
    assert isinstance(s, collections.abc.Mapping)

    d = dict(s)
    assert len(d) == len(s)
    assert d[f"{{{NS}}}a"] == s[f"{{{NS}}}a"]


def test_derives_from_stays_inside_one_schema_set():
    """A handle is an index into one set's arenas.

    Two unrelated schemas whose indices happen to line up used to report a
    derivation that does not exist.
    """
    a = build(
        '<xs:complexType name="B"><xs:sequence/></xs:complexType>'
        '<xs:complexType name="D"><xs:complexContent>'
        '<xs:extension base="tns:B"/></xs:complexContent></xs:complexType>'
    )
    c = build(
        '<xs:complexType name="X"><xs:sequence/></xs:complexType>'
        '<xs:complexType name="Y"><xs:sequence/></xs:complexType>'
    )
    assert a.type(NS, "D").derives_from(a.type(NS, "B"))
    assert not a.type(NS, "D").derives_from(c.type(NS, "X"))


def test_the_default_diagnostics_cannot_be_polluted():
    """The class-level default is shared by every error that sets none.

    It was a list, so one `append` reached every later `XsdError`.
    """
    errors = (xsdkit.XsdError, xsdkit.SchemaError, xsdkit.DocumentError, xsdkit.InvalidValueError)
    for cls in errors:
        err = cls("raised by hand")
        assert list(err.diagnostics) == []
        with pytest.raises(AttributeError):
            err.diagnostics.append("polluted")
        assert list(cls("the next one").diagnostics) == []


TWO_INCLUDES = (
    f'<xs:schema xmlns:xs="{XS}" targetNamespace="{NS}">'
    '<xs:include schemaLocation="one.xsd"/>'
    '<xs:include schemaLocation="two.xsd"/>'
    "</xs:schema>"
)


@pytest.mark.parametrize("interrupt", [KeyboardInterrupt, SystemExit])
def test_interrupting_a_resolver_stops_the_build(interrupt):
    """Ctrl-C in a slow resolver became a diagnostic, and the build went on."""
    asked = []

    def resolve(location, base):
        asked.append(location)
        raise interrupt

    with pytest.raises(interrupt):
        xsdkit.SchemaSet.from_string(TWO_INCLUDES, resolver=resolve)
    assert asked == ["one.xsd"], "nothing more is asked once the build is interrupted"

    # The functions that hand diagnostics back instead of raising do not
    # swallow it either.
    with pytest.raises(interrupt):
        xsdkit.load_string(TWO_INCLUDES, resolver=resolve)


def test_a_resolvers_exception_is_the_schema_errors_cause():
    """Flattened into a message, it lost its type and its traceback."""

    def missing(location, base):
        raise FileNotFoundError(f"{location} is not in the archive")

    with pytest.raises(xsdkit.SchemaError) as excinfo:
        xsdkit.SchemaSet.from_string(TWO_INCLUDES, resolver=missing)
    cause = excinfo.value.__cause__
    assert isinstance(cause, FileNotFoundError)
    assert "one.xsd" in str(cause), "the first exception raised is the cause"


def test_a_schema_nested_too_deeply_is_refused_not_parsed():
    """Parsing recurses once per level, so a deep enough schema overflowed the
    native stack and killed the interpreter.

    Run in a subprocess, so that a regression fails this test rather than
    ending the whole run.
    """
    import subprocess
    import sys
    import textwrap

    code = textwrap.dedent(
        '''
        import xsdkit

        def nested(levels):
            return (
                '<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">'
                + '<xs:element name="e"><xs:complexType><xs:sequence>' * levels
                + '</xs:sequence></xs:complexType></xs:element>' * levels
                + '</xs:schema>'
            )

        try:
            xsdkit.SchemaSet.from_string(nested(3000))
        except xsdkit.SchemaError as e:
            print(sorted({d.code for d in e.diagnostics}))
        # The limit is a keyword, for a trusted schema that needs more.
        print(len(xsdkit.SchemaSet.from_string(nested(100), max_depth=400)))
        '''
    )
    result = subprocess.run(
        [sys.executable, "-c", code], capture_output=True, text=True, timeout=120
    )
    assert result.returncode == 0, result.stderr[-2000:]
    assert result.stdout.split() == ["['XSD1001']", "1"]


def test_an_element_and_a_type_may_share_a_name():
    """Elements and types are separate symbol spaces, and
    `<xs:element name="Address" type="tns:Address"/>` is everywhere."""
    s = build(
        '<xs:element name="Address" type="tns:Address"/>'
        '<xs:element name="Zed" type="xs:string"/>'
        '<xs:complexType name="Aaa"><xs:sequence/></xs:complexType>'
        '<xs:complexType name="Address"><xs:sequence>'
        '<xs:element name="street" type="xs:string"/>'
        "</xs:sequence></xs:complexType>"
    )
    address = f"{{{NS}}}Address"
    assert list(s) == [address, f"{{{NS}}}Zed"]
    assert len(dict(s)) == len(s) == 2
    assert isinstance(s[address], xsdkit.Element)
    assert isinstance(s.types[address], xsdkit.Type)
    assert s[address].type == s.types[address]
    assert s.types.keys() == [f"{{{NS}}}Aaa", address]


def test_the_views_work_by_position_and_by_name():
    s = build(
        '<xs:element name="b" type="xs:string"/>'
        '<xs:element name="a" type="xs:string"/>'
        '<xs:attribute name="lang" type="xs:language"/>'
        '<xs:simpleType name="T"><xs:restriction base="xs:string"/></xs:simpleType>'
    )
    a, b = s.element(NS, "a"), s.element(NS, "b")

    # As a list: the components, in name order.
    assert list(s.elements) == [a, b]
    assert len(s.elements) == 2
    assert s.elements[0] == a and s.elements[-1] == b
    with pytest.raises(IndexError):
        s.elements[2]
    assert repr(s.elements) == f"[<Element {{{NS}}}a>, <Element {{{NS}}}b>]"

    # As a mapping: by name, in either spelling.
    assert s.elements[f"{{{NS}}}b"] == s.elements[(NS, "b")] == b
    assert f"{{{NS}}}a" in s.elements and a in s.elements
    assert s.elements.get(f"{{{NS}}}nope") is None
    assert s.elements.keys() == [k for k, _ in s.elements.items()]
    assert s.elements.values() == [a, b]
    with pytest.raises(KeyError, match="an element"):
        s.types[f"{{{NS}}}a"]

    # Attributes this schema declares, without the xml: and xsi: ones.
    assert s.attributes.keys() == [f"{{{NS}}}lang"]
    assert s.attributes[(NS, "lang")] == s.attribute(NS, "lang")
    assert s.attribute("http://www.w3.org/XML/1998/namespace", "lang") is not None


def test_accepts_resolves_names_the_way_subscripting_does():
    s = build(
        '<xs:element name="r"><xs:complexType><xs:sequence>'
        '<xs:element name="x" type="xs:int"/><xs:element name="y" type="xs:int"/>'
        "</xs:sequence></xs:complexType></xs:element>"
    )
    t = s[f"{{{NS}}}r"].type
    assert t["x"].local_name == "x"
    assert t.accepts(["x", "y"]), "local names, as `type[name]` takes them"
    assert t.accepts([f"{{{NS}}}x", (NS, "y")])
    assert not t.accepts(["y", "x"])
    with pytest.raises(TypeError, match="not a single string"):
        t.accepts(f"{{{NS}}}x")


def test_small_protocols_behave_like_their_python_counterparts():
    s = build(
        '<xs:element name="r"><xs:complexType><xs:sequence>'
        '<xs:element name="x" type="xs:int"/><xs:element name="y" type="xs:int"/>'
        "</xs:sequence></xs:complexType></xs:element>"
        '<xs:simpleType name="B"><xs:restriction base="xs:int">'
        '<xs:minInclusive value="1"/><xs:maxInclusive value="9"/>'
        "</xs:restriction></xs:simpleType>"
    )
    r = s[f"{{{NS}}}r"]

    it = iter(r)
    next(it)
    assert len(it) == 1, "an iterator's length is what is left"

    assert repr(s) == "<SchemaSet 1 document, 1 element, 1 type>"
    assert repr(s.types[(NS, "B")].declared_facets) == "<Facets min_inclusive=1 max_inclusive=9>"

    tree = r.tree()
    assert hash(tree) == hash(str(tree)) and {str(tree): 1}[tree] == 1
    assert tree + "!" == str(tree) + "!" and "!" + tree == "!" + str(tree)

    doc = f'<r xmlns="{NS}"><x>no</x><y>1</y></r>'
    first, again = s.validate(doc).diagnostics[0], s.validate(doc).diagnostics[0]
    assert first == again and len({first, again}) == 1

    assert weakref.ref(s)() is s


def test_errors_form_one_hierarchy():
    assert issubclass(xsdkit.SchemaError, xsdkit.XsdError)
    assert issubclass(xsdkit.DocumentError, xsdkit.XsdError)
    assert issubclass(xsdkit.InvalidValueError, xsdkit.XsdError)
    assert issubclass(xsdkit.InvalidValueError, ValueError), "`except ValueError` still catches it"


def test_each_mistake_raises_what_python_would(tmp_path):
    s = build('<xs:element name="a" type="xs:int"/>')

    # A value its type does not admit.
    with pytest.raises(xsdkit.InvalidValueError):
        s.type(XS, "int").validate("nope")

    # A document that does not fit, asked for its data.
    with pytest.raises(xsdkit.DocumentError) as excinfo:
        s.decode(f'<a xmlns="{NS}">nope</a>')
    assert excinfo.value.diagnostics

    # An argument of the wrong type.
    with pytest.raises(TypeError, match="not int"):
        s.validate(42)
    with pytest.raises(TypeError, match="not int"):
        s.element(42)

    # A file that is not there: the error reading any file gives.
    missing = tmp_path / "missing.xml"
    with pytest.raises(FileNotFoundError) as excinfo:
        s.validate(missing)
    assert excinfo.value.filename == str(missing)


def test_bytes_that_cannot_be_decoded_are_an_invalid_document():
    """Not an exception: `validate` answers, and `decode` refuses."""
    s = build('<xs:element name="a" type="xs:int"/>')
    doc = b'<?xml version="1.0" encoding="no-such-encoding"?><a/>'

    report = s.validate(doc)
    assert not report.is_valid
    assert report.errors[0].code in {"XSD1006", "XSD1007"}
    assert not s.iter_typed(doc).report.is_valid
    events, report = s.read_typed(doc)
    assert events == [] and not report.is_valid
    with pytest.raises(xsdkit.DocumentError):
        s.decode(doc)
    with pytest.raises(xsdkit.DocumentError):
        s.decode(doc, lax=True)


def test_search_paths_take_path_objects_but_not_a_single_path(tmp_path):
    (tmp_path / "part.xsd").write_text(
        f'<xs:schema xmlns:xs="{XS}" targetNamespace="{NS}">'
        '<xs:simpleType name="T"><xs:restriction base="xs:int"/></xs:simpleType>'
        "</xs:schema>"
    )
    main = (
        f'<xs:schema xmlns:xs="{XS}" xmlns:tns="{NS}" targetNamespace="{NS}">'
        '<xs:include schemaLocation="part.xsd"/>'
        '<xs:element name="root" type="tns:T"/></xs:schema>'
    )
    assert f"{{{NS}}}root" in xsdkit.SchemaSet.from_string(main, search_paths=[tmp_path])
    # A string or a Path on its own is one path, not a list of them.
    for single in (str(tmp_path), tmp_path):
        with pytest.raises(TypeError, match="not a single path"):
            xsdkit.SchemaSet.from_string(main, search_paths=single)


def test_a_resolver_and_search_paths_are_alternatives(tmp_path):
    """A resolver replaces the filesystem, so the search paths would go unread."""
    with pytest.raises(ValueError, match="not both"):
        xsdkit.SchemaSet.from_string(
            TWO_INCLUDES, resolver=lambda location, base: b"", search_paths=[tmp_path]
        )


def test_several_root_documents_load_into_one_set(tmp_path):
    a, b = tmp_path / "a.xsd", tmp_path / "b.xsd"
    a.write_text(
        f'<xs:schema xmlns:xs="{XS}" targetNamespace="urn:a">'
        '<xs:element name="a" type="xs:string"/></xs:schema>'
    )
    b.write_text(
        f'<xs:schema xmlns:xs="{XS}" targetNamespace="urn:b">'
        '<xs:element name="b" type="xs:string"/></xs:schema>'
    )
    assert sorted(xsdkit.SchemaSet.from_files([a, b])) == ["{urn:a}a", "{urn:b}b"]

    schemas, diagnostics = xsdkit.load_files([str(a), b])
    assert len(schemas) == 2 and diagnostics == []

    with pytest.raises(TypeError, match="not a single path"):
        xsdkit.SchemaSet.from_files(str(a))
    with pytest.raises(ValueError, match="at least one"):
        xsdkit.SchemaSet.from_files([])


def test_load_bytes_detects_the_encoding():
    xsd = (
        '<?xml version="1.0" encoding="ISO-8859-1"?>'
        f'<xs:schema xmlns:xs="{XS}" targetNamespace="{NS}">'
        '<xs:element name="größe" type="xs:string"/></xs:schema>'
    )
    schemas, diagnostics = xsdkit.load_bytes(xsd.encode("iso-8859-1"))
    assert diagnostics == []
    assert f"{{{NS}}}größe" in schemas


# --- data out ----------------------------------------------------------------


def test_a_schema_set_pickles():
    """A process pool could not share a schema set: nothing pickled."""
    s = build('<xs:element name="a" type="xs:int"/>')
    back = pickle.loads(pickle.dumps(s))
    assert list(back) == list(s)
    assert back.validate(f'<a xmlns="{NS}">1</a>').is_valid
    assert not back.validate(f'<a xmlns="{NS}">x</a>').is_valid
    # It cannot change, so a copy is the same object.
    assert copy.copy(s) is s and copy.deepcopy(s) is s


def test_serialized_bytes_are_only_read_by_the_version_that_wrote_them():
    s = build('<xs:element name="a" type="xs:int"/>')
    data = s.serialize()
    assert list(xsdkit.SchemaSet.deserialize(data)) == list(s)

    other = data.replace(xsdkit.__version__.encode(), b"0.0.0", 1)
    with pytest.raises(ValueError, match="compile the schema again"):
        xsdkit.SchemaSet.deserialize(other)
    with pytest.raises(ValueError, match="not a schema set"):
        xsdkit.SchemaSet.deserialize(b"<xs:schema/>")
    with pytest.raises(ValueError, match="cannot read"):
        xsdkit.SchemaSet.deserialize(data[: len(data) // 2])


def test_errors_and_reports_survive_pickling():
    """A `SchemaError` raised in a worker came back as a failure to pickle it."""
    with pytest.raises(xsdkit.SchemaError) as excinfo:
        build('<xs:element name="a" type="tns:Nope"/>')
    err = pickle.loads(pickle.dumps(excinfo.value))
    assert type(err) is xsdkit.SchemaError
    assert str(err) == str(excinfo.value)
    assert err.diagnostics == excinfo.value.diagnostics
    assert err.diagnostics[0].spans == excinfo.value.diagnostics[0].spans

    s = build('<xs:element name="a" type="xs:int"/>')
    report = s.validate(f'<a xmlns="{NS}">x</a>')
    back = pickle.loads(pickle.dumps(report))
    assert (back.is_valid, back.diagnostics) == (report.is_valid, report.diagnostics)

    with pytest.raises(xsdkit.InvalidValueError) as excinfo:
        s.type(XS, "int").validate("x")
    assert type(pickle.loads(pickle.dumps(excinfo.value))) is xsdkit.InvalidValueError


def test_decode_can_say_which_root_it_read():
    """`<a>x</a>` and `<b>x</b>` both decoded to `'x'`."""
    s = build('<xs:element name="a" type="xs:string"/><xs:element name="b" type="xs:string"/>')
    assert s.decode(f'<a xmlns="{NS}">x</a>') == s.decode(f'<b xmlns="{NS}">x</b>') == "x"
    assert s.decode(f'<a xmlns="{NS}">x</a>', root=True) == {"a": "x"}
    assert s.decode(f'<b xmlns="{NS}">x</b>', root=True) == {"b": "x"}


def test_a_root_key_is_spelled_out_when_another_global_shares_its_name(tmp_path):
    for ns in ("a", "b"):
        (tmp_path / f"{ns}.xsd").write_text(
            f'<xs:schema xmlns:xs="{XS}" targetNamespace="urn:{ns}">'
            '<xs:element name="r" type="xs:int"/></xs:schema>'
        )
    s = xsdkit.SchemaSet.from_files([tmp_path / "a.xsd", tmp_path / "b.xsd"])
    assert s.decode('<r xmlns="urn:a">1</r>', root=True) == {"{urn:a}r": 1}


@pytest.mark.parametrize(
    "not_xml", ["<a", "report.xml", "", f'<a xmlns="{NS}">1</a><a/>', f'<a xmlns="{NS}">1</a>junk']
)
def test_lax_decoding_still_refuses_what_is_not_xml(not_xml):
    """`lax=True` returned `None` for text that was not XML at all."""
    s = build('<xs:element name="a" type="xs:int"/>')
    assert s.decode(f'<a xmlns="{NS}">x</a>', lax=True) == "x"
    with pytest.raises(xsdkit.DocumentError):
        s.decode(not_xml, lax=True)


@pytest.mark.parametrize(
    "builtin, lexical, expected",
    [
        ("date", "2024-12-01", datetime.date(2024, 12, 1)),
        ("date", "2024-12-01Z", "2024-12-01Z"),
        ("date", "2024-12-01+05:00", "2024-12-01+05:00"),
        ("time", "12:00:00.5", datetime.time(12, 0, 0, 500_000)),
        ("time", "12:00:00.1234567", "12:00:00.1234567"),
        (
            "dateTime",
            "2024-12-01T12:00:00.000001Z",
            datetime.datetime(2024, 12, 1, 12, 0, 0, 1, tzinfo=datetime.timezone.utc),
        ),
        ("dateTime", "2024-12-01T12:00:00.0000001", "2024-12-01T12:00:00.0000001"),
        ("dayTimeDuration", "PT0.000001S", datetime.timedelta(microseconds=1)),
        ("dayTimeDuration", "PT0.0000001S", "PT0.0000001S"),
        ("dayTimeDuration", "-PT1.5S", -datetime.timedelta(seconds=1.5)),
    ],
)
def test_a_value_datetime_cannot_hold_exactly_stays_lexical(builtin, lexical, expected):
    """`2024-12-01Z` lost its timezone, and `PT0.0000001S` became `timedelta(0)`."""
    value = build('<xs:element name="e" type="xs:string"/>').type(XS, builtin).validate(lexical)
    assert value == expected
    assert type(value) is type(expected)


def test_a_float_arrives_as_the_decimal_it_was_written_as():
    """`0.1` came back as `0.10000000149011612`, the same 32-bit value widened."""
    t = build('<xs:element name="e" type="xs:string"/>').type(XS, "float")
    assert t.validate("0.1") == 0.1
    assert t.validate("3.4028235E38") == 3.4028235e38
    assert math.isnan(t.validate("NaN"))
    assert t.validate("-INF") == float("-inf")


def test_a_qname_value_is_read_against_the_bindings_given():
    """There was no way to pass bindings, so no prefixed QName could be checked."""
    qname = build('<xs:element name="e" type="xs:string"/>').type(XS, "QName")
    assert qname.validate("ex:report", namespaces={"ex": NS}) == f"{{{NS}}}report"
    assert qname.validate("report", namespaces={"": NS}) == f"{{{NS}}}report"
    assert qname.is_valid("ex:report", namespaces={"ex": NS})
    assert not qname.is_valid("ex:report")
    with pytest.raises(xsdkit.InvalidValueError):
        qname.validate("ex:report")
    with pytest.raises(TypeError, match="not list"):
        qname.validate("ex:report", namespaces=["ex"])


def test_a_complex_type_with_simple_content_validates_its_value():
    """It raised "a complex type has no value space", and `facets` was `None`."""
    s = build(
        '<xs:complexType name="Price"><xs:simpleContent><xs:extension base="xs:decimal">'
        '<xs:attribute name="currency" type="xs:string"/>'
        "</xs:extension></xs:simpleContent></xs:complexType>"
        '<xs:complexType name="Small"><xs:simpleContent><xs:restriction base="tns:Price">'
        '<xs:maxInclusive value="100"/>'
        "</xs:restriction></xs:simpleContent></xs:complexType>"
    )
    price = s.type(NS, "Price")
    assert price.validate("19.95") == Decimal("19.95")
    assert price.facets is not None
    with pytest.raises(xsdkit.InvalidValueError):
        price.validate("cheap")

    small = s.type(NS, "Small")
    assert small.is_valid("99") and not small.is_valid("101")
    assert small.facets.max_inclusive == "100"

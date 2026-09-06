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


def test_schema_set_is_a_mapping():
    s = build('<xs:element name="a" type="xs:string"/>'
              '<xs:simpleType name="T"><xs:restriction base="xs:string"/></xs:simpleType>')
    # Its own declarations only — the fifty-odd built-ins would bury them.
    assert len(s) == 2
    assert sorted(s) == [f"{{{NS}}}T", f"{{{NS}}}a"]
    assert f"{{{NS}}}a" in s
    assert s[f"{{{NS}}}a"] == s.element(NS, "a")
    assert s[(NS, "T")] == s.type(NS, "T")

    # A built-in is not one of this schema's declarations, but is still
    # resolvable by name.
    assert f"{{{XS}}}string" not in s
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
    with pytest.raises(ValueError, match="str or bytes"):
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
    """`dict(schemas)` was documented long before it worked.

    `__len__`, `__contains__`, `__getitem__` and `__iter__` make something
    that *looks* like a mapping; `dict()` needs `keys` as well, and without it
    raised a `ValueError` about sequence lengths that told nobody anything.
    """
    s = build(
        '<xs:element name="a" type="xs:string"/>'
        '<xs:complexType name="T"><xs:sequence/></xs:complexType>'
    )
    assert s.keys() == list(s)
    assert [k for k, _ in s.items()] == s.keys()
    assert [type(v).__name__ for v in s.values()] == ["Element", "Type"]

    d = dict(s)
    assert len(d) == len(s)
    assert d[f"{{{NS}}}a"] == s[f"{{{NS}}}a"]

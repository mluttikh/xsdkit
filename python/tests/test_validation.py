"""Instance validation and typed reading, from Python."""

import datetime
import decimal

import pytest
import xsdkit
from conftest import NS, build

SCHEMA = """
<xs:element name="reading">
  <xs:complexType>
    <xs:sequence>
      <xs:element name="at"    type="xs:dateTime"/>
      <xs:element name="count" type="xs:int"/>
      <xs:element name="rate"  type="xs:decimal"/>
      <xs:element name="on"    type="xs:date"/>
      <xs:element name="ok"    type="xs:boolean"/>
      <xs:element name="tags"  type="xs:NMTOKENS"/>
      <xs:element name="blob"  type="xs:base64Binary"/>
      <xs:element name="note"  type="xs:string" minOccurs="0"/>
    </xs:sequence>
    <xs:attribute name="id"  type="xs:ID" use="required"/>
    <xs:attribute name="seq" type="xs:int"/>
  </xs:complexType>
</xs:element>
"""

DOC = """<reading xmlns="urn:example" id="r1" seq="7">
  <at>2024-12-30T12:39:15Z</at>
  <count>42</count>
  <rate>3.14</rate>
  <on>2024-03-31</on>
  <ok>true</ok>
  <tags>alpha beta</tags>
  <blob>aGVsbG8=</blob>
</reading>"""


@pytest.fixture(scope="module")
def schemas():
    return build(SCHEMA)


def texts(events):
    return [e.value for e in events if e.kind == "text" and e.value is not None]


# --- validating ------------------------------------------------------------


def test_a_conforming_document_is_valid(schemas):
    report = schemas.validate(DOC)
    assert report.is_valid
    assert bool(report)
    assert report.errors == []


def test_an_invalid_document_reports_rather_than_raises(schemas):
    report = schemas.validate(DOC.replace("<count>42</count>", "<count>nope</count>"))
    assert not report.is_valid
    assert not bool(report)
    codes = [d.code for d in report.errors]
    assert "XSD2004" in codes, codes
    assert "xs:int" in report.errors[0].message


def test_structural_errors_are_reported(schemas):
    missing = DOC.replace("<count>42</count>", "")
    assert not schemas.validate(missing).is_valid

    extra = DOC.replace("</reading>", "<nope/></reading>")
    assert any(d.code == "XSD2002" for d in schemas.validate(extra).errors)


def test_a_missing_required_attribute_is_reported(schemas):
    report = schemas.validate(DOC.replace(' id="r1"', ""))
    assert any(d.code == "XSD2006" for d in report.errors)


def test_diagnostics_carry_a_line_and_help(schemas):
    report = schemas.validate(DOC.replace("<count>42</count>", "<count>nope</count>"))
    d = report.errors[0]
    assert d.spans and d.spans[0].line >= 3
    assert str(d).startswith("error[XSD2004]")


# --- typed reading ---------------------------------------------------------


def test_values_arrive_as_native_python_types(schemas):
    events, report = schemas.read_typed(DOC)
    assert report.is_valid
    values = texts(events)

    assert datetime.datetime(2024, 12, 30, 12, 39, 15, tzinfo=datetime.timezone.utc) in values
    assert 42 in values
    assert decimal.Decimal("3.14") in values
    assert datetime.date(2024, 3, 31) in values
    assert True in values
    assert ["alpha", "beta"] in values
    assert b"hello" in values


def test_integers_are_ints_not_decimals(schemas):
    """The distinction the Rust side had to fix: xs:int's primitive is
    xs:decimal, and parsing against it loses the integer bounds."""
    events, _ = schemas.read_typed(DOC)
    count = next(v for v in texts(events) if v == 42)
    assert isinstance(count, int) and not isinstance(count, bool)
    assert not isinstance(count, decimal.Decimal)


def test_start_events_carry_declaration_and_type(schemas):
    events, _ = schemas.read_typed(DOC)
    start = events[0]
    assert start.kind == "start"
    assert start.local_name == "reading"
    assert start.declaration is not None
    assert start.declaration.qname == f"{{{NS}}}reading"
    assert start.type.is_complex
    assert not start.type_from_instance
    assert start.line >= 1


def test_attributes_arrive_typed(schemas):
    events, _ = schemas.read_typed(DOC)
    attrs = {a.local_name: a for a in events[0].attributes}
    assert attrs["id"].value == "r1"
    assert attrs["seq"].value == 7, "an xs:int attribute is an int"
    assert attrs["seq"].declaration is not None
    assert attrs["seq"].lexical == "7"


def test_a_callback_streams_instead_of_collecting(schemas):
    seen = []
    events, report = schemas.read_typed(DOC, on_event=seen.append)
    assert events is None, "the callback form returns no list"
    assert report.is_valid
    assert len(seen) > 10
    assert seen[0].kind == "start"
    assert seen[-1].kind == "end"


def test_a_raising_callback_propagates(schemas):
    class Boom(Exception):
        pass

    def explode(_):
        raise Boom

    with pytest.raises(Boom):
        schemas.read_typed(DOC, on_event=explode)


# --- xsi:type and xsi:nil --------------------------------------------------


def test_xsi_type_is_reported_on_the_event():
    s = build(
        '<xs:complexType name="Base"><xs:sequence>'
        '<xs:element name="a" type="xs:string"/></xs:sequence></xs:complexType>'
        '<xs:complexType name="Derived"><xs:complexContent>'
        '<xs:extension base="tns:Base"><xs:sequence>'
        '<xs:element name="b" type="xs:string"/></xs:sequence></xs:extension>'
        "</xs:complexContent></xs:complexType>"
        '<xs:element name="thing" type="tns:Base"/>'
    )
    doc = (
        '<thing xmlns="urn:example" xmlns:tns="urn:example" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" '
        'xsi:type="tns:Derived"><a>x</a><b>y</b></thing>'
    )
    events, report = s.read_typed(doc)
    assert report.is_valid, [str(d) for d in report.errors]
    assert events[0].type_from_instance
    assert events[0].type.qname == f"{{{NS}}}Derived"


def test_xsi_nil_is_reported():
    s = build('<xs:element name="v" type="xs:int" nillable="true"/>')
    doc = (
        '<v xmlns="urn:example" '
        'xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/>'
    )
    events, report = s.read_typed(doc)
    assert report.is_valid, [str(d) for d in report.errors]
    assert events[0].nil


# --- standalone value checking --------------------------------------------


def test_a_type_validates_a_value_on_its_own(schemas):
    xs = "http://www.w3.org/2001/XMLSchema"
    t = schemas.type(xs, "int")
    assert t.validate("42") == 42
    assert t.is_valid("42")
    assert not t.is_valid("nope")
    with pytest.raises(ValueError, match="xs:int"):
        t.validate("nope")


def test_facets_are_enforced_on_a_user_type():
    s = build(
        '<xs:simpleType name="Small"><xs:restriction base="xs:int">'
        '<xs:maxInclusive value="9"/></xs:restriction></xs:simpleType>'
    )
    t = s.type(NS, "Small")
    assert t.validate("9") == 9
    assert not t.is_valid("10")
    with pytest.raises(ValueError, match="maxInclusive"):
        t.validate("10")


def test_malformed_xml_is_a_diagnostic_not_an_exception(schemas):
    report = schemas.validate("<reading xmlns='urn:example'><at>")
    assert not report.is_valid
    assert any(d.code == "XSD1001" for d in report.errors)


# --- schema-supplied attribute values ---------------------------------------


def test_a_fixed_attribute_is_supplied_when_absent():
    """`<len>3.2</len>` still carries what the schema pinned with `fixed`."""
    s = build(
        '<xs:element name="len"><xs:complexType><xs:simpleContent>'
        '<xs:extension base="xs:double">'
        '<xs:attribute name="uom" type="xs:string" fixed="m"/>'
        "</xs:extension></xs:simpleContent></xs:complexType></xs:element>"
    )
    events, report = s.read_typed('<len xmlns="urn:example">3.2</len>')
    assert report.is_valid
    (uom,) = events[0].attributes
    assert uom.local_name == "uom"
    assert uom.value == "m"
    assert uom.from_schema, "the schema supplied it, the document did not"


def test_a_written_attribute_is_not_from_the_schema():
    s = build(
        '<xs:element name="len"><xs:complexType><xs:simpleContent>'
        '<xs:extension base="xs:double">'
        '<xs:attribute name="uom" type="xs:string" fixed="m"/>'
        "</xs:extension></xs:simpleContent></xs:complexType></xs:element>"
    )
    events, _ = s.read_typed('<len xmlns="urn:example" uom="m">3.2</len>')
    assert not events[0].attributes[0].from_schema


def test_an_inherited_fixed_unit_survives_a_vacuous_extension():
    """GML's measure family is built this way, so it has to work."""
    s = build(
        '<xs:complexType name="Metres"><xs:simpleContent>'
        '<xs:extension base="xs:double">'
        '<xs:attribute name="uom" type="xs:string" fixed="m"/>'
        "</xs:extension></xs:simpleContent></xs:complexType>"
        '<xs:complexType name="Depth"><xs:simpleContent>'
        '<xs:extension base="tns:Metres"/></xs:simpleContent></xs:complexType>'
        '<xs:element name="depth" type="tns:Depth"/>'
    )
    # The declared unit is reachable from the schema alone, with no document.
    uses = s.type(NS, "Depth").attributes
    assert [(u.local_name, u.fixed) for u in uses] == [("uom", "m")]

    events, report = s.read_typed('<depth xmlns="urn:example">120.5</depth>')
    assert report.is_valid
    assert events[0].attributes[0].value == "m"


def test_iter_typed_is_an_iterator(schemas):
    """The shape Python expects: a callback composes with nothing."""
    it = schemas.iter_typed(DOC)
    kinds = [ev.kind for ev in it]
    assert kinds[0] == "start" and kinds[-1] == "end"
    # The outcome is available without waiting for the loop to end.
    assert schemas.iter_typed(DOC).report.is_valid

    # It composes: enumerate, islice, a generator expression.
    import itertools

    firsts = list(itertools.islice(schemas.iter_typed(DOC), 2))
    assert len(firsts) == 2
    texts = [ev.value for ev in schemas.iter_typed(DOC) if ev.kind == "text"]
    assert texts, "typed values arrive through the iterator too"
    for i, ev in enumerate(schemas.iter_typed(DOC)):
        assert ev.kind in {"start", "text", "end"}
    assert i + 1 == len(kinds)


@pytest.mark.parametrize(
    "lexical,expected",
    [
        ("P1D", datetime.timedelta(days=1)),
        ("P2D", datetime.timedelta(days=2)),
        ("PT2H", datetime.timedelta(hours=2)),
        ("PT30M", datetime.timedelta(minutes=30)),
        ("PT0.5S", datetime.timedelta(seconds=0.5)),
        ("P1DT2H3M4S", datetime.timedelta(days=1, hours=2, minutes=3, seconds=4)),
        ("P400D", datetime.timedelta(days=400)),
        ("-P1D", -datetime.timedelta(days=1)),
        ("-P1DT2H30M", -datetime.timedelta(days=1, hours=2, minutes=30)),
    ],
)
def test_daytimeduration_days_are_days(lexical, expected):
    """`timedelta`'s seventh positional argument is *weeks*, not days.

    Days were being passed there, so every `xs:dayTimeDuration` carrying a day
    component came back seven times too large — and silently, because the type
    was right and only the number was wrong.
    """
    xs = build('<xs:element name="e" type="xs:string"/>')
    assert xs.type(
        "http://www.w3.org/2001/XMLSchema", "dayTimeDuration"
    ).validate(lexical) == expected

"""Decoding documents into Python data."""

import datetime
from decimal import Decimal

import pytest
import xsdkit
from conftest import NS, build

REPORT = """
<xs:element name="report">
  <xs:complexType>
    <xs:sequence>
      <xs:element name="title" type="xs:string"/>
      <xs:element name="note" type="xs:string" minOccurs="0"/>
      <xs:element name="item" minOccurs="0" maxOccurs="unbounded">
        <xs:complexType>
          <xs:simpleContent>
            <xs:extension base="xs:decimal">
              <xs:attribute name="sku" type="xs:string"/>
            </xs:extension>
          </xs:simpleContent>
        </xs:complexType>
      </xs:element>
    </xs:sequence>
    <xs:attribute name="id" type="xs:string"/>
    <xs:attribute name="issued" type="xs:date"/>
  </xs:complexType>
</xs:element>
"""


def doc(body: str) -> str:
    return f'<report xmlns="{NS}">{body}</report>'


def test_values_arrive_in_their_value_space():
    s = build(REPORT)
    d = s.decode(doc("<title>t</title><item sku='a'>19.95</item>"))
    assert d["title"] == "t"
    # Not the string "19.95" — decoding sits on top of a typed PSVI.
    assert d["item"][0]["$"] == Decimal("19.95")


def test_a_date_attribute_is_a_date():
    s = build(REPORT)
    d = s.decode(f'<report xmlns="{NS}" issued="2024-12-01"><title>t</title></report>')
    assert d["@issued"] == datetime.date(2024, 12, 1)


@pytest.mark.parametrize(
    "body, expected",
    [
        ("<title>t</title>", 0),
        ("<title>t</title><item>1</item>", 1),
        ("<title>t</title><item>1</item><item>2</item>", 2),
    ],
)
def test_a_repeating_child_is_always_a_list(body, expected):
    """The shape comes from the schema, so it does not move under you."""
    s = build(REPORT)
    d = s.decode(doc(body))
    assert isinstance(d["item"], list)
    assert len(d["item"]) == expected


def test_a_single_child_that_is_absent_has_no_key():
    s = build(REPORT)
    d = s.decode(doc("<title>t</title>"))
    assert "note" not in d
    # But the repeating one is present and empty, which is the whole point.
    assert d["item"] == []


def test_attributes_are_prefixed_and_values_share_the_dollar_key():
    s = build(REPORT)
    d = s.decode(doc("<title>t</title><item sku='AB-1'>1.5</item>"))
    assert d["item"][0] == {"@sku": "AB-1", "$": Decimal("1.5")}


def test_a_simple_element_with_no_attributes_is_just_its_value():
    s = build(REPORT)
    d = s.decode(doc("<title>plain</title>"))
    assert d["title"] == "plain"


def test_nil_decodes_to_none():
    s = build('<xs:element name="e" type="xs:int" nillable="true"/>')
    d = s.decode(
        f'<e xmlns="{NS}" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"'
        ' xsi:nil="true"/>'
    )
    assert d is None


def test_colliding_local_names_are_spelled_out():
    """Two `title`s under one parent, in different namespaces."""
    xsd = f"""<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                 xmlns:tns="{NS}" targetNamespace="{NS}"
                 elementFormDefault="unqualified">
                <xs:element name="title" type="xs:string"/>
                <xs:element name="root">
                  <xs:complexType><xs:sequence>
                    <xs:element ref="tns:title"/>
                    <xs:element name="title" type="xs:string"/>
                  </xs:sequence></xs:complexType>
                </xs:element>
              </xs:schema>"""
    s = xsdkit.SchemaSet.from_string(xsd)
    d = s.decode(f'<root xmlns="{NS}"><title>global</title><title xmlns="">local</title></root>')
    assert d == {"{urn:example}title": "global", "title": "local"}


def test_an_invalid_document_raises_but_lax_hands_the_data_over():
    s = build(REPORT)
    bad = doc("<title>t</title><item>not-a-number</item>")
    with pytest.raises(xsdkit.XsdError):
        s.decode(bad)
    # The trap this removes: data from a document that does not fit.
    d = s.decode(bad, lax=True)
    assert d["item"][0] == "not-a-number"


def test_mixed_content_keeps_its_text():
    s = build(
        """<xs:element name="p">
             <xs:complexType mixed="true"><xs:sequence>
               <xs:element name="b" type="xs:string"/>
             </xs:sequence></xs:complexType>
           </xs:element>"""
    )
    d = s.decode(f'<p xmlns="{NS}">before<b>bold</b>after</p>')
    assert d["b"] == "bold"
    assert d["$"] == "beforeafter"


def test_a_document_may_be_given_as_a_path(tmp_path):
    s = build(REPORT)
    p = tmp_path / "r.xml"
    p.write_text(doc("<title>from a file</title>"))
    assert s.decode(p)["title"] == "from a file"


def test_a_path_as_a_string_says_what_went_wrong():
    """The trap: a path is a `str`, and so is a document."""
    s = build(REPORT)
    with pytest.raises(xsdkit.XsdError) as excinfo:
        s.decode("report.xml")
    message = str(excinfo.value)
    assert "no root element" in message
    # Naming the likely mistake is the whole point.
    assert "not a path" in message


def test_bytes_still_decode(tmp_path):
    s = build(REPORT)
    p = tmp_path / "r.xml"
    p.write_text(doc("<title>bytes</title>"))
    assert s.decode(p.read_bytes())["title"] == "bytes"


def test_an_empty_complex_element_keeps_its_schema_shape():
    """`children()` being empty is not the same question as "has a value"."""
    s = build(
        """<xs:element name="e">
             <xs:complexType>
               <xs:sequence>
                 <xs:element name="item" minOccurs="0" maxOccurs="unbounded"
                             type="xs:string"/>
               </xs:sequence>
               <xs:attribute name="a" type="xs:string"/>
             </xs:complexType>
           </xs:element>"""
    )
    d = s.decode(f'<e xmlns="{NS}" a="x"/>')
    # Not {"@a": "x", "$": None}: there is no value here to put under "$".
    assert d == {"@a": "x", "item": []}


def test_an_empty_complex_element_without_attributes_is_a_dict():
    s = build(
        """<xs:element name="e">
             <xs:complexType><xs:sequence>
               <xs:element name="item" minOccurs="0" maxOccurs="unbounded"
                           type="xs:string"/>
             </xs:sequence></xs:complexType>
           </xs:element>"""
    )
    assert s.decode(f'<e xmlns="{NS}"/>') == {"item": []}


def test_a_skipped_wildcard_does_not_swallow_the_document():
    s = build(
        """<xs:element name="root">
             <xs:complexType><xs:sequence>
               <xs:element name="a" type="xs:string"/>
               <xs:any namespace="##other" processContents="skip" minOccurs="0"/>
             </xs:sequence></xs:complexType>
           </xs:element>"""
    )
    d = s.decode(
        f'<root xmlns="{NS}" xmlns:o="urn:other"><a>x</a><o:junk><o:deep/></o:junk></root>'
    )
    assert d is not None, "a valid document must not decode to None"
    assert d["a"] == "x"


def test_decode_errors_carry_their_diagnostics():
    s = build(REPORT)
    with pytest.raises(xsdkit.XsdError) as excinfo:
        s.decode(doc("<title>t</title><item>nope</item>"))
    # A formatted string cannot be filtered by code or pointed at a line.
    diagnostics = excinfo.value.diagnostics
    assert len(diagnostics) >= 1
    assert any(d.code.startswith("XSD") for d in diagnostics)


ORDERED = """
<xs:element name="order">
  <xs:complexType>
    <xs:sequence>
      <xs:element name="id" type="xs:int"/>
      <xs:element name="line" type="xs:string" minOccurs="0" maxOccurs="unbounded"/>
      <xs:element name="total" type="xs:int"/>
    </xs:sequence>
    <xs:attribute name="ref" type="xs:string"/>
  </xs:complexType>
</xs:element>
"""


def test_keys_keep_document_order():
    """`==` on dicts ignores order, which is how the old order slipped through
    every other test here: repeating children used to be seeded first, so a
    `line` printed ahead of the `id` that precedes it."""
    s = build(ORDERED)
    d = s.decode(f'<order xmlns="{NS}" ref="r"><id>1</id><line>a</line><total>2</total></order>')
    assert list(d) == ["@ref", "id", "line", "total"]


def test_an_absent_repeating_child_comes_after_what_was_present():
    s = build(ORDERED)
    d = s.decode(f'<order xmlns="{NS}"><id>1</id><total>2</total></order>')
    # It had no position in the document, so it takes none among its siblings.
    assert list(d) == ["id", "total", "line"]
    assert d["line"] == []


def test_a_decimal_keeps_the_scale_it_was_written_with():
    s = build('<xs:element name="p" type="xs:decimal"/>')
    d = s.decode(f'<p xmlns="{NS}">4.50</p>')
    assert str(d) == "4.50"
    assert d == Decimal("4.5")  # one value, however it was written
    assert str(d * 2) == "9.00"  # and arithmetic keeps the precision


def test_every_path_to_a_decimal_agrees():
    xs = "http://www.w3.org/2001/XMLSchema"
    s = build('<xs:element name="p" type="xs:decimal"/>')
    doc = f'<p xmlns="{NS}">4.50</p>'
    via_decode = s.decode(doc)
    via_events = next(e.value for e in s.iter_typed(doc) if e.kind == "text")
    via_type = s.type(xs, "decimal").validate("4.50")
    assert str(via_decode) == str(via_events) == str(via_type) == "4.50"


def test_a_list_of_decimals_keeps_each_scale():
    s = build(
        '<xs:simpleType name="L"><xs:list itemType="xs:decimal"/></xs:simpleType>'
        '<xs:element name="p" type="tns:L"/>'
    )
    d = s.decode(f'<p xmlns="{NS}">1.0 2.50 3</p>')
    assert [str(x) for x in d] == ["1.0", "2.50", "3"]

//! Round-tripping a compiled schema through serde.
//!
//! Compiling a large schema set is the expensive part of using this crate;
//! serializing the result means paying it once. That only holds if the copy
//! that comes back is indistinguishable from the original — so these tests
//! compare behaviour, not bytes.

#![cfg(feature = "serde")]

use xsdkit::*;

const NS: &str = "urn:example";

/// A schema that touches most of what the model can hold: a substitution
/// group, an identity constraint, a wildcard, a list, a union, an enumeration
/// with a `QName` literal, and facets on a derived type.
fn kitchen_sink() -> Schemas {
    let xsd = format!(
        r###"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                        xmlns:tns="{NS}" targetNamespace="{NS}"
                        elementFormDefault="qualified">
          <xs:simpleType name="Unit">
            <xs:restriction base="xs:string">
              <xs:enumeration value="m"/>
              <xs:enumeration value="ft"/>
            </xs:restriction>
          </xs:simpleType>
          <xs:simpleType name="Units">
            <xs:list itemType="tns:Unit"/>
          </xs:simpleType>
          <xs:simpleType name="Depth">
            <xs:union memberTypes="xs:double tns:Unit"/>
          </xs:simpleType>
          <xs:simpleType name="Small">
            <xs:restriction base="xs:int">
              <xs:minInclusive value="0"/>
              <xs:maxExclusive value="100"/>
              <xs:pattern value="[0-9]+"/>
            </xs:restriction>
          </xs:simpleType>

          <xs:complexType name="Measure">
            <xs:simpleContent>
              <xs:extension base="tns:Depth">
                <xs:attribute name="uom" type="tns:Unit" default="m"/>
              </xs:extension>
            </xs:simpleContent>
          </xs:complexType>

          <xs:element name="shape" type="xs:string" abstract="true"/>
          <xs:element name="circle" type="xs:string" substitutionGroup="tns:shape"/>

          <xs:element name="well">
            <xs:complexType>
              <xs:sequence>
                <xs:element name="name" type="xs:string"/>
                <xs:element name="depth" type="tns:Measure" minOccurs="0"/>
                <xs:element name="rank" type="tns:Small" minOccurs="0"/>
                <xs:element name="units" type="tns:Units" minOccurs="0"/>
                <xs:element ref="tns:shape" minOccurs="0" maxOccurs="unbounded"/>
                <xs:element name="log" minOccurs="0" maxOccurs="unbounded">
                  <xs:complexType>
                    <xs:attribute name="id" type="xs:ID" use="required"/>
                    <xs:anyAttribute namespace="##other" processContents="lax"/>
                  </xs:complexType>
                </xs:element>
              </xs:sequence>
              <xs:attribute name="api" type="xs:string" use="required"/>
            </xs:complexType>
            <xs:key name="LogKey">
              <xs:selector xpath="tns:log"/>
              <xs:field xpath="@id"/>
            </xs:key>
          </xs:element>
        </xs:schema>"###
    );
    SchemaSetBuilder::new()
        .text(xsd, "mem://main.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"))
}

/// Documents that between them reach every construct above, half of them
/// invalid so the copy has to reject as well as accept.
const DOCS: &[&str] = &[
    r#"<well xmlns="urn:example" api="42"><name>A</name></well>"#,
    r#"<well xmlns="urn:example" api="42"><name>A</name>
         <depth uom="ft">1200.5</depth><rank>7</rank>
         <units>m ft m</units><circle>o</circle><circle>O</circle>
         <log id="l1" xmlns:x="urn:other" x:note="hi"/><log id="l2"/></well>"#,
    // Missing the required attribute.
    r#"<well xmlns="urn:example"><name>A</name></well>"#,
    // `rank` is out of range, and not by a facet the base type carries.
    r#"<well xmlns="urn:example" api="42"><name>A</name><rank>100</rank></well>"#,
    // Not a member of the union.
    r#"<well xmlns="urn:example" api="42"><name>A</name><depth>fathoms</depth></well>"#,
    // Not a member of the list's item type.
    r#"<well xmlns="urn:example" api="42"><name>A</name><units>m yd</units></well>"#,
    // The abstract head cannot appear itself.
    r#"<well xmlns="urn:example" api="42"><name>A</name><shape>x</shape></well>"#,
    // Duplicate key, which only the identity constraint catches.
    r#"<well xmlns="urn:example" api="42"><name>A</name>
         <log id="dup"/><log id="dup"/></well>"#,
];

/// Every diagnostic a schema produces for `DOCS`, rendered as text.
///
/// Comparing the rendered form rather than the ids catches a copy that
/// validates "correctly" while pointing at the wrong component.
fn verdicts(s: &Schemas) -> Vec<String> {
    DOCS.iter()
        .map(|xml| {
            let d = s.instance_validator().validate(xml).diagnostics;
            format!("{d}")
        })
        .collect()
}

#[test]
fn a_round_trip_validates_documents_identically() {
    let original = kitchen_sink();
    let bytes = postcard::to_allocvec(&original).expect("serialize");
    let copy: Schemas = postcard::from_bytes(&bytes).expect("deserialize");

    let before = verdicts(&original);
    let after = verdicts(&copy);
    // The corpus is only a real test if it exercises both outcomes.
    assert!(before.iter().any(|v| v.is_empty()), "no document was valid");
    assert!(before.iter().any(|v| !v.is_empty()), "no document failed");
    assert_eq!(before, after);
}

/// The one property the whole scheme rests on.
///
/// A `Symbol` is an index into the interner's table, and those indices are
/// spread through every component. If the table comes back in a different
/// order the schema is still well-formed — every id still resolves — but
/// every name in it silently means something else.
#[test]
fn a_round_trip_preserves_symbol_ids() {
    let original = kitchen_sink();
    let copy: Schemas = postcard::from_bytes(&postcard::to_allocvec(&original).unwrap()).unwrap();

    // A `QName` built by the original still names the same thing in the copy.
    let mut checked = 0;
    for (q, id) in &original.globals().types {
        assert_eq!(copy.display_name(*q), original.display_name(*q));
        assert_eq!(
            copy.type_(Some(NS), "Unit"),
            original.type_(Some(NS), "Unit")
        );
        assert!(copy.get_type(*id).is_some());
        checked += 1;
    }
    assert!(checked > 40, "expected the built-ins too, got {checked}");

    for (q, id) in &original.globals().elements {
        assert_eq!(copy.display_name(*q), original.display_name(*q));
        assert_eq!(copy[*id].name, original[*id].name);
    }
    assert_eq!(
        copy.element(Some(NS), "well"),
        original.element(Some(NS), "well")
    );
    // Interning is what makes the ids meaningful, so the table has to be the
    // same length as well as the same order.
    assert_eq!(copy.names().len(), original.names().len());
}

/// `QName` is a struct, and a JSON object's key must be a string — so every
/// map keyed by one is written as a sequence of pairs. Without that the model
/// would serialize to a binary format and not to a self-describing one, and
/// nobody would find out until they tried.
#[test]
fn a_self_describing_format_round_trips_too() {
    let original = kitchen_sink();
    let json = serde_json::to_string(&original).expect("serialize");
    let copy: Schemas = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(verdicts(&copy), verdicts(&original));
}

/// Provenance and the language the reader applied both survive.
///
/// `xsd_version` is not recoverable from the components — it decides whether
/// `0000` is a year and whether `+INF` is a double — so a copy that lost it
/// would validate a slightly different language.
#[test]
fn a_round_trip_keeps_what_the_components_do_not_say() {
    for version in [Version::Xsd10, Version::Xsd11] {
        let original = SchemaSetBuilder::new()
            .version(version)
            .text(
                format!(
                    r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                                  xmlns:tns="{NS}" targetNamespace="{NS}">
                         <xs:element name="d" type="xs:double"/>
                       </xs:schema>"#
                ),
                "mem://v.xsd",
            )
            .build()
            .unwrap_or_else(|d| panic!("{d}"));
        let copy: Schemas =
            postcard::from_bytes(&postcard::to_allocvec(&original).unwrap()).unwrap();

        assert_eq!(copy.xsd_version(), version);
        assert_eq!(copy.documents().len(), 1);
        assert_eq!(copy.documents()[0].uri, "mem://v.xsd");
        // `+INF` is an XSD 1.1 double and not an XSD 1.0 one, which is the
        // difference `xsd_version` is carrying.
        let d = copy
            .instance_validator()
            .validate(r#"<d xmlns="urn:example">+INF</d>"#)
            .diagnostics;
        assert_eq!(d.has_errors(), version == Version::Xsd10, "{d}");
    }
}

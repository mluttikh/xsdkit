//! Validating instance documents against a schema.

use xsdkit::instance::PsviEvent;
use xsdkit::*;

const NS: &str = "urn:example";

fn schema(body: &str) -> Schemas {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .text(xsd, "mem://main.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"))
}

fn check(s: &Schemas, xml: &str) -> Diagnostics {
    s.instance_validator().validate(xml).diagnostics
}

fn valid(s: &Schemas, xml: &str) {
    let d = check(s, xml);
    assert!(!d.has_errors(), "expected valid, got:\n{d}");
}

fn invalid(s: &Schemas, xml: &str, code: DiagCode) {
    let d = check(s, xml);
    assert!(
        d.errors().any(|e| e.code == code),
        "expected {code}, got:\n{d}"
    );
}

/// A report of `<report><title>..</title><count>..</count></report>`.
fn report_schema() -> Schemas {
    schema(
        r#"<xs:element name="report">
             <xs:complexType>
               <xs:sequence>
                 <xs:element name="title" type="xs:string"/>
                 <xs:element name="count" type="xs:int"/>
                 <xs:element name="note" type="xs:string" minOccurs="0"
                             maxOccurs="unbounded"/>
               </xs:sequence>
               <xs:attribute name="id" type="xs:ID" use="required"/>
               <xs:attribute name="lang" type="xs:language"/>
             </xs:complexType>
           </xs:element>"#,
    )
}

// ---------------------------------------------------------------------------
// The happy path
// ---------------------------------------------------------------------------

#[test]
fn a_conforming_document_validates() {
    let s = report_schema();
    valid(
        &s,
        r#"<report xmlns="urn:example" id="r1" lang="en-GB">
             <title>Quarterly</title>
             <count>42</count>
             <note>first</note>
             <note>second</note>
           </report>"#,
    );
}

#[test]
fn optional_and_repeating_children_are_honoured() {
    let s = report_schema();
    valid(
        &s,
        r#"<report xmlns="urn:example" id="r1">
             <title>t</title><count>1</count>
           </report>"#,
    );
}

// ---------------------------------------------------------------------------
// Structure
// ---------------------------------------------------------------------------

#[test]
fn an_undeclared_root_is_reported() {
    let s = report_schema();
    invalid(
        &s,
        r#"<nope xmlns="urn:example"/>"#,
        DiagCode::ElementNotDeclared,
    );
}

#[test]
fn an_element_out_of_order_is_reported() {
    let s = report_schema();
    invalid(
        &s,
        r#"<report xmlns="urn:example" id="r1">
             <count>1</count><title>t</title>
           </report>"#,
        DiagCode::UnexpectedElement,
    );
}

#[test]
fn a_missing_required_child_is_reported() {
    let s = report_schema();
    invalid(
        &s,
        r#"<report xmlns="urn:example" id="r1"><title>t</title></report>"#,
        DiagCode::IncompleteContent,
    );
}

#[test]
fn an_unexpected_child_is_reported() {
    let s = report_schema();
    invalid(
        &s,
        r#"<report xmlns="urn:example" id="r1">
             <title>t</title><count>1</count><extra/>
           </report>"#,
        DiagCode::UnexpectedElement,
    );
}

#[test]
fn character_data_in_element_only_content_is_reported() {
    let s = report_schema();
    invalid(
        &s,
        r#"<report xmlns="urn:example" id="r1">
             stray text
             <title>t</title><count>1</count>
           </report>"#,
        DiagCode::UnexpectedText,
    );
}

/// Whitespace between elements is not character data for this purpose.
#[test]
fn whitespace_between_elements_is_not_content() {
    let s = report_schema();
    valid(
        &s,
        "<report xmlns=\"urn:example\" id=\"r1\">\n\n  <title>t</title>\n  <count>1</count>\n</report>",
    );
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

#[test]
fn a_bad_element_value_is_reported_with_its_type() {
    let s = report_schema();
    let d = check(
        &s,
        r#"<report xmlns="urn:example" id="r1">
             <title>t</title><count>not-a-number</count>
           </report>"#,
    );
    let e = d
        .errors()
        .find(|e| e.code == DiagCode::InvalidValue)
        .unwrap_or_else(|| panic!("{d}"));
    assert!(e.message.contains("xs:int"), "{}", e.message);
}

#[test]
fn facets_are_enforced_on_instance_values() {
    let s = schema(
        r#"<xs:simpleType name="Small">
             <xs:restriction base="xs:int"><xs:maxInclusive value="9"/></xs:restriction>
           </xs:simpleType>
           <xs:element name="v" type="tns:Small"/>"#,
    );
    valid(&s, r#"<v xmlns="urn:example">9</v>"#);
    invalid(
        &s,
        r#"<v xmlns="urn:example">10</v>"#,
        DiagCode::InvalidValue,
    );
}

/// Numeric text is whitespace-collapsed before parsing, so this is valid.
#[test]
fn numeric_values_tolerate_surrounding_whitespace() {
    let s = schema(r#"<xs:element name="v" type="xs:int"/>"#);
    valid(&s, "<v xmlns=\"urn:example\"> 42 </v>");
}

// ---------------------------------------------------------------------------
// Attributes
// ---------------------------------------------------------------------------

#[test]
fn a_missing_required_attribute_is_reported() {
    let s = report_schema();
    invalid(
        &s,
        r#"<report xmlns="urn:example"><title>t</title><count>1</count></report>"#,
        DiagCode::MissingRequiredAttribute,
    );
}

#[test]
fn an_undeclared_attribute_is_reported() {
    let s = report_schema();
    invalid(
        &s,
        r#"<report xmlns="urn:example" id="r1" nope="x">
             <title>t</title><count>1</count>
           </report>"#,
        DiagCode::AttributeNotAllowed,
    );
}

#[test]
fn a_bad_attribute_value_is_reported() {
    let s = report_schema();
    invalid(
        &s,
        r#"<report xmlns="urn:example" id="r1" lang="not a language">
             <title>t</title><count>1</count>
           </report>"#,
        DiagCode::InvalidValue,
    );
}

/// A `fixed` attribute may be repeated but not contradicted — the case the
/// units layer will lean on.
#[test]
fn a_fixed_attribute_may_be_repeated_but_not_changed() {
    let s = schema(
        r#"<xs:element name="len">
             <xs:complexType><xs:simpleContent>
               <xs:extension base="xs:double">
                 <xs:attribute name="uom" type="xs:string" fixed="m"/>
               </xs:extension>
             </xs:simpleContent></xs:complexType>
           </xs:element>"#,
    );
    valid(&s, r#"<len xmlns="urn:example" uom="m">3.2</len>"#);
    valid(&s, r#"<len xmlns="urn:example">3.2</len>"#);
    invalid(
        &s,
        r#"<len xmlns="urn:example" uom="ft">3.2</len>"#,
        DiagCode::InvalidValue,
    );
}

// ---------------------------------------------------------------------------
// xsi:type and xsi:nil
// ---------------------------------------------------------------------------

fn derived_schema() -> Schemas {
    schema(
        r#"<xs:complexType name="Base">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="Derived">
             <xs:complexContent><xs:extension base="tns:Base">
               <xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence>
             </xs:extension></xs:complexContent>
           </xs:complexType>
           <xs:complexType name="Unrelated"><xs:sequence/></xs:complexType>
           <xs:element name="thing" type="tns:Base"/>"#,
    )
}

#[test]
fn xsi_type_substitutes_a_derived_type() {
    let s = derived_schema();
    // Without the override, `b` is not allowed.
    invalid(
        &s,
        r#"<thing xmlns="urn:example"><a>x</a><b>y</b></thing>"#,
        DiagCode::UnexpectedElement,
    );
    // With it, the derived content model applies.
    valid(
        &s,
        r#"<thing xmlns="urn:example" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                  xmlns:tns="urn:example" xsi:type="tns:Derived">
             <a>x</a><b>y</b>
           </thing>"#,
    );
}

#[test]
fn xsi_type_must_name_a_derived_type() {
    let s = derived_schema();
    invalid(
        &s,
        r#"<thing xmlns="urn:example" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                  xmlns:tns="urn:example" xsi:type="tns:Unrelated"/>"#,
        DiagCode::InvalidXsiType,
    );
    invalid(
        &s,
        r#"<thing xmlns="urn:example" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                  xmlns:tns="urn:example" xsi:type="tns:Nonexistent"/>"#,
        DiagCode::InvalidXsiType,
    );
}

#[test]
fn xsi_nil_permits_an_empty_element_that_would_otherwise_be_invalid() {
    let s = schema(r#"<xs:element name="v" type="xs:int" nillable="true"/>"#);
    // Empty content is not a valid xs:int.
    invalid(&s, r#"<v xmlns="urn:example"></v>"#, DiagCode::InvalidValue);
    valid(
        &s,
        r#"<v xmlns="urn:example" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
              xsi:nil="true"/>"#,
    );
}

#[test]
fn a_nil_element_may_not_have_content() {
    let s = schema(r#"<xs:element name="v" type="xs:int" nillable="true"/>"#);
    invalid(
        &s,
        r#"<v xmlns="urn:example" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
              xsi:nil="true">42</v>"#,
        DiagCode::NilElementNotEmpty,
    );
}

// ---------------------------------------------------------------------------
// Substitution groups
// ---------------------------------------------------------------------------

#[test]
fn a_substituting_element_is_matched_with_its_own_type() {
    let s = schema(
        r#"<xs:element name="shape" type="xs:string" abstract="true"/>
           <xs:element name="circle" type="xs:int" substitutionGroup="tns:shape"/>
           <xs:element name="drawing">
             <xs:complexType><xs:sequence>
               <xs:element ref="tns:shape" maxOccurs="unbounded"/>
             </xs:sequence></xs:complexType>
           </xs:element>"#,
    );
    // `circle` substitutes, and is validated as xs:int — its own type, not
    // the head's xs:string.
    valid(
        &s,
        r#"<drawing xmlns="urn:example"><circle>1</circle><circle>2</circle></drawing>"#,
    );
    invalid(
        &s,
        r#"<drawing xmlns="urn:example"><circle>not-an-int</circle></drawing>"#,
        DiagCode::InvalidValue,
    );
    // The head is abstract, so it cannot appear itself.
    invalid(
        &s,
        r#"<drawing xmlns="urn:example"><shape>x</shape></drawing>"#,
        DiagCode::UnexpectedElement,
    );
}

// ---------------------------------------------------------------------------
// The PSVI
// ---------------------------------------------------------------------------

#[test]
fn the_psvi_carries_typed_values() {
    let s = report_schema();
    let mut texts = Vec::new();
    let report = s.instance_validator().validate_with(
        r#"<report xmlns="urn:example" id="r1">
             <title>Quarterly</title><count>42</count>
           </report>"#,
        |ev| {
            if let PsviEvent::Text { value: Some(v), .. } = ev {
                texts.push(v);
            }
        },
    );
    assert!(report.is_valid(), "{}", report.diagnostics);
    assert!(texts.contains(&Value::String("Quarterly".into())));
    assert!(
        texts.contains(&Value::Integer(42)),
        "count must arrive as an integer"
    );
}

#[test]
fn the_psvi_reports_the_declaration_and_type_in_force() {
    let s = derived_schema();
    let mut starts = Vec::new();
    s.instance_validator().validate_with(
        r#"<thing xmlns="urn:example" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                  xmlns:tns="urn:example" xsi:type="tns:Derived">
             <a>x</a><b>y</b>
           </thing>"#,
        |ev| {
            if let PsviEvent::StartElement {
                type_id,
                type_from_instance,
                declaration,
                ..
            } = ev
            {
                starts.push((declaration, type_id, type_from_instance));
            }
        },
    );
    let (decl, ty, from_instance) = starts[0];
    assert!(
        decl.is_some(),
        "the root resolves to its global declaration"
    );
    assert!(from_instance, "xsi:type chose the type");
    assert_eq!(ty, s.type_(Some(NS), "Derived").unwrap());
}

#[test]
fn attributes_reach_the_psvi_typed() {
    let s = report_schema();
    let mut attrs = Vec::new();
    s.instance_validator().validate_with(
        r#"<report xmlns="urn:example" id="r1" lang="en">
             <title>t</title><count>1</count>
           </report>"#,
        |ev| {
            // Only the root carries attributes here; later children would
            // otherwise overwrite it with their empty lists.
            if let PsviEvent::StartElement { attributes, .. } = ev {
                if attrs.is_empty() {
                    attrs = attributes;
                }
            }
        },
    );
    let names: Vec<_> = attrs
        .iter()
        .map(|a| s.names().resolve(a.name.local).to_string())
        .collect();
    assert!(names.contains(&"id".to_string()));
    assert!(names.contains(&"lang".to_string()));
    assert!(attrs.iter().all(|a| a.declaration.is_some()));
}

// ---------------------------------------------------------------------------
// Malformed input
// ---------------------------------------------------------------------------

#[test]
fn malformed_xml_is_a_diagnostic_not_a_panic() {
    let s = report_schema();
    invalid(
        &s,
        "<report xmlns=\"urn:example\"><title>",
        DiagCode::MalformedXml,
    );
}

#[test]
fn a_truncated_document_is_reported() {
    let s = report_schema();
    let d = check(
        &s,
        r#"<report xmlns="urn:example" id="r1"><title>t</title>"#,
    );
    assert!(d.has_errors(), "an unclosed document must not validate");
}

#[test]
fn diagnostics_carry_a_line_number() {
    let s = report_schema();
    let d = check(
        &s,
        "<report xmlns=\"urn:example\" id=\"r1\">\n  <title>t</title>\n  <count>oops</count>\n</report>",
    );
    let e = d.errors().next().unwrap_or_else(|| panic!("{d}"));
    assert!(!e.spans.is_empty());
    assert!(
        e.spans[0].line >= 3,
        "expected line 3 or later, got {}",
        e.spans[0].line
    );
}

// ---------------------------------------------------------------------------
// Schema-supplied attribute values
// ---------------------------------------------------------------------------

/// An absent attribute with `fixed` or `default` is *supplied* by the schema.
///
/// This is what makes a schema-declared unit usable: `<len>3.2</len>` still
/// has a unit, and a reader that only reported attributes the document spelled
/// out would miss it entirely.
#[test]
fn a_fixed_attribute_is_supplied_when_absent() {
    let s = schema(
        r#"<xs:element name="len">
             <xs:complexType><xs:simpleContent>
               <xs:extension base="xs:double">
                 <xs:attribute name="uom" type="xs:string" fixed="m"/>
               </xs:extension>
             </xs:simpleContent></xs:complexType>
           </xs:element>"#,
    );
    let mut attrs = Vec::new();
    let report =
        s.instance_validator()
            .validate_with(r#"<len xmlns="urn:example">3.2</len>"#, |ev| {
                if let PsviEvent::StartElement { attributes, .. } = ev {
                    attrs = attributes;
                }
            });
    assert!(report.is_valid(), "{}", report.diagnostics);
    assert_eq!(attrs.len(), 1, "the schema supplies the absent uom");
    assert_eq!(attrs[0].lexical, "m");
    assert!(attrs[0].from_schema, "and says it came from the schema");
    assert_eq!(attrs[0].value, Some(Value::String("m".into())));
}

#[test]
fn a_default_attribute_is_supplied_too() {
    let s = schema(
        r#"<xs:element name="v">
             <xs:complexType><xs:simpleContent>
               <xs:extension base="xs:int">
                 <xs:attribute name="scale" type="xs:int" default="1"/>
               </xs:extension>
             </xs:simpleContent></xs:complexType>
           </xs:element>"#,
    );
    let mut attrs = Vec::new();
    s.instance_validator()
        .validate_with(r#"<v xmlns="urn:example">7</v>"#, |ev| {
            if let PsviEvent::StartElement { attributes, .. } = ev {
                attrs = attributes;
            }
        });
    assert_eq!(attrs.len(), 1);
    assert_eq!(attrs[0].value, Some(Value::Integer(1)));
    assert!(attrs[0].from_schema);
}

/// A value the document *did* spell out is not marked as schema-supplied.
#[test]
fn a_written_attribute_is_not_marked_as_from_the_schema() {
    let s = schema(
        r#"<xs:element name="len">
             <xs:complexType><xs:simpleContent>
               <xs:extension base="xs:double">
                 <xs:attribute name="uom" type="xs:string" fixed="m"/>
               </xs:extension>
             </xs:simpleContent></xs:complexType>
           </xs:element>"#,
    );
    let mut attrs = Vec::new();
    s.instance_validator()
        .validate_with(r#"<len xmlns="urn:example" uom="m">3.2</len>"#, |ev| {
            if let PsviEvent::StartElement { attributes, .. } = ev {
                attrs = attributes;
            }
        });
    assert_eq!(attrs.len(), 1);
    assert!(!attrs[0].from_schema);
}

/// A `fixed` unit inherited through a vacuous extension must still be
/// supplied — the GML measure-family shape.
#[test]
fn an_inherited_fixed_attribute_is_supplied() {
    let s = schema(
        r#"<xs:complexType name="Metres">
             <xs:simpleContent><xs:extension base="xs:double">
               <xs:attribute name="uom" type="xs:string" fixed="m"/>
             </xs:extension></xs:simpleContent>
           </xs:complexType>
           <xs:complexType name="Depth">
             <xs:simpleContent><xs:extension base="tns:Metres"/></xs:simpleContent>
           </xs:complexType>
           <xs:element name="depth" type="tns:Depth"/>"#,
    );
    let mut attrs = Vec::new();
    let report =
        s.instance_validator()
            .validate_with(r#"<depth xmlns="urn:example">120.5</depth>"#, |ev| {
                if let PsviEvent::StartElement { attributes, .. } = ev {
                    attrs = attributes;
                }
            });
    assert!(report.is_valid(), "{}", report.diagnostics);
    assert_eq!(
        attrs.len(),
        1,
        "the inherited fixed uom must survive derivation"
    );
    assert_eq!(attrs[0].lexical, "m");
}

/// A prohibited attribute is never supplied, even if the base gave it a value.
#[test]
fn a_prohibited_attribute_is_not_supplied() {
    let s = schema(
        r#"<xs:complexType name="Metres">
             <xs:simpleContent><xs:extension base="xs:double">
               <xs:attribute name="uom" type="xs:string" fixed="m"/>
             </xs:extension></xs:simpleContent>
           </xs:complexType>
           <xs:complexType name="Unitless">
             <xs:simpleContent><xs:restriction base="tns:Metres">
               <xs:attribute name="uom" use="prohibited"/>
             </xs:restriction></xs:simpleContent>
           </xs:complexType>
           <xs:element name="ratio" type="tns:Unitless"/>"#,
    );
    let mut attrs = Vec::new();
    s.instance_validator()
        .validate_with(r#"<ratio xmlns="urn:example">0.5</ratio>"#, |ev| {
            if let PsviEvent::StartElement { attributes, .. } = ev {
                attrs = attributes;
            }
        });
    assert!(attrs.is_empty(), "a prohibited attribute is not supplied");
}

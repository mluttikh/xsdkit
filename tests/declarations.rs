//! `default` and `fixed` values, checked against the type that has to accept
//! them.
//!
//! A schema that supplies a value its own type rejects is broken whether or
//! not anyone ever writes a document against it — the value reaches the PSVI
//! of every instance where the attribute is absent.

use xsdkit::diagnostics::DiagCode;
use xsdkit::*;

const NS: &str = "urn:example";

fn schema(body: &str) -> String {
    format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    )
}

fn diags_v(body: &str, version: Version) -> Diagnostics {
    SchemaSetBuilder::new()
        .version(version)
        .text(schema(body), "mem://main.xsd")
        .compile()
        .diagnostics
}

fn count(body: &str, code: DiagCode) -> usize {
    diags_v(body, Version::Xsd11)
        .errors()
        .filter(|d| d.code == code)
        .count()
}

fn clean(body: &str) {
    let d = diags_v(body, Version::Xsd11);
    assert!(!d.has_errors(), "expected a clean build, got:\n{d}");
}

#[test]
fn a_default_must_be_valid_against_its_type() {
    assert_eq!(
        count(
            r#"<xs:element name="e">
                 <xs:complexType>
                   <xs:attribute name="n" type="xs:int" default="not-a-number"/>
                 </xs:complexType>
               </xs:element>"#,
            DiagCode::InvalidValueConstraint
        ),
        1
    );
    // Facets count too, not just the lexical space — this is the whole point
    // of pinning a unit with `fixed`.
    assert_eq!(
        count(
            r#"<xs:simpleType name="Unit">
                 <xs:restriction base="xs:string">
                   <xs:enumeration value="m"/><xs:enumeration value="ft"/>
                 </xs:restriction>
               </xs:simpleType>
               <xs:element name="e">
                 <xs:complexType>
                   <xs:attribute name="unit" type="tns:Unit" fixed="feets"/>
                 </xs:complexType>
               </xs:element>"#,
            DiagCode::InvalidValueConstraint
        ),
        1
    );
    clean(
        r#"<xs:simpleType name="Unit">
             <xs:restriction base="xs:string">
               <xs:enumeration value="m"/><xs:enumeration value="ft"/>
             </xs:restriction>
           </xs:simpleType>
           <xs:element name="e">
             <xs:complexType>
               <xs:attribute name="unit" type="tns:Unit" fixed="m"/>
               <xs:attribute name="n" type="xs:int" default="0"/>
             </xs:complexType>
           </xs:element>"#,
    );
}

#[test]
fn an_element_default_is_checked_against_its_content() {
    assert_eq!(
        count(
            r#"<xs:element name="e" type="xs:date" default="2023-02-29"/>"#,
            DiagCode::InvalidValueConstraint
        ),
        1,
        "2023 is not a leap year"
    );
    // Simple content on a complex type is still character content.
    assert_eq!(
        count(
            r#"<xs:element name="e" default="nope">
                 <xs:complexType>
                   <xs:simpleContent>
                     <xs:extension base="xs:int">
                       <xs:attribute name="u" type="xs:string"/>
                     </xs:extension>
                   </xs:simpleContent>
                 </xs:complexType>
               </xs:element>"#,
            DiagCode::InvalidValueConstraint
        ),
        1
    );
    clean(r#"<xs:element name="e" type="xs:date" default="2024-02-29"/>"#);
}

/// A value constraint supplies *character* content, and an element-only
/// content model has nowhere to put it.
#[test]
fn element_only_content_cannot_carry_a_default() {
    assert_eq!(
        count(
            r#"<xs:element name="e" default="x">
                 <xs:complexType>
                   <xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>
                 </xs:complexType>
               </xs:element>"#,
            DiagCode::InvalidValueConstraint
        ),
        1
    );
    // Mixed content can hold characters, so it is left alone.
    clean(
        r#"<xs:element name="e" default="x">
             <xs:complexType mixed="true">
               <xs:sequence>
                 <xs:element name="a" type="xs:int" minOccurs="0"/>
               </xs:sequence>
             </xs:complexType>
           </xs:element>"#,
    );
}

/// 1.0 forbids a value constraint on an ID outright — a schema supplying the
/// same ID into every instance supplies a duplicate the moment the element
/// appears twice. 1.1 dropped the rule and lets ID uniqueness catch it when it
/// actually happens.
#[test]
fn an_id_may_not_be_supplied_by_the_schema_in_xsd10() {
    let body = r#"<xs:element name="e">
                    <xs:complexType>
                      <xs:attribute name="id" type="xs:ID" default="fixed-id"/>
                    </xs:complexType>
                  </xs:element>"#;
    assert_eq!(
        diags_v(body, Version::Xsd10)
            .errors()
            .filter(|d| d.code == DiagCode::InvalidValueConstraint)
            .count(),
        1
    );
    assert!(!diags_v(body, Version::Xsd11).has_errors());
}

/// A QName's prefix resolves against the document that wrote it, which the
/// model does not keep — so the value goes unchecked rather than guessed at.
#[test]
fn a_qname_default_is_left_alone() {
    clean(
        r#"<xs:element name="e">
             <xs:complexType>
               <xs:attribute name="q" type="xs:QName" default="tns:whatever"/>
             </xs:complexType>
           </xs:element>"#,
    );
}

/// An enumeration on a list names whole lists. Comparing one means parsing it
/// against the *item* type; comparing it as a string rejects every value,
/// which is what used to happen.
#[test]
fn an_enumeration_on_a_list_matches_a_whole_list() {
    clean(
        r#"<xs:simpleType name="Pair">
             <xs:restriction base="xs:NMTOKENS">
               <xs:enumeration value="asd qwe"/>
               <xs:enumeration value="one two three"/>
             </xs:restriction>
           </xs:simpleType>
           <xs:element name="e">
             <xs:complexType>
               <xs:attribute name="p" type="tns:Pair" default="asd qwe"/>
             </xs:complexType>
           </xs:element>"#,
    );
    assert_eq!(
        count(
            r#"<xs:simpleType name="Pair">
                 <xs:restriction base="xs:NMTOKENS">
                   <xs:enumeration value="asd qwe"/>
                 </xs:restriction>
               </xs:simpleType>
               <xs:element name="e">
                 <xs:complexType>
                   <xs:attribute name="p" type="tns:Pair" default="asd zzz"/>
                 </xs:complexType>
               </xs:element>"#,
            DiagCode::InvalidValueConstraint
        ),
        1
    );
}

// ---------------------------------------------------------------------------
// Where an annotation may sit
// ---------------------------------------------------------------------------

/// The schema for schemas gives almost every component the same content model:
/// an optional annotation, then the rest. So one, and it comes first.
#[test]
fn an_annotation_must_be_first_and_alone() {
    assert_eq!(
        count(
            r#"<xs:element name="people">
                 <xs:complexType>
                   <xs:sequence><xs:element name="person" type="xs:string"/></xs:sequence>
                 </xs:complexType>
                 <xs:unique name="u">
                   <xs:annotation><xs:documentation>one</xs:documentation></xs:annotation>
                   <xs:annotation><xs:documentation>two</xs:documentation></xs:annotation>
                   <xs:selector xpath="./person"/>
                   <xs:field xpath="."/>
                 </xs:unique>
               </xs:element>"#,
            DiagCode::MisplacedAnnotation
        ),
        1,
        "the second annotation"
    );
    assert_eq!(
        count(
            r#"<xs:element name="people">
                 <xs:complexType>
                   <xs:sequence><xs:element name="person" type="xs:string"/></xs:sequence>
                 </xs:complexType>
                 <xs:unique name="u">
                   <xs:selector xpath="./person"/>
                   <xs:field xpath="."/>
                   <xs:annotation><xs:documentation>after</xs:documentation></xs:annotation>
                 </xs:unique>
               </xs:element>"#,
            DiagCode::MisplacedAnnotation
        ),
        1,
        "an annotation after the thing it documents"
    );
}

/// `xs:schema` interleaves annotations with the declarations they document,
/// and `xs:redefine` and `xs:override` do the same with what they revise.
#[test]
fn the_containers_that_interleave_annotations_are_exempt() {
    clean(
        r#"<xs:annotation><xs:documentation>first</xs:documentation></xs:annotation>
           <xs:element name="a" type="xs:string"/>
           <xs:annotation><xs:documentation>between</xs:documentation></xs:annotation>
           <xs:element name="b" type="xs:string">
             <xs:annotation><xs:documentation>ok, first child</xs:documentation></xs:annotation>
           </xs:element>
           <xs:annotation><xs:documentation>last</xs:documentation></xs:annotation>"#,
    );
}

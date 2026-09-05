//! Schema Representation Constraints: the rules answerable from the document
//! alone, before anything is resolved.
//!
//! These are about the XML the schema for schemas describes rather than about
//! the components it produces, which is why a typo in `block` belongs here —
//! a keyword nobody recognises is a constraint that quietly does nothing.

use xsdkit::diagnostics::DiagCode;
use xsdkit::*;

const NS: &str = "urn:example";

fn diags(body: &str) -> Diagnostics {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .version(Version::Xsd11)
        .text(xsd, "mem://main.xsd")
        .build_with_warnings()
        .1
}

fn count(body: &str, code: DiagCode) -> usize {
    diags(body).errors().filter(|d| d.code == code).count()
}

fn clean(body: &str) {
    let d = diags(body);
    assert!(!d.has_errors(), "expected a clean build, got:\n{d}");
}

#[test]
fn block_and_final_only_name_derivation_methods() {
    for bad in [
        r#"<xs:element name="e" type="xs:string" block=" #illegalValue"/>"#,
        r#"<xs:element name="e" type="xs:string" final="nonsense"/>"#,
        r#"<xs:complexType name="T" block="extension oops"><xs:sequence/></xs:complexType>"#,
    ] {
        assert_eq!(count(bad, DiagCode::InvalidAttributeValue), 1, "{bad}");
    }
    // `#all` is the whole value or none of it.
    assert_eq!(
        count(
            r##"<xs:element name="e" type="xs:string" block="#all extension"/>"##,
            DiagCode::InvalidAttributeValue
        ),
        1
    );
}

#[test]
fn the_real_derivation_keywords_are_accepted() {
    clean(
        r##"<xs:element name="a" type="xs:string" block="#all"/>
           <xs:element name="b" type="xs:string" block="extension restriction substitution"/>
           <xs:simpleType name="S" final="list union restriction">
             <xs:restriction base="xs:string"/>
           </xs:simpleType>
           <xs:complexType name="T" final="extension restriction"><xs:sequence/></xs:complexType>"##,
    );
}

/// `default` may be overridden by the document and `fixed` may not, so a
/// declaration cannot mean both.
#[test]
fn a_declaration_cannot_have_both_default_and_fixed() {
    assert_eq!(
        count(
            r#"<xs:element name="e">
                 <xs:complexType>
                   <xs:attribute name="n" type="xs:int" default="1" fixed="1"/>
                 </xs:complexType>
               </xs:element>"#,
            DiagCode::InvalidValueConstraint
        ),
        1
    );
    assert_eq!(
        count(
            r#"<xs:element name="e" type="xs:int" default="1" fixed="1"/>"#,
            DiagCode::InvalidValueConstraint
        ),
        1
    );
    clean(r#"<xs:element name="e" type="xs:int" fixed="1"/>"#);
}

/// A named model group *is* its one model group. Two of them name no single
/// content model, and nothing downstream would look at the second.
#[test]
fn a_group_definition_holds_exactly_one_model_group() {
    assert_eq!(
        count(
            r#"<xs:group name="G">
                 <xs:all><xs:element name="c" type="xs:int"/></xs:all>
                 <xs:all><xs:element name="d" type="xs:date"/></xs:all>
               </xs:group>"#,
            DiagCode::ConflictingTypeDefinition
        ),
        1
    );
    assert_eq!(
        count(
            r#"<xs:group name="G">
                 <xs:annotation><xs:documentation>empty</xs:documentation></xs:annotation>
               </xs:group>"#,
            DiagCode::ConflictingTypeDefinition
        ),
        1
    );
    clean(
        r#"<xs:group name="G">
             <xs:annotation><xs:documentation>fine</xs:documentation></xs:annotation>
             <xs:sequence><xs:element name="c" type="xs:int"/></xs:sequence>
           </xs:group>
           <xs:complexType name="T"><xs:group ref="tns:G"/></xs:complexType>"#,
    );
}

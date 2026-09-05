//! `final`: the sealing rule.
//!
//! A type's `final` names the derivations it refuses to be the base of. It is
//! how an author says "this is the last word", and ignoring it turns a
//! deliberate seal into decoration.

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

fn blocked(body: &str) -> usize {
    diags(body)
        .errors()
        .filter(|d| d.code == DiagCode::DerivationBlocked)
        .count()
}

fn clean(body: &str) {
    let d = diags(body);
    assert!(!d.has_errors(), "expected a clean build, got:\n{d}");
}

#[test]
fn a_final_type_cannot_be_extended_or_restricted() {
    assert_eq!(
        blocked(
            r#"<xs:complexType name="Base" final="extension">
                 <xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>
               </xs:complexType>
               <xs:complexType name="Derived">
                 <xs:complexContent>
                   <xs:extension base="tns:Base">
                     <xs:sequence><xs:element name="b" type="xs:int"/></xs:sequence>
                   </xs:extension>
                 </xs:complexContent>
               </xs:complexType>"#
        ),
        1
    );
    assert_eq!(
        blocked(
            r#"<xs:simpleType name="Base" final="restriction">
                 <xs:restriction base="xs:string"/>
               </xs:simpleType>
               <xs:simpleType name="Derived">
                 <xs:restriction base="tns:Base"><xs:maxLength value="3"/></xs:restriction>
               </xs:simpleType>"#
        ),
        1
    );
}

/// The seal is per method, so sealing one leaves the other open.
#[test]
fn final_seals_only_the_method_it_names() {
    clean(
        r#"<xs:complexType name="Base" final="extension">
             <xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="Derived">
             <xs:complexContent>
               <xs:restriction base="tns:Base">
                 <xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>
               </xs:restriction>
             </xs:complexContent>
           </xs:complexType>"#,
    );
}

/// A simple type can seal itself against being an item type or a member type,
/// which the atomic rules never reach.
#[test]
fn a_simple_type_can_refuse_to_be_listed_or_unioned() {
    assert_eq!(
        blocked(
            r#"<xs:simpleType name="Atom" final="list">
                 <xs:restriction base="xs:string"/>
               </xs:simpleType>
               <xs:simpleType name="L"><xs:list itemType="tns:Atom"/></xs:simpleType>"#
        ),
        1
    );
    assert_eq!(
        blocked(
            r#"<xs:simpleType name="A" final="union">
                 <xs:restriction base="xs:string"/>
               </xs:simpleType>
               <xs:simpleType name="B"><xs:restriction base="xs:int"/></xs:simpleType>
               <xs:simpleType name="U">
                 <xs:union memberTypes="tns:A tns:B"/>
               </xs:simpleType>"#
        ),
        1,
        "only the member that sealed itself"
    );
    clean(
        r#"<xs:simpleType name="Atom" final="union">
             <xs:restriction base="xs:string"/>
           </xs:simpleType>
           <xs:simpleType name="L"><xs:list itemType="tns:Atom"/></xs:simpleType>"#,
    );
}

/// `#all` seals every method at once, and `finalDefault` applies it to every
/// type in the document that does not say otherwise.
#[test]
fn hash_all_and_final_default_both_seal() {
    assert_eq!(
        blocked(
            r##"<xs:simpleType name="Base" final="#all">
                  <xs:restriction base="xs:string"/>
                </xs:simpleType>
                <xs:simpleType name="D">
                  <xs:restriction base="tns:Base"><xs:maxLength value="3"/></xs:restriction>
                </xs:simpleType>"##
        ),
        1
    );

    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}" finalDefault="restriction">
             <xs:simpleType name="Base"><xs:restriction base="xs:string"/></xs:simpleType>
             <xs:simpleType name="D">
               <xs:restriction base="tns:Base"><xs:maxLength value="3"/></xs:restriction>
             </xs:simpleType>
           </xs:schema>"#
    );
    let d = SchemaSetBuilder::new()
        .text(xsd, "mem://main.xsd")
        .build_with_warnings()
        .1;
    assert_eq!(
        d.errors()
            .filter(|x| x.code == DiagCode::DerivationBlocked)
            .count(),
        1
    );
}

/// The built-ins seal nothing, so the ordinary schema keeps loading.
#[test]
fn deriving_from_a_builtin_is_never_blocked() {
    clean(
        r#"<xs:simpleType name="Code">
             <xs:restriction base="xs:string"><xs:maxLength value="4"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="Codes"><xs:list itemType="tns:Code"/></xs:simpleType>
           <xs:complexType name="T">
             <xs:simpleContent>
               <xs:extension base="tns:Code">
                 <xs:attribute name="u" type="xs:string"/>
               </xs:extension>
             </xs:simpleContent>
           </xs:complexType>"#,
    );
}

//! `xs:key`, `xs:keyref` and `xs:unique`.
//!
//! The XPath these take is not XPath: XSD Part 1 Appendix I defines a
//! deliberately tiny subset — an optional `.//`, child steps, and an
//! attribute as a field's last step — so that a validator needs no engine.
//! These tests pin the subset and the value-space equality keys compare by.

use xsdkit::diagnostics::DiagCode;
use xsdkit::*;

const NS: &str = "urn:example";

fn schema(body: &str) -> Schemas {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .text(xsd, "mem://idc.xsd")
        .compile()
        .into_result()
        .unwrap_or_else(|d| panic!("{d}"))
}

fn valid(s: &Schemas, xml: &str) {
    let d = s.document_validator().validate(xml).diagnostics;
    assert!(!d.has_errors(), "expected valid, got:\n{d}");
}

fn invalid(s: &Schemas, xml: &str, code: DiagCode) {
    let d = s.document_validator().validate(xml).diagnostics;
    assert!(
        d.errors().any(|e| e.code == code),
        "expected {code}, got:\n{d}"
    );
}

/// A catalogue with all three kinds of constraint over the same elements.
fn catalogue() -> Schemas {
    schema(
        r#"<xs:element name="cat">
             <xs:complexType><xs:sequence>
               <xs:element name="item" maxOccurs="unbounded">
                 <xs:complexType>
                   <xs:sequence><xs:element name="sku" type="xs:string"/></xs:sequence>
                   <xs:attribute name="id" type="xs:int"/>
                 </xs:complexType>
               </xs:element>
               <xs:element name="ref" minOccurs="0" maxOccurs="unbounded">
                 <xs:complexType><xs:attribute name="to" type="xs:int"/></xs:complexType>
               </xs:element>
             </xs:sequence></xs:complexType>
             <xs:key name="k">
               <xs:selector xpath="tns:item"/><xs:field xpath="@id"/>
             </xs:key>
             <xs:keyref name="r" refer="tns:k">
               <xs:selector xpath="tns:ref"/><xs:field xpath="@to"/>
             </xs:keyref>
             <xs:unique name="u">
               <xs:selector xpath=".//tns:item"/><xs:field xpath="tns:sku"/>
             </xs:unique>
           </xs:element>"#,
    )
}

const OK: &str = r#"<cat xmlns="urn:example">
    <item id="1"><sku>a</sku></item>
    <item id="2"><sku>b</sku></item>
    <ref to="1"/></cat>"#;

#[test]
fn a_document_satisfying_every_constraint_validates() {
    valid(&catalogue(), OK);
}

#[test]
fn a_key_must_be_unique_among_the_nodes_it_covers() {
    invalid(
        &catalogue(),
        r#"<cat xmlns="urn:example">
             <item id="1"><sku>a</sku></item>
             <item id="1"><sku>b</sku></item></cat>"#,
        DiagCode::DuplicateKey,
    );
}

/// `xs:unique` and `xs:key` differ in one place: a key requires every field
/// to be present, and a unique simply has nothing to say about a node that
/// lacks one.
#[test]
fn a_key_requires_its_fields_and_a_unique_does_not() {
    invalid(
        &catalogue(),
        r#"<cat xmlns="urn:example"><item><sku>a</sku></item></cat>"#,
        DiagCode::MissingKeyField,
    );
    let s = schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element name="n" maxOccurs="unbounded">
                 <xs:complexType><xs:attribute name="v" type="xs:string"/></xs:complexType>
               </xs:element>
             </xs:sequence></xs:complexType>
             <xs:unique name="u"><xs:selector xpath="tns:n"/><xs:field xpath="@v"/></xs:unique>
           </xs:element>"#,
    );
    // Two nodes without the field are not two equal keys.
    valid(&s, r#"<doc xmlns="urn:example"><n/><n/></doc>"#);
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><n v="x"/><n v="x"/></doc>"#,
        DiagCode::DuplicateKey,
    );
}

#[test]
fn a_keyref_must_match_a_key() {
    invalid(
        &catalogue(),
        r#"<cat xmlns="urn:example">
             <item id="1"><sku>a</sku></item>
             <ref to="9"/></cat>"#,
        DiagCode::UnresolvedKeyRef,
    );
}

/// A `unique` over `.//item` reaches items at any depth, which is the only
/// wildcard over depth the subset has.
#[test]
fn a_descendant_selector_reaches_any_depth() {
    let s = schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element name="box" maxOccurs="unbounded">
                 <xs:complexType><xs:sequence>
                   <xs:element name="item" maxOccurs="unbounded">
                     <xs:complexType><xs:attribute name="id" type="xs:string"/></xs:complexType>
                   </xs:element>
                 </xs:sequence></xs:complexType>
               </xs:element>
             </xs:sequence></xs:complexType>
             <xs:unique name="u">
               <xs:selector xpath=".//tns:item"/><xs:field xpath="@id"/>
             </xs:unique>
           </xs:element>"#,
    );
    valid(
        &s,
        r#"<doc xmlns="urn:example"><box><item id="a"/></box><box><item id="b"/></box></doc>"#,
    );
    // Two boxes, one duplicate between them.
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><box><item id="a"/></box><box><item id="a"/></box></doc>"#,
        DiagCode::DuplicateKey,
    );
}

/// Keys compare in the *value* space. Two spellings of one instant are one
/// key, which text comparison could not see.
#[test]
fn keys_compare_as_values_not_as_text() {
    let s = schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element name="target" type="xs:time"/>
               <xs:element name="equiv" type="xs:time" maxOccurs="unbounded"/>
             </xs:sequence></xs:complexType>
             <xs:key name="t"><xs:selector xpath="tns:target"/><xs:field xpath="."/></xs:key>
             <xs:keyref name="r" refer="tns:t">
               <xs:selector xpath="tns:equiv"/><xs:field xpath="."/>
             </xs:keyref>
           </xs:element>"#,
    );
    valid(
        &s,
        r#"<doc xmlns="urn:example">
             <target>02:00:00-05:00</target>
             <equiv>07:00:00Z</equiv></doc>"#,
    );
}

/// A one-item list and the value it holds are the same key, which is what
/// lets a list-valued `keyref` refer to an atomic key.
#[test]
fn a_singleton_list_equals_the_value_it_holds() {
    let s = schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element name="para" maxOccurs="unbounded">
                 <xs:complexType>
                   <xs:attribute name="key" type="xs:Name" use="required"/>
                   <xs:attribute name="ref" type="tns:Names"/>
                 </xs:complexType>
               </xs:element>
             </xs:sequence></xs:complexType>
             <xs:key name="k"><xs:selector xpath="tns:para"/><xs:field xpath="@key"/></xs:key>
             <xs:keyref name="r" refer="tns:k">
               <xs:selector xpath="tns:para"/><xs:field xpath="@ref"/>
             </xs:keyref>
           </xs:element>
           <xs:simpleType name="Names"><xs:list itemType="xs:Name"/></xs:simpleType>"#,
    );
    valid(
        &s,
        r#"<doc xmlns="urn:example">
             <para key="alpha"/><para key="beta" ref="alpha"/></doc>"#,
    );
}

/// An unprefixed name in one of these paths is in *no* namespace, whatever
/// the default declaration says — which is why XSD 1.1 added
/// `xpathDefaultNamespace` to override it.
#[test]
fn a_path_does_not_use_the_default_namespace() {
    let s = schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element name="item" maxOccurs="unbounded">
                 <xs:complexType><xs:attribute name="id" type="xs:string"/></xs:complexType>
               </xs:element>
             </xs:sequence></xs:complexType>
             <xs:unique name="u">
               <xs:selector xpath="tns:item"/><xs:field xpath="@id"/>
             </xs:unique>
           </xs:element>"#,
    );
    // The prefixed selector matches, so the duplicate is caught.
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><item id="x"/><item id="x"/></doc>"#,
        DiagCode::DuplicateKey,
    );

    // The same selector written unprefixed names an element in *no*
    // namespace, which this document has none of, so it selects nothing and
    // the duplicate goes unnoticed.
    let bare = schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element name="item" maxOccurs="unbounded">
                 <xs:complexType><xs:attribute name="id" type="xs:string"/></xs:complexType>
               </xs:element>
             </xs:sequence></xs:complexType>
             <xs:unique name="u">
               <xs:selector xpath="item"/><xs:field xpath="@id"/>
             </xs:unique>
           </xs:element>"#,
    );
    valid(
        &bare,
        r#"<doc xmlns="urn:example"><item id="x"/><item id="x"/></doc>"#,
    );
}

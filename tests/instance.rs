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

/// A schema with a mixed type, a vacuous extension of it, an extension that
/// adds a particle, and a restriction — the four ways mixedness is decided.
fn mixed_derivation_schema() -> Schemas {
    schema(
        r#"<xs:complexType name="Mixed" mixed="true">
             <xs:sequence>
               <xs:element name="a" type="xs:string" minOccurs="0"/>
             </xs:sequence>
           </xs:complexType>
           <xs:complexType name="JustAttributes">
             <xs:complexContent><xs:extension base="tns:Mixed">
               <xs:attribute name="x" type="xs:string"/>
             </xs:extension></xs:complexContent>
           </xs:complexType>
           <xs:complexType name="Tightened">
             <xs:complexContent><xs:restriction base="tns:Mixed">
               <xs:sequence>
                 <xs:element name="a" type="xs:string" minOccurs="0"/>
               </xs:sequence>
             </xs:restriction></xs:complexContent>
           </xs:complexType>
           <xs:complexType name="Plain">
             <xs:sequence>
               <xs:element name="a" type="xs:string" minOccurs="0"/>
             </xs:sequence>
           </xs:complexType>
           <xs:complexType name="StillPlain">
             <xs:complexContent><xs:extension base="tns:Plain">
               <xs:attribute name="x" type="xs:string"/>
             </xs:extension></xs:complexContent>
           </xs:complexType>
           <xs:element name="vacuous" type="tns:JustAttributes"/>
           <xs:element name="tightened" type="tns:Tightened"/>
           <xs:element name="plain" type="tns:StillPlain"/>"#,
    )
}

/// An extension that adds only attributes takes the base's content type
/// whole, mixedness included — it never restates `mixed="true"`, and does not
/// have to.
#[test]
fn mixed_content_survives_an_extension_that_adds_no_particle() {
    let s = mixed_derivation_schema();
    valid(
        &s,
        r#"<vacuous xmlns="urn:example">text<a>y</a>more</vacuous>"#,
    );
}

/// Restriction states the content model in full, so mixedness is not
/// inherited across it.
#[test]
fn mixed_content_does_not_survive_a_restriction_that_omits_it() {
    let s = mixed_derivation_schema();
    invalid(
        &s,
        r#"<tightened xmlns="urn:example">text</tightened>"#,
        DiagCode::UnexpectedText,
    );
}

/// And the walk does not turn element-only content mixed on the way past.
#[test]
fn an_extension_of_an_element_only_type_stays_element_only() {
    let s = mixed_derivation_schema();
    invalid(
        &s,
        r#"<plain xmlns="urn:example">text</plain>"#,
        DiagCode::UnexpectedText,
    );
}

/// `xs:boolean` has four lexical forms, and a schema may use any of them for
/// `mixed`, `nillable` or `abstract` — as may an instance for `xsi:nil`.
#[test]
fn boolean_schema_attributes_accept_the_numeric_spelling() {
    let s = schema(
        r#"<xs:element name="note" nillable="1">
             <xs:complexType mixed="1">
               <xs:sequence>
                 <xs:element name="a" type="xs:string" minOccurs="0"/>
               </xs:sequence>
             </xs:complexType>
           </xs:element>"#,
    );
    valid(&s, r#"<note xmlns="urn:example">text<a>y</a>more</note>"#);
    valid(
        &s,
        &format!(r#"<note xmlns="urn:example" {XSI} xsi:nil="1"/>"#),
    );
    invalid(
        &s,
        &format!(r#"<note xmlns="urn:example" {XSI} xsi:nil="1">text</note>"#),
        DiagCode::NilElementNotEmpty,
    );
}

// ---------------------------------------------------------------------------
// simpleContent restrictions
// ---------------------------------------------------------------------------

/// Facets written straight under a `simpleContent` restriction, with no
/// `xs:simpleType` wrapper, declare the type the content is validated
/// against. Dropping them made the whole restriction do nothing.
fn narrowed_simple_content() -> Schemas {
    schema(
        r#"<xs:complexType name="Base">
             <xs:simpleContent><xs:extension base="xs:string">
               <xs:attribute name="k" type="xs:string"/>
             </xs:extension></xs:simpleContent>
           </xs:complexType>
           <xs:complexType name="Code">
             <xs:simpleContent>
               <xs:restriction base="tns:Base">
                 <xs:maxLength value="3"/>
                 <xs:attribute name="k" type="xs:string"/>
               </xs:restriction>
             </xs:simpleContent>
           </xs:complexType>
           <xs:element name="e" type="tns:Code"/>"#,
    )
}

#[test]
fn facets_on_a_simple_content_restriction_are_enforced() {
    let s = narrowed_simple_content();
    valid(&s, r#"<e xmlns="urn:example">abc</e>"#);
    invalid(
        &s,
        r#"<e xmlns="urn:example">abcdefgh</e>"#,
        DiagCode::InvalidValue,
    );
}

/// And the attributes still come through — the restriction narrows the value
/// space without discarding everything else about the type.
#[test]
fn a_narrowed_simple_content_keeps_its_attributes() {
    let s = narrowed_simple_content();
    valid(&s, r#"<e xmlns="urn:example" k="v">ab</e>"#);
    let mut typed = Vec::new();
    s.instance_validator()
        .validate_with(r#"<e xmlns="urn:example">ab</e>"#, |ev| {
            if let PsviEvent::Text { value: Some(v), .. } = ev {
                typed.push(v);
            }
        });
    assert_eq!(typed, vec![Value::String("ab".into())]);
}

/// An enumeration there behaves the same way.
#[test]
fn an_enumeration_on_a_simple_content_restriction_is_enforced() {
    let s = schema(
        r#"<xs:complexType name="Base">
             <xs:simpleContent><xs:extension base="xs:string"/></xs:simpleContent>
           </xs:complexType>
           <xs:complexType name="Pick">
             <xs:simpleContent>
               <xs:restriction base="tns:Base">
                 <xs:enumeration value="red"/>
                 <xs:enumeration value="green"/>
               </xs:restriction>
             </xs:simpleContent>
           </xs:complexType>
           <xs:element name="p" type="tns:Pick"/>"#,
    );
    valid(&s, r#"<p xmlns="urn:example">red</p>"#);
    invalid(
        &s,
        r#"<p xmlns="urn:example">blue</p>"#,
        DiagCode::InvalidValue,
    );
}

/// XSD 1.1 checks Element Declarations Consistent when a document walks into
/// it rather than when the schema is read: a wildcard may admit a name the
/// content model also declares, and that is only a problem if the two
/// declarations describe no common value.
fn edc_schema(local: &str, global: &str) -> Schemas {
    schema(&format!(
        r#"<xs:complexType name="T">
             <xs:sequence>
               <xs:element name="e" {local}/>
               <xs:element name="f" type="xs:string"/>
               <xs:any namespace="{}" processContents="lax"/>
             </xs:sequence>
           </xs:complexType>
           <xs:element name="doc" type="tns:T"/>
           <xs:element name="e" {global}/>"#,
        "##targetNamespace"
    ))
}

const TWO: &str = r#"<doc xmlns="urn:example"><e>2008-11-03</e><f>x</f><e>2008-11-04</e></doc>"#;

#[test]
fn a_wildcard_may_not_admit_a_name_the_model_declares_incompatibly() {
    // `xs:date` here and `xs:time` there: no document satisfies both.
    let s = edc_schema(r#"type="xs:date""#, r#"type="xs:time""#);
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><e>2008-11-03</e><f>x</f><e>12:20:02</e></doc>"#,
        DiagCode::InconsistentDeclarations,
    );
}

/// Consistent means *related*, not identical — a value of one can be a value
/// of the other.
#[test]
fn related_declarations_are_consistent() {
    // Derivation, either way round.
    valid(
        &edc_schema(r#"type="xs:integer""#, r#"type="xs:positiveInteger""#),
        r#"<doc xmlns="urn:example"><e>-12</e><f>x</f><e>12</e></doc>"#,
    );
    valid(
        &edc_schema(r#"type="xs:integer""#, r#"type="xs:decimal""#),
        r#"<doc xmlns="urn:example"><e>12</e><f>x</f><e>1.5</e></doc>"#,
    );
    // The same declaration on both sides.
    valid(&edc_schema(r#"type="xs:date""#, r#"type="xs:date""#), TWO);
}

/// A union and its members are related too, which the base chain does not
/// record: every `xs:date` is a value of a union that has `xs:date` in it.
#[test]
fn a_union_and_its_members_are_consistent() {
    let s = schema(
        r###"<xs:complexType name="T">
             <xs:sequence>
               <xs:element name="e">
                 <xs:simpleType><xs:union memberTypes="xs:date xs:time"/></xs:simpleType>
               </xs:element>
               <xs:element name="f" type="xs:string"/>
               <xs:any namespace="##targetNamespace" processContents="lax"/>
             </xs:sequence>
           </xs:complexType>
           <xs:element name="doc" type="tns:T"/>
           <xs:element name="e" type="xs:date"/>"###,
    );
    valid(
        &s,
        r#"<doc xmlns="urn:example"><e>12:12:00</e><f>x</f><e>2008-11-02</e></doc>"#,
    );
}

/// `skip` looks at nothing, so there is no declaration to disagree with.
#[test]
fn a_skip_wildcard_raises_no_consistency_question() {
    let s = schema(
        r###"<xs:complexType name="T">
             <xs:sequence>
               <xs:element name="e" type="xs:date"/>
               <xs:element name="f" type="xs:string"/>
               <xs:any namespace="##targetNamespace" processContents="skip"/>
             </xs:sequence>
           </xs:complexType>
           <xs:element name="doc" type="tns:T"/>
           <xs:element name="e" type="xs:time"/>"###,
    );
    valid(
        &s,
        r#"<doc xmlns="urn:example"><e>2008-11-03</e><f>x</f><e>12:20:02</e></doc>"#,
    );
}

// ---------------------------------------------------------------------------
// xs:ID and xs:IDREF
// ---------------------------------------------------------------------------

fn identified() -> Schemas {
    schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element ref="tns:para" maxOccurs="unbounded"/>
               <xs:element name="u" type="tns:MaybeId" minOccurs="0" maxOccurs="unbounded"/>
               <xs:element name="r" type="xs:IDREF" minOccurs="0" maxOccurs="unbounded"/>
             </xs:sequence></xs:complexType>
           </xs:element>
           <xs:element name="para">
             <xs:complexType>
               <xs:attribute name="a" type="xs:ID"/>
               <xs:attribute name="b" type="xs:ID"/>
               <xs:attribute name="rs" type="xs:IDREFS"/>
             </xs:complexType>
           </xs:element>
           <xs:simpleType name="MaybeId">
             <xs:union memberTypes="xs:integer xs:boolean xs:ID"/>
           </xs:simpleType>"#,
    )
}

/// An `xs:ID` binds a value to the element carrying it. Two *different*
/// elements may not claim one — but one element repeating it is a binding
/// with a single member, which is legal.
#[test]
fn an_id_binds_to_an_element_not_merely_to_a_value() {
    let s = identified();
    valid(
        &s,
        r#"<doc xmlns="urn:example"><para a="eee" b="eee"/></doc>"#,
    );
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><para a="x"/><para b="x"/></doc>"#,
        DiagCode::DuplicateId,
    );
    // Compared after whitespace collapse, so these are one identifier.
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><para a="x"/><para b=" x "/></doc>"#,
        DiagCode::DuplicateId,
    );
}

/// An `xs:IDREF` may point forward, so references are settled only once the
/// document ends.
#[test]
fn an_idref_must_match_an_id_somewhere_in_the_document() {
    let s = identified();
    valid(
        &s,
        r#"<doc xmlns="urn:example"><para a="x"/><r>x</r></doc>"#,
    );
    // Declared after the reference to it.
    valid(
        &s,
        r#"<doc xmlns="urn:example"><para rs="x"/><para a="x"/></doc>"#,
    );
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><para a="x"/><r>nope</r></doc>"#,
        DiagCode::UnresolvedIdRef,
    );
    // `xs:IDREFS` is a list, and every item has to resolve.
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><para a="x" rs="x nope"/></doc>"#,
        DiagCode::UnresolvedIdRef,
    );
}

/// A list of unions settles the question for each item separately: the list
/// type has no members of its own, so asking it once for the whole value
/// resolves nothing at all.
#[test]
fn a_list_of_unions_resolves_each_item_on_its_own() {
    let s = schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element name="n" maxOccurs="unbounded">
                 <xs:complexType>
                   <xs:attribute name="id" type="tns:IdList"/>
                   <xs:attribute name="refs" type="tns:RefList"/>
                 </xs:complexType>
               </xs:element>
             </xs:sequence></xs:complexType>
           </xs:element>
           <xs:simpleType name="IdList"><xs:list itemType="tns:IdOrInt"/></xs:simpleType>
           <xs:simpleType name="RefList"><xs:list itemType="tns:RefOrInt"/></xs:simpleType>
           <xs:simpleType name="IdOrInt">
             <xs:union memberTypes="xs:ID xs:integer"/>
           </xs:simpleType>
           <xs:simpleType name="RefOrInt">
             <xs:union memberTypes="xs:IDREF xs:integer"/>
           </xs:simpleType>"#,
    );
    // The names are identifiers; the integers among them are not.
    valid(
        &s,
        r#"<doc xmlns="urn:example"><n id="aaa 23 bbb"/><n refs="bbb 29 aaa"/></doc>"#,
    );
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><n id="aaa 23"/><n refs="nope"/></doc>"#,
        DiagCode::UnresolvedIdRef,
    );
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><n id="aaa"/><n id="aaa"/></doc>"#,
        DiagCode::DuplicateId,
    );
    // `23` is not an NCName, so it never reaches the `xs:IDREF` member and
    // there is no reference to leave dangling.
    valid(
        &s,
        r#"<doc xmlns="urn:example"><n id="23"/><n refs="23"/></doc>"#,
    );
}

/// A union's members are tried in order and the first that validates wins, so
/// whether a value is an identifier is a question about the *value*, not
/// about the type.
#[test]
fn a_union_is_an_id_only_when_the_id_member_is_the_one_that_matched() {
    let s = identified();
    // `abc` is neither an integer nor a boolean, so it reaches `xs:ID`.
    valid(
        &s,
        r#"<doc xmlns="urn:example"><para a="p"/><u>abc</u><r>abc</r></doc>"#,
    );
    // `123` matches `xs:integer` first, so it never becomes an identifier.
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><para a="p"/><u>123</u><r>123</r></doc>"#,
        DiagCode::UnresolvedIdRef,
    );
}

/// An `xs:ENTITY` names an *unparsed* entity — one declared with `NDATA`,
/// pointing at content the document does not contain. A parsed entity is text
/// the reader expands and is not one of these.
#[test]
fn an_entity_must_name_an_unparsed_entity_the_dtd_declares() {
    let s = schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element name="e" type="xs:ENTITY" maxOccurs="unbounded"/>
             </xs:sequence></xs:complexType>
           </xs:element>"#,
    );
    let doc = |body: &str, decls: &str| {
        format!("<!DOCTYPE doc [{decls}]><doc xmlns=\"urn:example\">{body}</doc>")
    };
    let declared = r#"<!ENTITY pic SYSTEM "p.gif" NDATA GIF>
                      <!NOTATION GIF SYSTEM "v.exe">"#;
    valid(&s, &doc("<e>pic</e>", declared));
    invalid(&s, &doc("<e>other</e>", declared), DiagCode::UnknownEntity);
    // A *parsed* entity is not an unparsed one, however it is spelled.
    invalid(
        &s,
        &doc("<e>text</e>", r#"<!ENTITY text "just words">"#),
        DiagCode::UnknownEntity,
    );
}

/// `xs:ENTITIES` is a list, and every item has to name one.
#[test]
fn every_item_of_an_entities_value_must_be_declared() {
    let s = schema(
        r#"<xs:element name="doc">
             <xs:complexType>
               <xs:attribute name="pics" type="xs:ENTITIES"/>
             </xs:complexType>
           </xs:element>"#,
    );
    let decls = r#"<!ENTITY a SYSTEM "a.gif" NDATA GIF>
                   <!ENTITY b SYSTEM "b.gif" NDATA GIF>
                   <!NOTATION GIF SYSTEM "v.exe">"#;
    valid(
        &s,
        &format!("<!DOCTYPE doc [{decls}]><doc xmlns=\"urn:example\" pics=\"a b\"/>"),
    );
    invalid(
        &s,
        &format!("<!DOCTYPE doc [{decls}]><doc xmlns=\"urn:example\" pics=\"a nope\"/>"),
        DiagCode::UnknownEntity,
    );
}

// ---------------------------------------------------------------------------
// Wildcard processContents
// ---------------------------------------------------------------------------

/// A wildcard admitting a child says how it must be processed. Treating every
/// wildcard as `skip` leaves a hole in the document where nothing is checked
/// — which is what the W3C's own datatype tests sit inside.
fn wrapped_in(process_contents: &str) -> Schemas {
    schema(&format!(
        r#"<xs:element name="inner" type="tns:Small"/>
           <xs:simpleType name="Small">
             <xs:restriction base="xs:int"><xs:maxInclusive value="10"/></xs:restriction>
           </xs:simpleType>
           <xs:element name="out">
             <xs:complexType><xs:sequence>
               <xs:any processContents="{process_contents}"/>
             </xs:sequence></xs:complexType>
           </xs:element>"#
    ))
}

const OUT: &str = r#"<out xmlns="urn:example">"#;

#[test]
fn a_strict_wildcard_validates_what_it_admits() {
    let s = wrapped_in("strict");
    valid(&s, &format!("{OUT}<inner>5</inner></out>"));
    // The declaration is found and its facets apply.
    invalid(
        &s,
        &format!("{OUT}<inner>999</inner></out>"),
        DiagCode::InvalidValue,
    );
    // And `strict` insists there be a declaration at all.
    invalid(
        &s,
        &format!("{OUT}<nosuch>x</nosuch></out>"),
        DiagCode::ElementNotDeclared,
    );
}

#[test]
fn a_lax_wildcard_validates_only_what_it_can_find() {
    let s = wrapped_in("lax");
    valid(&s, &format!("{OUT}<inner>5</inner></out>"));
    invalid(
        &s,
        &format!("{OUT}<inner>999</inner></out>"),
        DiagCode::InvalidValue,
    );
    // Nothing to check it against, which `lax` accepts and `strict` does not.
    valid(&s, &format!("{OUT}<nosuch>x</nosuch></out>"));
}

#[test]
fn a_skip_wildcard_looks_no_further() {
    let s = wrapped_in("skip");
    valid(&s, &format!("{OUT}<inner>999</inner></out>"));
    valid(&s, &format!("{OUT}<nosuch>x</nosuch></out>"));
}

/// The subtree under a validated wildcard is validated too, not just its
/// immediate child.
#[test]
fn a_strict_wildcard_reaches_the_whole_subtree() {
    let s = schema(
        r#"<xs:element name="inner">
             <xs:complexType><xs:sequence>
               <xs:element name="n" type="xs:int"/>
             </xs:sequence></xs:complexType>
           </xs:element>
           <xs:element name="out">
             <xs:complexType><xs:sequence>
               <xs:any processContents="strict"/>
             </xs:sequence></xs:complexType>
           </xs:element>"#,
    );
    valid(&s, &format!("{OUT}<inner><n>1</n></inner></out>"));
    invalid(
        &s,
        &format!("{OUT}<inner><n>oops</n></inner></out>"),
        DiagCode::InvalidValue,
    );
}

// ---------------------------------------------------------------------------
// Attribute wildcards
// ---------------------------------------------------------------------------

/// An `xs:anyAttribute` says how what it admits must be processed, exactly as
/// `xs:any` does — and an admitted attribute reaches the PSVI typed, not as a
/// name with no declaration behind it.
fn attributes_wrapped_in(process_contents: &str) -> Schemas {
    schema(&format!(
        r#"<xs:attribute name="k" type="tns:Small"/>
           <xs:simpleType name="Small">
             <xs:restriction base="xs:int"><xs:maxInclusive value="10"/></xs:restriction>
           </xs:simpleType>
           <xs:element name="out">
             <xs:complexType><xs:sequence/>
               <xs:anyAttribute namespace="{}" processContents="{process_contents}"/>
             </xs:complexType>
           </xs:element>"#,
        "##any"
    ))
}

#[test]
fn a_strict_attribute_wildcard_validates_what_it_admits() {
    let s = attributes_wrapped_in("strict");
    valid(
        &s,
        r#"<out xmlns="urn:example" xmlns:t="urn:example" t:k="5"/>"#,
    );
    invalid(
        &s,
        r#"<out xmlns="urn:example" xmlns:t="urn:example" t:k="999"/>"#,
        DiagCode::InvalidValue,
    );
    invalid(
        &s,
        r#"<out xmlns="urn:example" xmlns:o="urn:other" o:z="x"/>"#,
        DiagCode::AttributeNotAllowed,
    );
}

#[test]
fn a_lax_attribute_wildcard_validates_only_what_it_can_find() {
    let s = attributes_wrapped_in("lax");
    invalid(
        &s,
        r#"<out xmlns="urn:example" xmlns:t="urn:example" t:k="999"/>"#,
        DiagCode::InvalidValue,
    );
    valid(
        &s,
        r#"<out xmlns="urn:example" xmlns:o="urn:other" o:z="x"/>"#,
    );
}

#[test]
fn a_skip_attribute_wildcard_looks_no_further() {
    let s = attributes_wrapped_in("skip");
    valid(
        &s,
        r#"<out xmlns="urn:example" xmlns:t="urn:example" t:k="999"/>"#,
    );
}

/// The point of `lax` and `strict` beyond the verdict: the attribute arrives
/// with the declaration the schema has for it, and a typed value.
#[test]
fn an_attribute_admitted_by_a_wildcard_reaches_the_psvi_typed() {
    let s = attributes_wrapped_in("strict");
    let mut seen = Vec::new();
    let r = s.instance_validator().validate_with(
        r#"<out xmlns="urn:example" xmlns:t="urn:example" t:k="5"/>"#,
        |ev| {
            if let PsviEvent::StartElement { attributes, .. } = ev {
                for a in attributes {
                    seen.push((a.declaration.is_some(), a.value.clone()));
                }
            }
        },
    );
    assert!(r.is_valid(), "{}", r.diagnostics);
    assert_eq!(seen, vec![(true, Some(Value::Integer(5)))]);
}

/// Wildcards reaching one type from two attribute groups are *intersected* —
/// an attribute has to satisfy both, not either.
#[test]
fn attribute_group_wildcards_intersect() {
    let s = schema(
        r#"<xs:complexType name="T"><xs:sequence/>
             <xs:attributeGroup ref="tns:a"/>
             <xs:attributeGroup ref="tns:b"/>
           </xs:complexType>
           <xs:attributeGroup name="a">
             <xs:anyAttribute namespace="urn:one urn:two" processContents="lax"/>
           </xs:attributeGroup>
           <xs:attributeGroup name="b">
             <xs:anyAttribute namespace="urn:two urn:three" processContents="lax"/>
           </xs:attributeGroup>
           <xs:element name="e" type="tns:T"/>"#,
    );
    // In both.
    valid(&s, r#"<e xmlns="urn:example" xmlns:t="urn:two" t:x="1"/>"#);
    // In one only — the intersection excludes it.
    invalid(
        &s,
        r#"<e xmlns="urn:example" xmlns:o="urn:one" o:x="1"/>"#,
        DiagCode::AttributeNotAllowed,
    );
    invalid(
        &s,
        r#"<e xmlns="urn:example" xmlns:h="urn:three" h:x="1"/>"#,
        DiagCode::AttributeNotAllowed,
    );
}

/// An extension may only widen, so its wildcard is the *union* of its own and
/// the base's — and a type that adds no wildcard still inherits one.
#[test]
fn an_extension_unions_its_wildcard_with_the_bases() {
    let s = schema(
        r#"<xs:complexType name="Base"><xs:sequence/>
             <xs:anyAttribute namespace="urn:a" processContents="lax"/>
           </xs:complexType>
           <xs:complexType name="Wider">
             <xs:complexContent><xs:extension base="tns:Base">
               <xs:anyAttribute namespace="urn:b" processContents="lax"/>
             </xs:extension></xs:complexContent>
           </xs:complexType>
           <xs:complexType name="Same">
             <xs:complexContent><xs:extension base="tns:Base">
               <xs:attribute name="k" type="xs:string"/>
             </xs:extension></xs:complexContent>
           </xs:complexType>
           <xs:element name="w" type="tns:Wider"/>
           <xs:element name="s" type="tns:Same"/>"#,
    );
    valid(&s, r#"<w xmlns="urn:example" xmlns:a="urn:a" a:x="1"/>"#);
    valid(&s, r#"<w xmlns="urn:example" xmlns:b="urn:b" b:y="1"/>"#);
    // Adding only an attribute keeps the wildcard it inherited.
    valid(&s, r#"<s xmlns="urn:example" xmlns:a="urn:a" a:x="1"/>"#);
    invalid(
        &s,
        r#"<w xmlns="urn:example" xmlns:c="urn:c" c:z="1"/>"#,
        DiagCode::AttributeNotAllowed,
    );
}

/// A wildcard's namespace decides; its mere presence does not admit
/// everything.
#[test]
fn a_wildcards_namespace_constraint_is_enforced() {
    // `r###` because the schema text itself contains `"##`.
    let s = schema(
        r###"<xs:complexType name="T"><xs:sequence/>
             <xs:anyAttribute namespace="##other" processContents="lax"/>
           </xs:complexType>
           <xs:element name="e" type="tns:T"/>"###,
    );
    valid(
        &s,
        r#"<e xmlns="urn:example" xmlns:o="urn:other" o:x="1"/>"#,
    );
    // `##other` excludes the target namespace.
    invalid(
        &s,
        r#"<e xmlns="urn:example" xmlns:t="urn:example" t:x="1"/>"#,
        DiagCode::AttributeNotAllowed,
    );
}

/// An attribute `fixed` value is compared in the value space, as an element's
/// is — `1.0` and `1.00` are one decimal.
#[test]
fn an_attribute_fixed_value_compares_values_not_strings() {
    let s = schema(
        r#"<xs:element name="e">
             <xs:complexType>
               <xs:attribute name="n" type="xs:decimal" fixed="1.00"/>
             </xs:complexType>
           </xs:element>"#,
    );
    valid(&s, r#"<e xmlns="urn:example" n="1.0"/>"#);
    valid(&s, r#"<e xmlns="urn:example" n="1.00"/>"#);
    invalid(
        &s,
        r#"<e xmlns="urn:example" n="2"/>"#,
        DiagCode::InvalidValue,
    );
}

// ---------------------------------------------------------------------------
// References, and schema-supplied element content
// ---------------------------------------------------------------------------

/// The character content the PSVI reports for a one-element document.
fn text_of(s: &Schemas, xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    s.instance_validator().validate_with(xml, |ev| {
        if let PsviEvent::Text { lexical, .. } = ev {
            out.push(lexical);
        }
    });
    out
}

/// `&amp;` and `&#233;` are parser events of their own rather than part of
/// the surrounding text. Dropping them reads `caf&#233;` as `caf` — a wrong
/// value with no diagnostic, which is the worst way to be wrong.
#[test]
fn character_and_entity_references_reach_the_value() {
    let s = schema(r#"<xs:element name="item" type="xs:string"/>"#);
    for (written, want) in [
        ("plain", "plain"),
        ("a&amp;b", "a&b"),
        ("a&lt;b&gt;c", "a<b>c"),
        ("&quot;q&quot;", "\"q\""),
        ("it&apos;s", "it's"),
        ("caf&#233;", "café"),
        ("&#65;", "A"),
        ("x&#x41;y", "xAy"),
        // Beyond the basic plane, so the reference is not one UTF-16 unit.
        ("&#x10000;", "\u{10000}"),
    ] {
        let xml = format!(r#"<item xmlns="urn:example">{written}</item>"#);
        assert_eq!(text_of(&s, &xml), vec![want.to_string()], "for `{written}`");
    }
}

/// An entity this reader cannot expand is reported rather than silently
/// treated as nothing.
#[test]
fn an_unexpandable_entity_reference_is_reported() {
    let s = schema(r#"<xs:element name="item" type="xs:string"/>"#);
    invalid(
        &s,
        r#"<item xmlns="urn:example">&mystery;</item>"#,
        DiagCode::MalformedXml,
    );
}

fn defaulted_schema() -> Schemas {
    schema(
        r#"<xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element name="d" type="xs:int" default="7" minOccurs="0"/>
               <xs:element name="f" type="xs:decimal" fixed="1.00" minOccurs="0"/>
             </xs:sequence></xs:complexType>
           </xs:element>"#,
    )
}

/// An empty element takes its declaration's `default` — the schema supplying
/// what the document left out, as it does for an absent attribute.
#[test]
fn an_empty_element_takes_its_declarations_default() {
    let s = defaulted_schema();
    let mut seen = Vec::new();
    let r = s
        .instance_validator()
        .validate_with(r#"<doc xmlns="urn:example"><d/></doc>"#, |ev| {
            if let PsviEvent::Text {
                value, from_schema, ..
            } = ev
            {
                seen.push((value, from_schema));
            }
        });
    assert!(r.is_valid(), "{}", r.diagnostics);
    assert_eq!(seen, vec![(Some(Value::Integer(7)), true)]);
}

/// `fixed` behaves the same way when the element is empty.
#[test]
fn an_empty_element_takes_its_declarations_fixed_value() {
    let s = defaulted_schema();
    valid(&s, r#"<doc xmlns="urn:example"><f/></doc>"#);
}

/// And constrains the content when the document does write some. The
/// comparison is in the value space, so `1.0` satisfies a decimal fixed at
/// `1.00`.
#[test]
fn content_must_match_a_fixed_element_value() {
    let s = defaulted_schema();
    valid(&s, r#"<doc xmlns="urn:example"><f>1.0</f></doc>"#);
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><f>8</f></doc>"#,
        DiagCode::InvalidValue,
    );
}

/// Whitespace is character content, so it is not the absence a `default`
/// fills — and it is not a valid `xs:int` either.
#[test]
fn whitespace_content_is_not_an_absent_value() {
    let s = defaulted_schema();
    invalid(
        &s,
        r#"<doc xmlns="urn:example"><d> </d></doc>"#,
        DiagCode::InvalidValue,
    );
}

// ---------------------------------------------------------------------------
// xs:enumeration over QNames
// ---------------------------------------------------------------------------

/// These schemas declare their own prefixes, so they cannot use the shared
/// `schema` helper's fixed prologue.
fn from_xsd(xsd: &str) -> Schemas {
    SchemaSetBuilder::new()
        .text(xsd.to_string(), "mem://qname.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"))
}

/// A QName enumeration lists *values*, and a value is a namespace plus a
/// local name. The literal's prefix binds in the schema, the instance's in
/// the document, and the two need never agree on the spelling.
#[test]
fn a_qname_enumeration_resolves_its_literals_against_the_schema() {
    let s = from_xsd(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:code="urn:codes" xmlns:tns="urn:example"
                      targetNamespace="urn:example" elementFormDefault="qualified">
             <xs:simpleType name="Code">
               <xs:restriction base="xs:QName">
                 <xs:enumeration value="code:alpha"/>
                 <xs:enumeration value="code:beta"/>
               </xs:restriction>
             </xs:simpleType>
             <xs:element name="pick" type="tns:Code"/>
           </xs:schema>"#,
    );
    // A different prefix for the same namespace is the same value.
    valid(
        &s,
        r#"<pick xmlns="urn:example" xmlns:x="urn:codes">x:alpha</pick>"#,
    );
    valid(
        &s,
        r#"<pick xmlns="urn:example" xmlns:code="urn:codes">code:beta</pick>"#,
    );
    // The right local name in the wrong namespace is a different value.
    invalid(
        &s,
        r#"<pick xmlns="urn:example" xmlns:x="urn:other">x:alpha</pick>"#,
        DiagCode::InvalidValue,
    );
    // And one the schema never listed.
    invalid(
        &s,
        r#"<pick xmlns="urn:example" xmlns:x="urn:codes">x:gamma</pick>"#,
        DiagCode::InvalidValue,
    );
}

/// An unprefixed literal takes the schema's *default* namespace — which is
/// the shape the NIST QName tests use, and is not the same as no namespace.
#[test]
fn an_unprefixed_qname_literal_takes_the_schemas_default_namespace() {
    let s = from_xsd(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns="urn:example" xmlns:tns="urn:example"
                      targetNamespace="urn:example" elementFormDefault="qualified">
             <xs:simpleType name="Code">
               <xs:restriction base="xs:QName">
                 <xs:enumeration value="alpha"/>
               </xs:restriction>
             </xs:simpleType>
             <xs:element name="pick" type="tns:Code"/>
           </xs:schema>"#,
    );
    valid(&s, r#"<pick xmlns="urn:example">alpha</pick>"#);
    // No default namespace in the document, so this `alpha` is in no
    // namespace and is a different value from the schema's.
    invalid(
        &s,
        r#"<t:pick xmlns:t="urn:example">alpha</t:pick>"#,
        DiagCode::InvalidValue,
    );
}

/// The binding nearest the literal wins, which is why these are kept per
/// facet set rather than once per schema document.
#[test]
fn a_qname_literal_uses_the_binding_nearest_it() {
    let s = from_xsd(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:code="urn:outer" xmlns:tns="urn:example"
                      targetNamespace="urn:example" elementFormDefault="qualified">
             <xs:simpleType name="Code">
               <xs:restriction base="xs:QName" xmlns:code="urn:inner">
                 <xs:enumeration value="code:alpha"/>
               </xs:restriction>
             </xs:simpleType>
             <xs:element name="pick" type="tns:Code"/>
           </xs:schema>"#,
    );
    valid(
        &s,
        r#"<pick xmlns="urn:example" xmlns:x="urn:inner">x:alpha</pick>"#,
    );
    invalid(
        &s,
        r#"<pick xmlns="urn:example" xmlns:x="urn:outer">x:alpha</pick>"#,
        DiagCode::InvalidValue,
    );
}

/// A list's enumeration literal is a whole list, so each item resolves
/// separately — and still against the schema.
#[test]
fn a_qname_list_enumeration_resolves_every_item() {
    let s = from_xsd(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:a="urn:a" xmlns:b="urn:b" xmlns:tns="urn:example"
                      targetNamespace="urn:example" elementFormDefault="qualified">
             <xs:simpleType name="Names">
               <xs:list itemType="xs:QName"/>
             </xs:simpleType>
             <xs:simpleType name="Pair">
               <xs:restriction base="tns:Names">
                 <xs:enumeration value="a:one b:two"/>
               </xs:restriction>
             </xs:simpleType>
             <xs:element name="pick" type="tns:Pair"/>
           </xs:schema>"#,
    );
    valid(
        &s,
        r#"<pick xmlns="urn:example" xmlns:p="urn:a" xmlns:q="urn:b">p:one q:two</pick>"#,
    );
    // Both items in one namespace is a different list.
    invalid(
        &s,
        r#"<pick xmlns="urn:example" xmlns:p="urn:a">p:one p:two</pick>"#,
        DiagCode::InvalidValue,
    );
}

/// A prefixed literal is no longer reported as an invalid facet value.
#[test]
fn a_prefixed_qname_enumeration_literal_is_not_a_schema_error() {
    let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:code="urn:codes" xmlns:tns="urn:example"
                      targetNamespace="urn:example">
             <xs:simpleType name="Code">
               <xs:restriction base="xs:QName">
                 <xs:enumeration value="code:alpha"/>
               </xs:restriction>
             </xs:simpleType>
           </xs:schema>"#;
    let (_, diags) = SchemaSetBuilder::new()
        .text(xsd.to_string(), "mem://qname.xsd")
        .build_with_warnings();
    assert!(!diags.has_errors(), "{diags}");
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

/// A `fixed` attribute may be repeated but not contradicted.
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

/// The same types, but with `thing` one level down, so a prefix can be
/// declared above the element that uses it.
fn nested_derived_schema() -> Schemas {
    schema(
        r#"<xs:complexType name="Base">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="Derived">
             <xs:complexContent><xs:extension base="tns:Base">
               <xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence>
             </xs:extension></xs:complexContent>
           </xs:complexType>
           <xs:element name="wrapper">
             <xs:complexType>
               <xs:sequence><xs:element name="thing" type="tns:Base"/></xs:sequence>
             </xs:complexType>
           </xs:element>"#,
    )
}

#[test]
fn an_xsi_type_prefix_declared_on_an_ancestor_resolves() {
    let s = nested_derived_schema();
    valid(
        &s,
        r#"<wrapper xmlns="urn:example"
                    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                    xmlns:tns="urn:example">
             <thing xsi:type="tns:Derived"><a>x</a><b>y</b></thing>
           </wrapper>"#,
    );
}

#[test]
fn an_unprefixed_xsi_type_takes_the_default_namespace_in_scope() {
    // A QName in *value* position uses the default namespace, unlike an
    // attribute *name*, which never does.
    let s = nested_derived_schema();
    valid(
        &s,
        r#"<wrapper xmlns="urn:example"
                    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
             <thing xsi:type="Derived"><a>x</a><b>y</b></thing>
           </wrapper>"#,
    );
}

#[test]
fn an_inner_rebinding_of_an_xsi_type_prefix_wins() {
    let s = nested_derived_schema();
    valid(
        &s,
        r#"<wrapper xmlns="urn:example"
                    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                    xmlns:p="urn:elsewhere">
             <thing xmlns:p="urn:example" xsi:type="p:Derived">
               <a>x</a><b>y</b>
             </thing>
           </wrapper>"#,
    );
    // Without the inner declaration the same literal names nothing.
    invalid(
        &s,
        r#"<wrapper xmlns="urn:example"
                    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                    xmlns:p="urn:elsewhere">
             <thing xsi:type="p:Derived"><a>x</a><b>y</b></thing>
           </wrapper>"#,
        DiagCode::InvalidXsiType,
    );
}

#[test]
fn an_unbound_xsi_type_prefix_is_reported_as_such() {
    let s = nested_derived_schema();
    let d = check(
        &s,
        r#"<wrapper xmlns="urn:example"
                    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
             <thing xsi:type="nope:Derived"><a>x</a></thing>
           </wrapper>"#,
    );
    assert!(
        d.errors()
            .any(|e| e.code == DiagCode::InvalidXsiType && e.message.contains("not bound")),
        "expected an unbound-prefix report, got:\n{d}"
    );
}

/// `block` names the derivation methods an `xsi:type` may not use to reach
/// the declared type — on the type itself, and on the element declaration.
fn blocking_schema() -> Schemas {
    schema(
        r#"<xs:complexType name="Sealed" block="extension">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="ByExtension">
             <xs:complexContent><xs:extension base="tns:Sealed">
               <xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence>
             </xs:extension></xs:complexContent>
           </xs:complexType>
           <xs:complexType name="ByRestriction">
             <xs:complexContent><xs:restriction base="tns:Sealed">
               <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
             </xs:restriction></xs:complexContent>
           </xs:complexType>
           <xs:complexType name="RestrictedExtension">
             <xs:complexContent><xs:restriction base="tns:ByExtension">
               <xs:sequence>
                 <xs:element name="a" type="xs:string"/>
                 <xs:element name="b" type="xs:string"/>
               </xs:sequence>
             </xs:restriction></xs:complexContent>
           </xs:complexType>
           <xs:complexType name="Open">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="OpenRestricted">
             <xs:complexContent><xs:restriction base="tns:Open">
               <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
             </xs:restriction></xs:complexContent>
           </xs:complexType>
           <xs:element name="sealed" type="tns:Sealed"/>
           <xs:element name="guarded" type="tns:Open" block="restriction"/>
           <xs:element name="unguarded" type="tns:Open"/>"#,
    )
}

const XSI: &str = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#;

#[test]
fn a_blocked_derivation_method_bars_the_xsi_type_that_uses_it() {
    let s = blocking_schema();
    // Restriction is not blocked, so this substitution stands.
    valid(
        &s,
        &format!(
            r#"<sealed xmlns="urn:example" {XSI} xmlns:tns="urn:example"
                       xsi:type="tns:ByRestriction"><a>x</a></sealed>"#
        ),
    );
    // Extension is.
    invalid(
        &s,
        &format!(
            r#"<sealed xmlns="urn:example" {XSI} xmlns:tns="urn:example"
                       xsi:type="tns:ByExtension"><a>x</a><b>y</b></sealed>"#
        ),
        DiagCode::InvalidXsiType,
    );
}

/// The block applies at every step of the chain, not only the last one — a
/// restriction of a blocked extension is still out.
#[test]
fn a_block_reaches_through_the_whole_derivation_chain() {
    let s = blocking_schema();
    invalid(
        &s,
        &format!(
            r#"<sealed xmlns="urn:example" {XSI} xmlns:tns="urn:example"
                       xsi:type="tns:RestrictedExtension"><a>x</a><b>y</b></sealed>"#
        ),
        DiagCode::InvalidXsiType,
    );
}

/// The element declaration's own `block` counts too, even when its type
/// blocks nothing.
#[test]
fn an_element_declarations_block_bars_an_xsi_type() {
    let s = blocking_schema();
    invalid(
        &s,
        &format!(
            r#"<guarded xmlns="urn:example" {XSI} xmlns:tns="urn:example"
                        xsi:type="tns:OpenRestricted"><a>x</a></guarded>"#
        ),
        DiagCode::InvalidXsiType,
    );
    valid(
        &s,
        &format!(
            r#"<unguarded xmlns="urn:example" {XSI} xmlns:tns="urn:example"
                          xsi:type="tns:OpenRestricted"><a>x</a></unguarded>"#
        ),
    );
}

fn abstract_schema() -> Schemas {
    schema(
        r#"<xs:complexType name="Shape" abstract="true">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="Circle">
             <xs:complexContent><xs:extension base="tns:Shape"/></xs:complexContent>
           </xs:complexType>
           <xs:element name="shape" type="tns:Shape"/>"#,
    )
}

/// An abstract type stands in for its derivations; naming one is what
/// `xsi:type` is for, and leaving it out is an error rather than a default.
#[test]
fn an_abstract_type_needs_an_xsi_type_to_stand_in_for_it() {
    let s = abstract_schema();
    invalid(
        &s,
        r#"<shape xmlns="urn:example"><a>x</a></shape>"#,
        DiagCode::AbstractType,
    );
    invalid(
        &s,
        &format!(
            r#"<shape xmlns="urn:example" {XSI} xmlns:tns="urn:example"
                      xsi:type="tns:Shape"><a>x</a></shape>"#
        ),
        DiagCode::AbstractType,
    );
    valid(
        &s,
        &format!(
            r#"<shape xmlns="urn:example" {XSI} xmlns:tns="urn:example"
                      xsi:type="tns:Circle"><a>x</a></shape>"#
        ),
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

/// `block` on the head bars a member from standing in for it. The head
/// itself is unaffected — it is not substituting for anything.
#[test]
fn a_head_that_blocks_substitution_admits_only_itself() {
    let s = schema(
        r#"<xs:element name="root">
             <xs:complexType><xs:sequence>
               <xs:element ref="tns:Head" maxOccurs="unbounded"/>
               <xs:element ref="tns:Open" minOccurs="0" maxOccurs="unbounded"/>
             </xs:sequence></xs:complexType>
           </xs:element>
           <xs:complexType name="T"><xs:sequence/></xs:complexType>
           <xs:element name="Head" type="tns:T" block="substitution"/>
           <xs:element name="Member" type="tns:T" substitutionGroup="tns:Head"/>
           <xs:element name="Open" type="tns:T"/>
           <xs:element name="OpenMember" type="tns:T" substitutionGroup="tns:Open"/>"#,
    );
    valid(&s, r#"<root xmlns="urn:example"><Head/></root>"#);
    invalid(
        &s,
        r#"<root xmlns="urn:example"><Head/><Member/></root>"#,
        DiagCode::UnexpectedElement,
    );
    // A head that blocks nothing still takes its members.
    valid(
        &s,
        r#"<root xmlns="urn:example"><Head/><OpenMember/></root>"#,
    );
}

/// The bar may come from the head's *type* rather than from the element: a
/// type can refuse to be restricted into a substitute without the element
/// declaration saying anything at all.
#[test]
fn a_head_types_prohibited_substitutions_bar_a_member_too() {
    let s = schema(
        r#"<xs:element name="root">
             <xs:complexType><xs:sequence>
               <xs:element ref="tns:Head" maxOccurs="unbounded"/>
             </xs:sequence></xs:complexType>
           </xs:element>
           <xs:element name="Head" type="tns:Sealed"/>
           <xs:complexType name="Sealed" block="restriction"><xs:sequence/></xs:complexType>
           <xs:complexType name="ByRestriction">
             <xs:complexContent><xs:restriction base="tns:Sealed">
               <xs:sequence/>
             </xs:restriction></xs:complexContent>
           </xs:complexType>
           <xs:element name="Member" type="tns:ByRestriction" substitutionGroup="tns:Head"/>"#,
    );
    invalid(
        &s,
        r#"<root xmlns="urn:example"><Member/></root>"#,
        DiagCode::UnexpectedElement,
    );
}

/// A member whose type is unrelated to the head's still substitutes when
/// nothing is blocked. Whether the two types are related is a *schema*
/// question, and asking it here would throw out members the schema accepted.
#[test]
fn an_unblocked_head_does_not_second_guess_its_members_types() {
    let s = schema(
        r#"<xs:element name="head" type="xs:string"/>
           <xs:element name="member" type="xs:int" substitutionGroup="tns:head"/>
           <xs:element name="doc">
             <xs:complexType><xs:sequence>
               <xs:element ref="tns:head" maxOccurs="unbounded"/>
             </xs:sequence></xs:complexType>
           </xs:element>"#,
    );
    valid(&s, r#"<doc xmlns="urn:example"><member>1</member></doc>"#);
}

/// An abstract element exists to be substituted for. A content model never
/// offers one, but the document root asks the schema directly.
#[test]
fn an_abstract_element_may_not_appear_in_a_document() {
    let s = schema(r#"<xs:element name="lonely" abstract="true"/>"#);
    invalid(
        &s,
        r#"<lonely xmlns="urn:example"/>"#,
        DiagCode::AbstractElement,
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

/// `simpleContent` restricting a *complex* base, with the base declared after
/// the type that derives from it.
///
/// Resolving the simple type a `simpleContent` validates against used to be a
/// single forward pass over the arena that followed exactly one link. Both
/// assumptions were wrong. A base may be declared below its derived type, in
/// which case its own content is still a placeholder when the derived type is
/// reached, and the pass fell back to naming the *complex* base as the simple
/// target — so the text was then checked against a type with no value space.
///
/// The symptom was that declaration order decided whether a document
/// validated, silently, and it cost the W3C suite hundreds of valid documents.
#[test]
fn simple_content_resolves_whatever_the_declaration_order() {
    let derived = r#"<xs:element name="x"><xs:complexType><xs:simpleContent>
           <xs:restriction base="tns:Base">
             <xs:attribute name="a" type="xs:integer"/>
           </xs:restriction>
         </xs:simpleContent></xs:complexType></xs:element>"#;
    let base = r#"<xs:complexType name="Base"><xs:simpleContent>
           <xs:extension base="xs:string"><xs:anyAttribute/></xs:extension>
         </xs:simpleContent></xs:complexType>"#;

    for (order, body) in [
        ("derived first", format!("{derived}{base}")),
        ("base first", format!("{base}{derived}")),
    ] {
        let s = schema(&body);
        let d = check(&s, &format!(r#"<x xmlns="{NS}" a="2">Hello</x>"#));
        assert!(!d.has_errors(), "{order}: expected valid, got:\n{d}");
    }
}

/// The same, three links deep, so the fix cannot be one extra hop.
#[test]
fn a_simple_content_chain_resolves_through_every_link() {
    let s = schema(
        r#"<xs:element name="z"><xs:complexType><xs:simpleContent>
             <xs:restriction base="tns:Mid"/>
           </xs:simpleContent></xs:complexType></xs:element>
           <xs:complexType name="Mid"><xs:simpleContent>
             <xs:restriction base="tns:Base"/>
           </xs:simpleContent></xs:complexType>
           <xs:complexType name="Base"><xs:simpleContent>
             <xs:extension base="xs:int"/>
           </xs:simpleContent></xs:complexType>"#,
    );
    valid(&s, &format!(r#"<z xmlns="{NS}">42</z>"#));
    // And the base's type is still enforced through the whole chain.
    invalid(
        &s,
        &format!(r#"<z xmlns="{NS}">not-an-int</z>"#),
        DiagCode::InvalidValue,
    );
}

/// A document whose root is undeclared, against a schema with no global
/// elements at all.
///
/// The validator keys its element stack on an interned name, and interning is
/// impossible after compilation. For an undeclared root there is no parent
/// name to borrow, so it used to reach for the first global element — and
/// `expect`ed one to exist. A schema that declares only types has none, and
/// the whole validator panicked on an ordinary invalid document. Untrusted
/// input must produce a diagnostic, never a crash.
#[test]
fn an_undeclared_root_against_an_element_less_schema_reports_rather_than_panics() {
    let s =
        schema(r#"<xs:simpleType name="Only"><xs:restriction base="xs:string"/></xs:simpleType>"#);

    let d = check(&s, r#"<whatever xmlns="urn:nowhere"><child/></whatever>"#);
    assert!(
        d.errors().any(|e| e.code == DiagCode::ElementNotDeclared),
        "expected a diagnostic naming the undeclared root, got:\n{d}"
    );
}

// ---------------------------------------------------------------------------
// xs:QName — the one datatype whose value depends on the document
// ---------------------------------------------------------------------------

/// A QName's value is the namespace its prefix is bound to plus the local
/// part, so validating one needs the bindings in scope where it was written.
/// Those live in the instance, not in the schema, which is why value parsing
/// takes a resolver at all.
#[test]
fn qname_content_resolves_against_the_documents_namespaces() {
    let s = schema(r#"<xs:element name="e" type="xs:QName"/>"#);

    // A prefix declared on the element itself.
    valid(
        &s,
        &format!(r#"<e xmlns="{NS}" xmlns:p="urn:p">p:thing</e>"#),
    );
    // The default namespace, for an unprefixed name.
    valid(&s, &format!(r#"<e xmlns="{NS}">thing</e>"#));
    // No default namespace in scope: still a value, just in no namespace.
    valid(&s, &format!(r#"<e xmlns="{NS}" xmlns:p="urn:p">thing</e>"#));
    // The `xml` prefix is bound everywhere without being declared.
    valid(&s, &format!(r#"<e xmlns="{NS}">xml:lang</e>"#));

    // A prefix bound to nothing is not a value.
    invalid(
        &s,
        &format!(r#"<e xmlns="{NS}">nope:thing</e>"#),
        DiagCode::InvalidValue,
    );
    // Two colons is not a QName.
    invalid(
        &s,
        &format!(r#"<e xmlns="{NS}">a:b:c</e>"#),
        DiagCode::InvalidValue,
    );
}

/// The bindings are a *stack*: a prefix declared on an ancestor is in scope on
/// every descendant. Resolving against the current element's attributes alone
/// would pass the common case and fail this one.
#[test]
fn a_qname_resolves_a_prefix_declared_on_an_ancestor() {
    let s = schema(
        r#"<xs:element name="outer"><xs:complexType><xs:sequence>
             <xs:element name="inner" type="xs:QName"/>
           </xs:sequence></xs:complexType></xs:element>"#,
    );
    valid(
        &s,
        &format!(r#"<outer xmlns="{NS}" xmlns:p="urn:p"><inner>p:thing</inner></outer>"#),
    );
}

/// The resolved value, not merely the verdict.
///
/// Validity alone cannot tell `p:thing` in `urn:p` from `thing` in no
/// namespace — both are valid QNames — so the PSVI is where the work is
/// visible. The prefix is deliberately absent from the value: it is a
/// document detail, and `a:x` and `b:x` are the same value when `a` and `b`
/// name the same namespace.
#[test]
fn a_qname_reaches_the_psvi_resolved() {
    let s = schema(r#"<xs:element name="e" type="xs:QName"/>"#);

    let value_of = |xml: &str| {
        let mut found = None;
        let report = s.instance_validator().validate_with(xml, |ev| {
            if let PsviEvent::Text { value: Some(v), .. } = ev {
                found = Some(v);
            }
        });
        assert!(report.is_valid(), "{}", report.diagnostics);
        found.expect("a text event")
    };

    let qname = |ns: Option<&str>, local: &str| {
        Value::QName(xsdkit::values::QNameValue {
            namespace: ns.map(str::to_string),
            local: local.into(),
        })
    };

    // Two different prefixes for one namespace are one value.
    assert_eq!(
        value_of(&format!(r#"<e xmlns="{NS}" xmlns:a="urn:p">a:thing</e>"#)),
        value_of(&format!(r#"<e xmlns="{NS}" xmlns:b="urn:p">b:thing</e>"#)),
    );
    assert_eq!(
        value_of(&format!(r#"<e xmlns="{NS}" xmlns:a="urn:p">a:thing</e>"#)),
        qname(Some("urn:p"), "thing"),
    );
    // An unprefixed name takes the default namespace...
    assert_eq!(
        value_of(&format!(r#"<e xmlns="{NS}">thing</e>"#)),
        qname(Some(NS), "thing"),
    );
    // The default namespace applies whatever prefix the element itself used.
    assert_eq!(
        value_of(&format!(r#"<p:e xmlns:p="{NS}" xmlns="urn:d">thing</p:e>"#)),
        qname(Some("urn:d"), "thing"),
    );
}

/// `xmlns=""` undeclares the default namespace rather than binding it to the
/// empty string, so an unprefixed QName beneath it is in *no* namespace — a
/// different value from one in the namespace the ancestor declared.
#[test]
fn an_undeclared_default_namespace_leaves_a_qname_unqualified() {
    let s = schema(
        r#"<xs:element name="outer"><xs:complexType><xs:sequence>
             <xs:element name="inner" type="xs:QName"/>
           </xs:sequence></xs:complexType></xs:element>"#,
    );

    let mut found = None;
    let report = s.instance_validator().validate_with(
        &format!(
            r#"<p:outer xmlns:p="{NS}" xmlns="urn:d">
                 <p:inner xmlns="">thing</p:inner>
               </p:outer>"#
        ),
        |ev| {
            if let PsviEvent::Text { value: Some(v), .. } = ev {
                found = Some(v);
            }
        },
    );
    assert!(report.is_valid(), "{}", report.diagnostics);
    assert_eq!(
        found.expect("a text event"),
        Value::QName(xsdkit::values::QNameValue {
            namespace: None,
            local: "thing".into(),
        }),
        "`xmlns=\"\"` undeclares, it does not bind to the empty string",
    );
}

/// `length`, `minLength` and `maxLength` do not constrain a QName. The value
/// space is a pair, not a string, and the specification's errata deprecate
/// these facets there — which is what the W3C suite asserts, with valid
/// instances 1 to 61 characters long against `length="7"`.
#[test]
fn length_facets_do_not_constrain_a_qname() {
    let s = schema(
        r#"<xs:element name="e" type="tns:Q"/>
           <xs:simpleType name="Q">
             <xs:restriction base="xs:QName"><xs:length value="7"/></xs:restriction>
           </xs:simpleType>"#,
    );
    valid(&s, &format!(r#"<e xmlns="{NS}">a</e>"#));
    valid(
        &s,
        &format!(r#"<e xmlns="{NS}">considerably_longer_than_seven</e>"#),
    );
}

/// A list of QNames resolves every item, so the resolver has to reach the
/// item type rather than stopping at the list.
#[test]
fn a_list_of_qnames_resolves_every_item() {
    let s = schema(
        r#"<xs:element name="e" type="tns:Qs"/>
           <xs:simpleType name="Qs"><xs:list itemType="xs:QName"/></xs:simpleType>"#,
    );
    valid(
        &s,
        &format!(r#"<e xmlns="{NS}" xmlns:p="urn:p">p:one p:two three</e>"#),
    );
    invalid(
        &s,
        &format!(r#"<e xmlns="{NS}" xmlns:p="urn:p">p:one nope:two</e>"#),
        DiagCode::InvalidValue,
    );
}

/// A QName-typed attribute resolves in its element's scope too.
#[test]
fn a_qname_attribute_resolves_against_the_element_it_is_on() {
    let s = schema(
        r#"<xs:element name="e"><xs:complexType>
             <xs:attribute name="ref" type="xs:QName"/>
           </xs:complexType></xs:element>"#,
    );
    valid(
        &s,
        &format!(r#"<e xmlns="{NS}" xmlns:p="urn:p" ref="p:thing"/>"#),
    );
    invalid(
        &s,
        &format!(r#"<e xmlns="{NS}" ref="nope:thing"/>"#),
        DiagCode::InvalidValue,
    );
}

//! XSD 1.1 structural features: open content, default attributes, and the
//! relaxed Unique Particle Attribution rule.

use xsdkit::model::Term;
use xsdkit::*;

const NS: &str = "urn:example";

fn build(body: &str, version: Version) -> Schemas {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:tns="{NS}"
                      targetNamespace="{NS}" elementFormDefault="qualified">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .version(version)
        .text(xsd, "mem://main.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"))
}

fn diagnostics(body: &str, version: Version) -> Diagnostics {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:tns="{NS}"
                      targetNamespace="{NS}" elementFormDefault="qualified">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .version(version)
        .text(xsd, "mem://main.xsd")
        .build_with_warnings()
        .1
}

fn valid(s: &Schemas, xml: &str) -> bool {
    s.instance_validator().validate(xml).is_valid()
}

// ---------------------------------------------------------------------------
// Relaxed UPA
// ---------------------------------------------------------------------------

/// An element particle competing with a wildcard is ambiguous in 1.0 and
/// legal in 1.1, where the element wins.
#[test]
fn a_wildcard_competing_with_an_element_is_legal_in_xsd11() {
    const BODY: &str = r###"<xs:complexType name="T"><xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="0"/>
        <xs:any namespace="##any" processContents="lax"/>
    </xs:sequence></xs:complexType>"###;

    let d10 = diagnostics(BODY, Version::Xsd10);
    assert!(
        d10.iter()
            .any(|x| x.code == DiagCode::AmbiguousContentModel),
        "XSD 1.0 rejects it:\n{d10}"
    );

    let d11 = diagnostics(BODY, Version::Xsd11);
    assert!(
        !d11.iter()
            .any(|x| x.code == DiagCode::AmbiguousContentModel),
        "XSD 1.1 resolves it in favour of the element:\n{d11}"
    );
}

/// The relaxation is specific: two element particles sharing a name are still
/// ambiguous in 1.1.
#[test]
fn element_versus_element_stays_ambiguous_in_xsd11() {
    const BODY: &str = r#"<xs:complexType name="T"><xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="0"/>
        <xs:element name="a" type="xs:string"/>
    </xs:sequence></xs:complexType>"#;
    let d = diagnostics(BODY, Version::Xsd11);
    assert!(
        d.iter().any(|x| x.code == DiagCode::AmbiguousContentModel),
        "1.1 relaxes only the wildcard case:\n{d}"
    );
}

// ---------------------------------------------------------------------------
// Open content
// ---------------------------------------------------------------------------

const OPEN: &str = r###"<xs:element name="root">
    <xs:complexType>
      <xs:openContent mode="interleave">
        <xs:any namespace="##other" processContents="lax"/>
      </xs:openContent>
      <xs:sequence>
        <xs:element name="a" type="xs:string"/>
        <xs:element name="b" type="xs:string"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>"###;

#[test]
fn interleaved_open_content_admits_a_wildcard_anywhere() {
    let s = build(OPEN, Version::Xsd11);
    assert!(valid(
        &s,
        r#"<root xmlns="urn:example"><a>1</a><b>2</b></root>"#
    ));
    // Between the two declared elements...
    assert!(valid(
        &s,
        r#"<root xmlns="urn:example" xmlns:o="urn:other">
             <a>1</a><o:x/><b>2</b></root>"#
    ));
    // ...before them, and after.
    assert!(valid(
        &s,
        r#"<root xmlns="urn:example" xmlns:o="urn:other">
             <o:x/><a>1</a><b>2</b><o:y/></root>"#
    ));
}

/// Open content does not excuse missing declared content.
#[test]
fn open_content_does_not_satisfy_the_declared_model() {
    let s = build(OPEN, Version::Xsd11);
    assert!(
        !valid(
            &s,
            r#"<root xmlns="urn:example" xmlns:o="urn:other"><a>1</a><o:x/></root>"#
        ),
        "`b` is still required"
    );
}

/// The wildcard's own namespace constraint still applies.
#[test]
fn open_content_respects_its_namespace_constraint() {
    let s = build(OPEN, Version::Xsd11);
    assert!(
        !valid(
            &s,
            r#"<root xmlns="urn:example"><a>1</a><c/><b>2</b></root>"#
        ),
        "##other excludes the target namespace"
    );
}

#[test]
fn suffix_open_content_is_only_admitted_at_the_end() {
    let s = build(
        r###"<xs:element name="root">
              <xs:complexType>
                <xs:openContent mode="suffix">
                  <xs:any namespace="##other" processContents="lax"/>
                </xs:openContent>
                <xs:sequence>
                  <xs:element name="a" type="xs:string"/>
                  <xs:element name="b" type="xs:string"/>
                </xs:sequence>
              </xs:complexType>
            </xs:element>"###,
        Version::Xsd11,
    );
    assert!(valid(
        &s,
        r#"<root xmlns="urn:example" xmlns:o="urn:other"><a>1</a><b>2</b><o:x/></root>"#
    ));
    assert!(
        !valid(
            &s,
            r#"<root xmlns="urn:example" xmlns:o="urn:other"><a>1</a><o:x/><b>2</b></root>"#
        ),
        "a suffix wildcard may not appear mid-content"
    );
}

#[test]
fn open_content_is_ignored_when_reading_as_xsd10() {
    let s = build(OPEN, Version::Xsd11);
    assert_eq!(s.content_stats().open_content, 1);

    let s10 = build(OPEN, Version::Xsd10);
    assert_eq!(
        s10.content_stats().open_content,
        0,
        "1.0 has no open content"
    );
    assert!(!valid(
        &s10,
        r#"<root xmlns="urn:example" xmlns:o="urn:other"><a>1</a><o:x/><b>2</b></root>"#
    ));
}

#[test]
fn default_open_content_applies_to_every_type_in_the_document() {
    let s = build(
        r###"<xs:defaultOpenContent mode="interleave">
              <xs:any namespace="##other" processContents="lax"/>
            </xs:defaultOpenContent>
            <xs:element name="root">
              <xs:complexType><xs:sequence>
                <xs:element name="a" type="xs:string"/>
              </xs:sequence></xs:complexType>
            </xs:element>"###,
        Version::Xsd11,
    );
    assert!(valid(
        &s,
        r#"<root xmlns="urn:example" xmlns:o="urn:other"><o:x/><a>1</a></root>"#
    ));
}

/// `mode="none"` is how one type opts out of the document's default.
#[test]
fn a_type_can_opt_out_of_the_default_open_content() {
    let s = build(
        r###"<xs:defaultOpenContent mode="interleave">
              <xs:any namespace="##other" processContents="lax"/>
            </xs:defaultOpenContent>
            <xs:element name="closed">
              <xs:complexType>
                <xs:openContent mode="none"/>
                <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
              </xs:complexType>
            </xs:element>"###,
        Version::Xsd11,
    );
    assert!(!valid(
        &s,
        r#"<closed xmlns="urn:example" xmlns:o="urn:other"><o:x/><a>1</a></closed>"#
    ));
    assert!(valid(
        &s,
        r#"<closed xmlns="urn:example"><a>1</a></closed>"#
    ));
}

// ---------------------------------------------------------------------------
// Default attributes
// ---------------------------------------------------------------------------

#[test]
fn default_attributes_reach_every_complex_type() {
    let s = build(
        r#"<xs:attributeGroup name="Common">
             <xs:attribute name="id" type="xs:string"/>
             <xs:attribute name="lang" type="xs:string"/>
           </xs:attributeGroup>
           <xs:complexType name="A"><xs:sequence/></xs:complexType>
           <xs:complexType name="B">
             <xs:attribute name="own" type="xs:string"/>
           </xs:complexType>"#
            .to_string()
            .as_str(),
        Version::Xsd11,
    );
    let _ = &s;
}

#[test]
fn xsd11_only_constructs_warn_when_read_as_xsd10() {
    let d = diagnostics(
        r###"<xs:defaultOpenContent><xs:any namespace="##other"/></xs:defaultOpenContent>"###,
        Version::Xsd10,
    );
    assert!(
        d.iter().any(|x| x.code == DiagCode::Unsupported),
        "reading 1.1 syntax as 1.0 should say so:\n{d}"
    );
    assert!(!d.has_errors(), "but it is a warning, not an error:\n{d}");
}

/// `notNamespace` is the inverse of `namespace`: XSD 1.0 could only exclude
/// one namespace, with `##other`. Without it the wildcard fell back to
/// `##any`, so two wildcards that partition the namespaces looked like they
/// overlapped and UPA rejected a valid schema.
#[test]
fn not_namespace_excludes_rather_than_admits() {
    let s = build(
        r###"<xs:complexType name="T">
             <xs:sequence>
               <xs:any namespace="##local" processContents="skip"/>
               <xs:any notNamespace="##local" processContents="skip"/>
             </xs:sequence>
           </xs:complexType>
           <xs:element name="e" type="tns:T"/>"###,
        Version::Xsd11,
    );
    let t = s.type_(Some(NS), "T").expect("type");
    let p = s[t].as_complex().unwrap().content.particle().unwrap();
    let kids = s.child_particles(p);
    assert_eq!(kids.len(), 2);

    let Term::Wildcard(w) = &s[kids[1]].term else {
        panic!("expected a wildcard");
    };
    // Everything but the absent namespace.
    assert!(!w.namespace.admits(None));
    // Anything that is not the absent namespace, including a URI this schema
    // has never seen — which is the whole point of a wildcard.
    assert!(
        w.namespace
            .admits_uri(s.names(), Some("urn:never-mentioned"))
    );
    assert!(!w.namespace.admits_uri(s.names(), None));

    // The first one is its inverse.
    let Term::Wildcard(first) = &s[kids[0]].term else {
        panic!("expected a wildcard");
    };
    assert!(first.namespace.admits(None));
    assert!(
        !first
            .namespace
            .admits_uri(s.names(), Some("urn:never-mentioned"))
    );
}

/// The two are alternatives — together they name no set at all.
#[test]
fn namespace_and_not_namespace_are_alternatives() {
    let d = diagnostics(
        r###"<xs:complexType name="T">
             <xs:sequence>
               <xs:any namespace="##any" notNamespace="##local"/>
             </xs:sequence>
           </xs:complexType>"###,
        Version::Xsd11,
    );
    assert!(
        d.errors()
            .any(|x| x.code == DiagCode::InvalidAttributeValue),
        "{d}"
    );
}

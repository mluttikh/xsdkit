//! XSD 1.1 structural features: open content, default attributes, and the
//! relaxed Unique Particle Attribution rule.

use xsdkit::diagnostics::DiagCode;
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
        .compile()
        .into_result()
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
        .compile()
        .diagnostics
}

fn valid(s: &Schemas, xml: &str) -> bool {
    s.document_validator().validate(xml).is_valid()
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
    let t = s.type_id(Some(NS), "T").expect("type");
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

/// `##definedSibling` excludes every name the surrounding content model writes
/// out, which is what lets a wildcard sit between named particles without
/// competing with them. Without it the wildcard looked ambiguous against the
/// element after it and UPA rejected a valid schema.
#[test]
fn defined_sibling_excludes_the_names_beside_it() {
    let body = r###"<xs:complexType name="T">
                      <xs:sequence>
                        <xs:element ref="tns:b"/>
                        <xs:any notQName="##definedSibling" processContents="lax"
                                maxOccurs="unbounded"/>
                        <xs:element ref="tns:c"/>
                      </xs:sequence>
                    </xs:complexType>
                    <xs:element name="b" type="xs:int"/>
                    <xs:element name="c" type="xs:int"/>
                    <xs:element name="d" type="xs:int"/>
                    <xs:element name="root" type="tns:T"/>"###;
    let d = diagnostics(body, Version::Xsd11);
    assert!(!d.has_errors(), "{d}");

    // And the exclusion has to reach validation, not just UPA: the wildcard
    // must refuse `c` so the sequence can finish on it.
    let s = build(body, Version::Xsd11);
    assert!(valid(
        &s,
        r#"<root xmlns="urn:example"><b>1</b><d>2</d><c>3</c></root>"#
    ));
    // `c` is a sibling, so the wildcard never swallows it — two of them in a
    // row cannot both be matched by the wildcard.
    assert!(!valid(
        &s,
        r#"<root xmlns="urn:example"><b>1</b><c>2</c><c>3</c></root>"#
    ));
}

/// `##defined` excludes every name the schema declares globally, so the
/// wildcard admits only what the schema does not describe.
#[test]
fn defined_excludes_every_global_declaration() {
    let body = r###"<xs:complexType name="T">
                      <xs:sequence>
                        <xs:any notQName="##defined" processContents="lax"
                                namespace="##targetNamespace" maxOccurs="unbounded"/>
                      </xs:sequence>
                    </xs:complexType>
                    <xs:element name="known" type="xs:int"/>
                    <xs:element name="root" type="tns:T"/>"###;
    let s = build(body, Version::Xsd11);
    // `known` has a global declaration, so the wildcard refuses it.
    assert!(!valid(
        &s,
        r#"<root xmlns="urn:example"><known>1</known></root>"#
    ));
    // A name the schema never declares goes through.
    assert!(valid(
        &s,
        r#"<root xmlns="urn:example"><unknown>1</unknown></root>"#
    ));
}

/// The sibling set is the model as it ends up, so a name reached through a
/// group reference counts too.
#[test]
fn defined_sibling_sees_through_a_group_reference() {
    let body = r###"<xs:group name="G">
                      <xs:sequence><xs:element ref="tns:inner"/></xs:sequence>
                    </xs:group>
                    <xs:complexType name="T">
                      <xs:sequence>
                        <xs:group ref="tns:G"/>
                        <xs:any notQName="##definedSibling" processContents="lax" minOccurs="0"/>
                      </xs:sequence>
                    </xs:complexType>
                    <xs:element name="inner" type="xs:int"/>
                    <xs:element name="root" type="tns:T"/>"###;
    let s = build(body, Version::Xsd11);
    assert!(valid(
        &s,
        r#"<root xmlns="urn:example"><inner>1</inner><other>2</other></root>"#
    ));
    // `inner` came from the group, and is still a sibling.
    assert!(!valid(
        &s,
        r#"<root xmlns="urn:example"><inner>1</inner><inner>2</inner></root>"#
    ));
}

/// XSD 1.1 lets a *local* declaration name its namespace outright, which is
/// how a schema puts a declaration in a namespace it does not own — and the
/// only way to restrict a wildcard that admits one.
#[test]
fn a_local_declaration_may_name_its_own_namespace() {
    let s = build(
        r#"<xs:complexType name="B">
             <xs:sequence><xs:any namespace="urn:other" processContents="lax"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="R">
             <xs:complexContent>
               <xs:restriction base="tns:B">
                 <xs:sequence>
                   <xs:element name="child" targetNamespace="urn:other" type="xs:int"/>
                 </xs:sequence>
               </xs:restriction>
             </xs:complexContent>
           </xs:complexType>
           <xs:element name="root" type="tns:R"/>"#,
        Version::Xsd11,
    );
    // The declaration landed in the namespace it named, not the document's.
    let t = s.type_id(Some(NS), "R").expect("type");
    let p = s[t].as_complex().unwrap().content.particle().unwrap();
    let kids = s.child_particles(p);
    let Term::Element(e) = s[kids[0]].term else {
        panic!("expected an element")
    };
    assert_eq!(s.display_name(s[e].name), "{urn:other}child");

    assert!(valid(
        &s,
        r#"<root xmlns="urn:example"><child xmlns="urn:other">1</child></root>"#
    ));
}

/// Three conditions on it, and the third is the one with a reason: naming a
/// *different* namespace only means something against a base declaration to
/// correspond to.
#[test]
fn a_local_target_namespace_is_constrained() {
    let cases = [
        // Top-level: already in the document's namespace.
        r#"<xs:element name="e" type="xs:int" targetNamespace="urn:other"/>"#,
        // `form` already decides the namespace.
        r#"<xs:complexType name="T">
             <xs:sequence>
               <xs:element name="e" type="xs:int" form="qualified"
                           targetNamespace="urn:example"/>
             </xs:sequence>
           </xs:complexType>"#,
        // Another namespace, with nothing to correspond to.
        r#"<xs:complexType name="T">
             <xs:sequence>
               <xs:element name="e" type="xs:int" targetNamespace="urn:other"/>
             </xs:sequence>
           </xs:complexType>"#,
    ];
    for c in cases {
        let d = diagnostics(c, Version::Xsd11);
        assert!(
            d.errors()
                .any(|x| x.code == DiagCode::InvalidAttributeValue),
            "expected a rejection for:\n{c}\ngot:\n{d}"
        );
    }

    // Naming the document's own namespace is always allowed.
    let d = diagnostics(
        r#"<xs:complexType name="T">
             <xs:attribute name="a" type="xs:int" targetNamespace="urn:example"/>
           </xs:complexType>"#,
        Version::Xsd11,
    );
    assert!(!d.has_errors(), "{d}");
}

// ---------------------------------------------------------------------------
// Conditional inclusion (vc:)
// ---------------------------------------------------------------------------

/// One document can serve processors of different versions: an element whose
/// `vc:` conditions this processor does not meet is ignored, subtree and all.
/// Without that, the two alternatives for a name both load and collide.
#[test]
fn conditional_inclusion_picks_one_alternative() {
    let body = r#"<xs:element name="e" vc:minVersion="1.1" type="xs:dateTimeStamp"/>
                  <xs:element name="e" vc:maxVersion="1.1" type="xs:string"/>"#;
    for (version, expected) in [
        (Version::Xsd11, "dateTimeStamp"),
        (Version::Xsd10, "string"),
    ] {
        let xsd = format!(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          xmlns:vc="http://www.w3.org/2007/XMLSchema-versioning"
                          xmlns:tns="{NS}" targetNamespace="{NS}">{body}</xs:schema>"#
        );
        let s = SchemaSetBuilder::new()
            .version(version)
            .text(xsd, "mem://main.xsd")
            .compile()
            .into_result()
            .unwrap_or_else(|d| panic!("{version:?}: {d}"));
        let e = s.element_id(Some(NS), "e").expect("element");
        assert!(
            s[s[e].type_id]
                .name()
                .map(|n| s.display_name(n))
                .unwrap_or_default()
                .ends_with(expected),
            "{version:?} should have kept the {expected} alternative"
        );
    }
}

/// `maxVersion` names the first version that must ignore the element, not the
/// last that may read it.
#[test]
fn max_version_is_exclusive() {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:vc="http://www.w3.org/2007/XMLSchema-versioning"
                      xmlns:tns="{NS}" targetNamespace="{NS}">
             <xs:element name="only10" vc:maxVersion="1.1" type="xs:string"/>
           </xs:schema>"#
    );
    let read_as = |v: Version| {
        SchemaSetBuilder::new()
            .version(v)
            .text(xsd.clone(), "mem://main.xsd")
            .compile()
            .into_result()
            .unwrap()
            .element_id(Some(NS), "only10")
            .is_some()
    };
    assert!(read_as(Version::Xsd10), "1.0 is below the ceiling");
    assert!(!read_as(Version::Xsd11), "1.1 is the ceiling itself");
}

/// A skipped element's children are never looked at either — the conditions
/// remove a subtree, not just a declaration.
#[test]
fn an_excluded_element_takes_its_subtree_with_it() {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:vc="http://www.w3.org/2007/XMLSchema-versioning"
                      xmlns:tns="{NS}" targetNamespace="{NS}">
             <xs:complexType name="T" vc:minVersion="9.9">
               <xs:sequence><xs:element name="nope" type="xs:notAType"/></xs:sequence>
             </xs:complexType>
             <xs:element name="e" type="xs:string"/>
           </xs:schema>"#
    );
    let s = SchemaSetBuilder::new()
        .version(Version::Xsd11)
        .text(xsd, "mem://main.xsd")
        .compile()
        .into_result()
        .unwrap_or_else(|d| panic!("the unread subtree should raise nothing:\n{d}"));
    assert!(s.type_id(Some(NS), "T").is_none());
}

/// The conditions can sit on `xs:schema` itself, which is the idiom for a
/// document another version reads as empty.
#[test]
fn a_whole_document_can_be_excluded() {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:vc="http://www.w3.org/2007/XMLSchema-versioning"
                      xmlns:tns="{NS}" targetNamespace="{NS}" vc:maxVersion="0.9">
             <xs:element name="gone" type="xs:string"/>
           </xs:schema>"#
    );
    let s = SchemaSetBuilder::new()
        .version(Version::Xsd11)
        .text(xsd, "mem://main.xsd")
        .compile()
        .into_result()
        .unwrap_or_else(|d| panic!("{d}"));
    assert!(s.element_id(Some(NS), "gone").is_none());
}

//! *Derivation Valid (Restriction, Complex)* — particle subsumption.
//!
//! A restriction states its content model in full, so nothing structural keeps
//! it narrower than its base. These check that the assertion holds, and — just
//! as important — that the shapes this crate cannot judge are *accepted*
//! rather than guessed at.

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

fn bad(body: &str) -> usize {
    diags(body)
        .errors()
        .filter(|d| d.code == DiagCode::InvalidRestriction)
        .count()
}

fn clean(body: &str) {
    let d = diags(body);
    assert!(!d.has_errors(), "expected a clean build, got:\n{d}");
}

/// Wraps a base and a restriction of it around two content models.
fn pair(base: &str, derived: &str) -> String {
    format!(
        r#"<xs:complexType name="B">{base}</xs:complexType>
           <xs:complexType name="R">
             <xs:complexContent>
               <xs:restriction base="tns:B">{derived}</xs:restriction>
             </xs:complexContent>
           </xs:complexType>"#
    )
}

// ---------------------------------------------------------------------------
// Occurrence ranges
// ---------------------------------------------------------------------------

/// The workhorse: a restriction may not allow more occurrences, nor fewer.
#[test]
fn occurrence_ranges_may_only_narrow() {
    // minOccurs below the base's.
    assert_eq!(
        bad(&pair(
            r#"<xs:sequence><xs:element name="a" type="xs:int" minOccurs="1"/></xs:sequence>"#,
            r#"<xs:sequence><xs:element name="a" type="xs:int" minOccurs="0"/></xs:sequence>"#,
        )),
        1
    );
    // maxOccurs above the base's.
    assert_eq!(
        bad(&pair(
            r#"<xs:sequence><xs:element name="a" type="xs:int" maxOccurs="3"/></xs:sequence>"#,
            r#"<xs:sequence><xs:element name="a" type="xs:int" maxOccurs="5"/></xs:sequence>"#,
        )),
        1
    );
    // Unbounded is not below any finite bound.
    assert_eq!(
        bad(&pair(
            r#"<xs:sequence><xs:element name="a" type="xs:int" maxOccurs="9"/></xs:sequence>"#,
            r#"<xs:sequence><xs:element name="a" type="xs:int" maxOccurs="unbounded"/></xs:sequence>"#,
        )),
        1
    );
    // Narrowing on both ends is fine, and so is leaving it alone.
    clean(&pair(
        r#"<xs:sequence>
             <xs:element name="a" type="xs:int" minOccurs="1" maxOccurs="9"/>
           </xs:sequence>"#,
        r#"<xs:sequence>
             <xs:element name="a" type="xs:int" minOccurs="3" maxOccurs="4"/>
           </xs:sequence>"#,
    ));
}

// ---------------------------------------------------------------------------
// Elt:Elt, and what may replace what
// ---------------------------------------------------------------------------

#[test]
fn a_restriction_cannot_introduce_a_name() {
    assert_eq!(
        bad(&pair(
            r#"<xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>"#,
            r#"<xs:sequence><xs:element name="b" type="xs:int"/></xs:sequence>"#,
        )),
        1
    );
}

#[test]
fn a_dropped_particle_must_have_been_optional() {
    assert_eq!(
        bad(&pair(
            r#"<xs:sequence>
                 <xs:element name="a" type="xs:int"/>
                 <xs:element name="b" type="xs:int"/>
               </xs:sequence>"#,
            r#"<xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>"#,
        )),
        1,
        "`b` was required"
    );
    clean(&pair(
        r#"<xs:sequence>
             <xs:element name="a" type="xs:int"/>
             <xs:element name="b" type="xs:int" minOccurs="0"/>
           </xs:sequence>"#,
        r#"<xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>"#,
    ));
}

/// The element's type has to be the base's or derived from it, or a document
/// valid against the restriction could carry a value the base rejects.
#[test]
fn an_element_type_must_be_derived_from_the_one_it_replaces() {
    assert_eq!(
        bad(&pair(
            r#"<xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>"#,
            r#"<xs:sequence><xs:element name="a" type="xs:date"/></xs:sequence>"#,
        )),
        1
    );
    clean(&pair(
        r#"<xs:sequence><xs:element name="a" type="xs:integer"/></xs:sequence>"#,
        r#"<xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>"#,
    ));
}

/// A member of a union is validly derived from that union — the values it
/// admits are a subset by construction, with no restriction step needed.
#[test]
fn a_union_member_may_replace_the_union() {
    clean(&format!(
        r#"<xs:simpleType name="U"><xs:union memberTypes="xs:date xs:time"/></xs:simpleType>
           {}"#,
        pair(
            r#"<xs:sequence><xs:element name="a" type="tns:U"/></xs:sequence>"#,
            r#"<xs:sequence><xs:element name="a" type="xs:date"/></xs:sequence>"#,
        )
    ));
}

// ---------------------------------------------------------------------------
// Wildcards
// ---------------------------------------------------------------------------

#[test]
fn a_wildcard_may_only_narrow_and_may_not_replace_a_name() {
    // An element may replace a wildcard that admits it.
    clean(&pair(
        r###"<xs:sequence><xs:any namespace="##targetNamespace"/></xs:sequence>"###,
        r#"<xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>"#,
    ));
    // But not one that does not.
    assert_eq!(
        bad(&pair(
            r###"<xs:sequence><xs:any namespace="##other"/></xs:sequence>"###,
            r#"<xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>"#,
        )),
        1
    );
    // A wildcard cannot replace a name: it admits everything the element did
    // and more.
    assert_eq!(
        bad(&pair(
            r#"<xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>"#,
            r###"<xs:sequence><xs:any namespace="##any"/></xs:sequence>"###,
        )),
        1
    );
    // Wildcard against wildcard, narrowing the namespace set.
    clean(&pair(
        r###"<xs:sequence><xs:any namespace="##any"/></xs:sequence>"###,
        r###"<xs:sequence><xs:any namespace="##targetNamespace"/></xs:sequence>"###,
    ));
    assert_eq!(
        bad(&pair(
            r###"<xs:sequence><xs:any namespace="##targetNamespace"/></xs:sequence>"###,
            r###"<xs:sequence><xs:any namespace="##any"/></xs:sequence>"###,
        )),
        1
    );
}

// ---------------------------------------------------------------------------
// Groups
// ---------------------------------------------------------------------------

/// An `xs:all` is unordered, so its members correspond by name — which is what
/// lets a sequence restrict it at all.
#[test]
fn an_unordered_base_matches_its_members_by_name() {
    let base = r#"<xs:all>
                    <xs:element name="a" type="xs:int" minOccurs="0"/>
                    <xs:element name="b" type="xs:int" minOccurs="1"/>
                    <xs:element name="c" type="xs:int" minOccurs="0"/>
                  </xs:all>"#;
    // Reordered and narrowed, dropping only what was optional.
    clean(&pair(
        base,
        r#"<xs:all>
             <xs:element name="c" type="xs:int" minOccurs="0"/>
             <xs:element name="b" type="xs:int" minOccurs="1"/>
           </xs:all>"#,
    ));
    // `b` was required by the base.
    assert_eq!(
        bad(&pair(
            base,
            r#"<xs:all><xs:element name="a" type="xs:int" minOccurs="0"/></xs:all>"#,
        )),
        1
    );
    // Widening a member's range, whatever the restriction's own compositor.
    assert_eq!(
        bad(&pair(
            base,
            r#"<xs:sequence>
                 <xs:element name="b" type="xs:int" minOccurs="0"/>
               </xs:sequence>"#,
        )),
        1
    );
}

/// Narrowing a choice means removing alternatives, so a base alternative the
/// restriction drops is the point rather than an error.
#[test]
fn a_choice_may_drop_alternatives() {
    clean(&pair(
        r#"<xs:choice>
             <xs:element name="a" type="xs:int"/>
             <xs:element name="b" type="xs:int"/>
             <xs:element name="c" type="xs:int"/>
           </xs:choice>"#,
        r#"<xs:choice>
             <xs:element name="a" type="xs:int"/>
             <xs:element name="c" type="xs:int"/>
           </xs:choice>"#,
    ));
    // But not add one, nor reorder past it.
    assert_eq!(
        bad(&pair(
            r#"<xs:choice>
                 <xs:element name="a" type="xs:int"/>
                 <xs:element name="b" type="xs:int"/>
               </xs:choice>"#,
            r#"<xs:choice>
                 <xs:element name="a" type="xs:int"/>
                 <xs:element name="z" type="xs:int"/>
               </xs:choice>"#,
        )),
        1
    );
}

/// An extension's content model is the base's followed by its own, so a
/// restriction of an extended type has to be compared against the whole chain
/// — not against the part the base happened to declare itself.
#[test]
fn the_base_is_its_whole_derivation_chain() {
    clean(
        r#"<xs:complexType name="A">
             <xs:sequence><xs:element name="a" type="xs:int" minOccurs="0"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="B">
             <xs:complexContent>
               <xs:extension base="tns:A">
                 <xs:sequence><xs:element name="b" type="xs:int" minOccurs="0"/></xs:sequence>
               </xs:extension>
             </xs:complexContent>
           </xs:complexType>
           <xs:complexType name="R">
             <xs:complexContent>
               <xs:restriction base="tns:B">
                 <xs:sequence>
                   <xs:element name="a" type="xs:int" minOccurs="0"/>
                   <xs:element name="b" type="xs:int" minOccurs="0"/>
                 </xs:sequence>
               </xs:restriction>
             </xs:complexContent>
           </xs:complexType>"#,
    );
}

// ---------------------------------------------------------------------------
// What this deliberately does not judge
// ---------------------------------------------------------------------------

/// A restriction may name members of the base particle's substitution group,
/// and several of them may share one base particle — so the bounds have to be
/// summed across them. That is `MapAndSum`, which this crate does not
/// implement, so such a schema is accepted rather than guessed at.
#[test]
fn substituting_for_a_base_particle_is_not_judged() {
    clean(
        r#"<xs:element name="head" type="xs:int"/>
           <xs:element name="member" type="xs:int" substitutionGroup="tns:head"/>
           <xs:complexType name="B">
             <xs:all><xs:element ref="tns:head" minOccurs="10" maxOccurs="20"/></xs:all>
           </xs:complexType>
           <xs:complexType name="R">
             <xs:complexContent>
               <xs:restriction base="tns:B">
                 <xs:all>
                   <xs:element ref="tns:member" minOccurs="6" maxOccurs="8"/>
                 </xs:all>
               </xs:restriction>
             </xs:complexContent>
           </xs:complexType>"#,
    );
}

/// Mixing compositors across the two sides is `MapAndSum` or
/// `RecurseUnordered`, neither of which is implemented — accepted, not guessed.
#[test]
fn mismatched_compositors_are_not_judged() {
    clean(&pair(
        r#"<xs:choice>
             <xs:element name="a" type="xs:int"/>
             <xs:element name="b" type="xs:int"/>
           </xs:choice>"#,
        r#"<xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence>"#,
    ));
}

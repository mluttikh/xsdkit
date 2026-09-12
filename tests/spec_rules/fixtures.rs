//! One rule, one pair: the smallest schema that breaks it, and a near-miss.
//!
//! The negative case pins that the rule fires *and which code it fires*. The
//! near-miss is the half that is easy to skip and does the most work: a check
//! written slightly too broadly passes every negative test in the world and
//! rejects schemas that are fine. Every pair here is one rule, named by its
//! anchor in the specification, so a reader can go from the table to the
//! fixture to the REC without guessing.
//!
//! Adding a pair means adding its rule id to [`COVERED`] and setting the
//! `fixtures` column in `tests/conformance/spec-rules.tsv`; `main.rs` requires
//! the two to agree, and requires the rule to be one the table says is
//! enforced — a passing negative fixture for an unenforced rule is a
//! contradiction.

use fxhash::FxHashMap;
use xsdkit::instance::PsviEvent;
use xsdkit::{Conformance, DiagCode, Diagnostics, Resolver, SchemaSetBuilder, Schemas, Version};

/// The rules with fixtures below, as `spec#anchor`.
pub const COVERED: &[&str] = &[
    "structures#cos-all-limited",
    "structures#cos-nonambig",
    "structures#sic-attr-decl",
    "structures#sic-attrDefault",
    "structures#sic-elt-decl",
    "structures#sic-eltDefault",
    "structures#sic-eltType",
    "structures#sic-schema",
    "structures#src-import",
    "structures#src-include",
];

/// Resolves `schemaLocation` out of a map, so a composition fixture needs no
/// files. The same idiom as `tests/integration_tests.rs`.
#[derive(Default)]
struct MapResolver(FxHashMap<String, String>);

impl MapResolver {
    fn with(mut self, name: &str, xsd: &str) -> Self {
        self.0.insert(name.to_string(), xsd.to_string());
        self
    }
}

impl Resolver for MapResolver {
    fn resolve(&self, location: &str, _base: Option<&str>) -> Result<(String, Vec<u8>), String> {
        self.0
            .get(location)
            .map(|t| (location.to_string(), t.clone().into_bytes()))
            .ok_or_else(|| format!("not in map: {location}"))
    }
}

/// Compiles `main` with `other` available at its own name, and hands back
/// every diagnostic.
fn compose(main: &str, other: (&str, &str)) -> Diagnostics {
    SchemaSetBuilder::new()
        .conformance(Conformance::Strict)
        .resolver(MapResolver::default().with(other.0, other.1))
        .text(main, "mem://main.xsd")
        .compile()
        .diagnostics
}

/// Compiles one document at one version. Several rules below say different
/// things in 1.0 and 1.1, so the version is never implicit here.
fn build(version: Version, body: &str) -> Diagnostics {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:t" xmlns:t="urn:t">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .version(version)
        .conformance(Conformance::Strict)
        .text(xsd, "mem://main.xsd")
        .compile()
        .diagnostics
}

/// The same body read as both versions, for a rule that does not differ.
fn build_both(body: &str) -> [Diagnostics; 2] {
    [build(Version::Xsd10, body), build(Version::Xsd11, body)]
}

#[track_caller]
fn expect_code(d: &Diagnostics, code: DiagCode) {
    let found: Vec<&str> = d.errors().map(|e| e.code.as_str()).collect();
    assert!(
        found.contains(&code.as_str()),
        "expected {}, got {found:?}:\n{d}",
        code.as_str()
    );
}

/// Compiles a schema that must build cleanly, for the fixtures that go on to
/// validate a document against it.
fn schema(body: &str) -> Schemas {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:t" xmlns:t="urn:t"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .text(xsd, "mem://main.xsd")
        .compile()
        .into_result()
        .unwrap_or_else(|d| panic!("expected a clean build, got:\n{d}"))
}

#[track_caller]
fn expect_clean(d: &Diagnostics) {
    assert!(!d.has_errors(), "expected a clean build, got:\n{d}");
}

// ---------------------------------------------------------------------------
// structures#src-include — Inclusion Constraints and Semantics
// ---------------------------------------------------------------------------

/// Clause 2: an included document may declare the includer's namespace, or
/// none at all. Anything else is somebody else's namespace.
#[test]
fn src_include_rejects_a_document_of_another_namespace() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:mine">
             <xs:include schemaLocation="theirs.xsd"/>
           </xs:schema>"#,
        (
            "theirs.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          targetNamespace="urn:theirs">
                 <xs:element name="e" type="xs:string"/>
               </xs:schema>"#,
        ),
    );
    expect_code(&d, DiagCode::IncludeNamespaceMismatch);
}

/// Clause 2.1: the same namespace, which is the ordinary case and must not be
/// caught by the check above.
#[test]
fn src_include_accepts_the_same_namespace() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:mine">
             <xs:include schemaLocation="part.xsd"/>
             <xs:element name="a" type="xs:string"/>
           </xs:schema>"#,
        (
            "part.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          targetNamespace="urn:mine">
                 <xs:element name="b" type="xs:string"/>
               </xs:schema>"#,
        ),
    );
    expect_clean(&d);
}

/// Clause 2.3: no `targetNamespace` of its own, so it is absorbed into the
/// includer's. The near-miss that matters most — a check comparing the two
/// namespaces without this case would reject every chameleon include there is.
#[test]
fn src_include_accepts_a_chameleon() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:mine">
             <xs:include schemaLocation="chameleon.xsd"/>
           </xs:schema>"#,
        (
            "chameleon.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
                 <xs:element name="absorbed" type="xs:string"/>
               </xs:schema>"#,
        ),
    );
    expect_clean(&d);
}

/// Clause 2.2: neither has one. Also fine, and a check keyed on "the included
/// document declared nothing" rather than on the comparison would get this
/// right by accident and 2.1 wrong.
#[test]
fn src_include_accepts_neither_having_a_namespace() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
             <xs:include schemaLocation="part.xsd"/>
           </xs:schema>"#,
        (
            "part.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
                 <xs:element name="b" type="xs:string"/>
               </xs:schema>"#,
        ),
    );
    expect_clean(&d);
}

/// A no-namespace document including one that *has* a namespace is the same
/// violation in the other direction: there is nothing for it to be absorbed
/// into, so the namespaces simply disagree.
#[test]
fn src_include_rejects_a_namespace_where_the_includer_has_none() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
             <xs:include schemaLocation="theirs.xsd"/>
           </xs:schema>"#,
        (
            "theirs.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          targetNamespace="urn:theirs">
                 <xs:element name="e" type="xs:string"/>
               </xs:schema>"#,
        ),
    );
    expect_code(&d, DiagCode::IncludeNamespaceMismatch);
}

// ---------------------------------------------------------------------------
// structures#src-import — Import Constraints and Semantics
// ---------------------------------------------------------------------------

/// Clause 3.1: the `namespace` on the import has to be the one the document
/// declares. Naming a namespace and getting a document that declares another
/// is the common form of this — a `schemaLocation` pointing at the wrong file.
#[test]
fn src_import_rejects_a_document_declaring_another_namespace() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:mine">
             <xs:import namespace="urn:wanted" schemaLocation="other.xsd"/>
           </xs:schema>"#,
        (
            "other.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          targetNamespace="urn:actual">
                 <xs:element name="e" type="xs:string"/>
               </xs:schema>"#,
        ),
    );
    expect_code(&d, DiagCode::ImportNamespaceMismatch);
}

/// Clause 3.1 satisfied. The ordinary case, and the reason the check cannot
/// simply compare against the *importing* schema's namespace.
#[test]
fn src_import_accepts_the_namespace_it_named() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:o="urn:other" targetNamespace="urn:mine">
             <xs:import namespace="urn:other" schemaLocation="other.xsd"/>
             <xs:element name="a" type="o:T"/>
           </xs:schema>"#,
        (
            "other.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          targetNamespace="urn:other">
                 <xs:simpleType name="T">
                   <xs:restriction base="xs:string"/>
                 </xs:simpleType>
               </xs:schema>"#,
        ),
    );
    expect_clean(&d);
}

/// Clause 3.2: an import with no `namespace` is asking for a document that has
/// none. One that declares a namespace is not it.
#[test]
fn src_import_rejects_a_namespace_where_none_was_named() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:mine">
             <xs:import schemaLocation="other.xsd"/>
           </xs:schema>"#,
        (
            "other.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          targetNamespace="urn:unexpected">
                 <xs:element name="e" type="xs:string"/>
               </xs:schema>"#,
        ),
    );
    expect_code(&d, DiagCode::ImportNamespaceMismatch);
}

/// Clause 3.2 satisfied: no `namespace`, and a document with none. This is how
/// a namespaced schema reaches components that live in no namespace, and a
/// check that treated "no namespace named" as "anything goes" would pass the
/// negative case above by accident.
#[test]
fn src_import_accepts_no_namespace_when_none_was_named() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:mine">
             <xs:import schemaLocation="plain.xsd"/>
             <xs:element name="a" type="T"/>
           </xs:schema>"#,
        (
            "plain.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
                 <xs:simpleType name="T">
                   <xs:restriction base="xs:string"/>
                 </xs:simpleType>
               </xs:schema>"#,
        ),
    );
    expect_clean(&d);
}

/// An import is not an include: a document in another namespace is exactly
/// what it is for, and must not be absorbed into the importer's namespace the
/// way a chameleon include is.
#[test]
fn src_import_does_not_absorb_the_document_it_names() {
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:o="urn:other" targetNamespace="urn:mine">
             <xs:import namespace="urn:other" schemaLocation="other.xsd"/>
             <xs:element name="a" type="o:T"/>
           </xs:schema>"#,
        (
            "other.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          targetNamespace="urn:other">
                 <xs:simpleType name="T">
                   <xs:restriction base="xs:string"/>
                 </xs:simpleType>
               </xs:schema>"#,
        ),
    );
    expect_clean(&d);
    // And the same reference without the prefix must not resolve, which is
    // what would happen if an import coerced the namespace.
    let d = compose(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:mine">
             <xs:import namespace="urn:other" schemaLocation="other.xsd"/>
             <xs:element name="a" type="T"/>
           </xs:schema>"#,
        (
            "other.xsd",
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          targetNamespace="urn:other">
                 <xs:simpleType name="T">
                   <xs:restriction base="xs:string"/>
                 </xs:simpleType>
               </xs:schema>"#,
        ),
    );
    expect_code(&d, DiagCode::UnresolvedReference);
}

// ---------------------------------------------------------------------------
// structures#cos-all-limited — All Group Limited
// ---------------------------------------------------------------------------
//
// The clearest case of a rule the two versions state differently. 1.0 confines
// an `all` to two positions and caps each member at one occurrence; 1.1 lifts
// the cap, allows an `all` inside an `all`, and in exchange requires a group
// referenced from inside an `all` to be an `all` group.

/// Clause 1: not inside an `xs:sequence`, in either version. An `all` matches
/// its members in any order and a sequence fixes an order, so there is no
/// reading of the two together.
#[test]
fn cos_all_limited_rejects_an_all_inside_a_sequence() {
    for d in build_both(
        r#"<xs:complexType name="T">
             <xs:sequence>
               <xs:all>
                 <xs:element name="a" type="xs:string"/>
               </xs:all>
             </xs:sequence>
           </xs:complexType>"#,
    ) {
        expect_code(&d, DiagCode::InvalidOccurrence);
    }
}

/// And not inside an `xs:choice` either, which is the same clause and the
/// mistake a check written only against `xs:sequence` would miss.
#[test]
fn cos_all_limited_rejects_an_all_inside_a_choice() {
    for d in build_both(
        r#"<xs:complexType name="T">
             <xs:choice>
               <xs:all>
                 <xs:element name="a" type="xs:string"/>
               </xs:all>
             </xs:choice>
           </xs:complexType>"#,
    ) {
        expect_code(&d, DiagCode::InvalidOccurrence);
    }
}

/// Clause 1.2: a complex type's whole content model, with `maxOccurs` of 1.
/// Repeating the group would need the ordering its members do not have.
#[test]
fn cos_all_limited_rejects_a_repeated_all() {
    for d in build_both(
        r#"<xs:complexType name="T">
             <xs:all maxOccurs="2">
               <xs:element name="a" type="xs:string"/>
             </xs:all>
           </xs:complexType>"#,
    ) {
        expect_code(&d, DiagCode::InvalidOccurrence);
    }
}

/// The two positions clause 1 does allow: a complex type's content model, and
/// a named group definition. Both versions, and the near-miss for everything
/// above.
#[test]
fn cos_all_limited_accepts_the_positions_it_allows() {
    for d in build_both(
        r#"<xs:complexType name="T">
             <xs:all minOccurs="0">
               <xs:element name="a" type="xs:string"/>
               <xs:element name="b" type="xs:string" minOccurs="0"/>
             </xs:all>
           </xs:complexType>
           <xs:group name="g">
             <xs:all>
               <xs:element name="c" type="xs:string"/>
             </xs:all>
           </xs:group>"#,
    ) {
        expect_clean(&d);
    }
}

/// 1.0 clause 2: a member occurs at most once. This is the half of the rule
/// the W3C suite never exercises — the `All` test set is 1.1-only — so without
/// a fixture the 1.0 branch would be a check nobody had ever seen fire.
#[test]
fn cos_all_limited_rejects_a_repeating_member_in_1_0() {
    let d = build(
        Version::Xsd10,
        r#"<xs:complexType name="T">
             <xs:all>
               <xs:element name="a" type="xs:string" maxOccurs="unbounded"/>
             </xs:all>
           </xs:complexType>"#,
    );
    expect_code(&d, DiagCode::InvalidOccurrence);
}

/// And 1.1 lifts it: `xsd1_1-AllGroups-MaxOccurs`. The same schema, and the
/// version is the only thing that changes the answer — which is why a rule
/// like this one needs a pair rather than a case.
#[test]
fn cos_all_limited_accepts_a_repeating_member_in_1_1() {
    let d = build(
        Version::Xsd11,
        r#"<xs:complexType name="T">
             <xs:all>
               <xs:element name="a" type="xs:string" maxOccurs="unbounded"/>
             </xs:all>
           </xs:complexType>"#,
    );
    expect_clean(&d);
}

/// 1.1 clause 2: a group referenced from inside an `all` has to *be* an `all`
/// group. `saxonData/All/all008` is this case, and it needs the reference
/// resolved — the compositor of the referenced group is not in the document
/// that refers to it.
#[test]
fn cos_all_limited_rejects_a_reference_to_a_sequence_group() {
    let d = build(
        Version::Xsd11,
        r#"<xs:complexType name="T">
             <xs:all>
               <xs:element name="a" type="xs:string"/>
               <xs:group ref="t:g"/>
             </xs:all>
           </xs:complexType>
           <xs:group name="g">
             <xs:sequence>
               <xs:element name="b" type="xs:string"/>
             </xs:sequence>
           </xs:group>"#,
    );
    expect_code(&d, DiagCode::InvalidOccurrence);
}

/// Clause 1.3: the reference is allowed, but exactly once.
/// `saxonData/All/all009` differs from the case above only in the occurrence.
#[test]
fn cos_all_limited_rejects_an_optional_reference_inside_an_all() {
    let d = build(
        Version::Xsd11,
        r#"<xs:complexType name="T">
             <xs:all>
               <xs:element name="a" type="xs:string"/>
               <xs:group ref="t:g" minOccurs="0"/>
             </xs:all>
           </xs:complexType>
           <xs:group name="g">
             <xs:all>
               <xs:element name="b" type="xs:string"/>
             </xs:all>
           </xs:group>"#,
    );
    expect_code(&d, DiagCode::InvalidOccurrence);
}

/// The near-miss for both of those: an `all` group, referenced exactly once,
/// from inside an `all`. This is the `xsd1_1-AllGroups-NamedModelGroupRef`
/// feature, and a check that simply refused group references inside an `all`
/// would pass every negative case above and reject this.
#[test]
fn cos_all_limited_accepts_an_all_group_referenced_once() {
    let d = build(
        Version::Xsd11,
        r#"<xs:complexType name="T">
             <xs:all>
               <xs:element name="a" type="xs:string"/>
               <xs:group ref="t:g"/>
             </xs:all>
           </xs:complexType>
           <xs:group name="g">
             <xs:all>
               <xs:element name="b" type="xs:string"/>
             </xs:all>
           </xs:group>"#,
    );
    expect_clean(&d);
}

/// An inline `xs:all` nested in another: allowed in 1.1 at exactly one
/// occurrence, never in 1.0.
#[test]
fn cos_all_limited_nests_an_all_only_in_1_1() {
    let body = r#"<xs:complexType name="T">
                    <xs:all>
                      <xs:element name="a" type="xs:string"/>
                      <xs:all>
                        <xs:element name="b" type="xs:string"/>
                      </xs:all>
                    </xs:all>
                  </xs:complexType>"#;
    expect_clean(&build(Version::Xsd11, body));
    expect_code(&build(Version::Xsd10, body), DiagCode::InvalidOccurrence);
}

/// And in 1.1 the nested one is still bounded to one occurrence.
#[test]
fn cos_all_limited_rejects_a_repeated_nested_all() {
    let d = build(
        Version::Xsd11,
        r#"<xs:complexType name="T">
             <xs:all>
               <xs:all maxOccurs="2">
                 <xs:element name="b" type="xs:string"/>
               </xs:all>
             </xs:all>
           </xs:complexType>"#,
    );
    expect_code(&d, DiagCode::InvalidOccurrence);
}

// ---------------------------------------------------------------------------
// structures#cos-nonambig — Unique Particle Attribution
// ---------------------------------------------------------------------------
//
// UPA over a sequence or a choice is automaton determinism, and has been
// checked since the automata existed. Over an `xs:all` there is no automaton —
// members get per-member counters — and it went unchecked until
// `saxonData/All`'s all240 to all243 said so. These fixtures are the `xs:all`
// half; the automaton half is covered by `tests/content_model.rs`.

/// Two members naming the same element. A violation even though both particles
/// refer to one declaration: the matcher has to attribute the element to a
/// member, and either would do.
#[test]
fn cos_nonambig_rejects_two_all_members_with_one_name() {
    for d in build_both(
        r#"<xs:element name="o" type="xs:integer"/>
           <xs:complexType name="T">
             <xs:all>
               <xs:element ref="t:o"/>
               <xs:element name="x" type="xs:boolean"/>
               <xs:element ref="t:o"/>
             </xs:all>
           </xs:complexType>"#,
    ) {
        expect_code(&d, DiagCode::AmbiguousContentModel);
    }
}

/// One member's substitution group reaching another member's name. The overlap
/// is between the *closures*, not the declarations, which is why the check
/// compares what each member admits rather than what it names.
#[test]
fn cos_nonambig_rejects_an_all_member_reachable_by_substitution() {
    for d in build_both(
        r#"<xs:element name="o" type="xs:integer"/>
           <xs:element name="p" substitutionGroup="t:o" type="xs:integer"/>
           <xs:complexType name="T">
             <xs:all>
               <xs:element ref="t:o"/>
               <xs:element name="x" type="xs:boolean"/>
               <xs:element ref="t:p"/>
             </xs:all>
           </xs:complexType>"#,
    ) {
        expect_code(&d, DiagCode::AmbiguousContentModel);
    }
}

/// Two wildcards over overlapping namespaces. `urn:b` satisfies either, and
/// unlike the element-versus-wildcard case below, XSD 1.1 does not resolve
/// this one — there is no declaration to prefer.
#[test]
fn cos_nonambig_rejects_two_overlapping_wildcards_in_an_all() {
    for d in build_both(
        r#"<xs:complexType name="T">
             <xs:all>
               <xs:any namespace="urn:a urn:b" processContents="lax"/>
               <xs:any namespace="urn:b urn:c" processContents="lax"/>
             </xs:all>
           </xs:complexType>"#,
    ) {
        expect_code(&d, DiagCode::AmbiguousContentModel);
    }
}

/// Members that cannot be confused are fine, however many there are. The
/// near-miss for all three cases above — a check that reported every pair of
/// `xs:all` members would pass them and reject every `xs:all` ever written.
#[test]
fn cos_nonambig_accepts_distinct_all_members() {
    for d in build_both(
        r#"<xs:complexType name="T">
             <xs:all>
               <xs:element name="a" type="xs:string"/>
               <xs:element name="b" type="xs:string" minOccurs="0"/>
               <xs:element name="c" type="xs:string"/>
             </xs:all>
           </xs:complexType>"#,
    ) {
        expect_clean(&d);
    }
}

/// An element competing with a wildcard inside an `xs:all`: ambiguous in 1.0,
/// and resolved in favour of the element in 1.1. The same version split the
/// automaton half already applies, and applying it in one place and not the
/// other is exactly what sharing the overlap predicate prevents.
#[test]
fn cos_nonambig_resolves_an_element_against_a_wildcard_only_in_1_1() {
    // `r###` because the content carries `"##local"`, and `"##` would close an
    // `r#"…"#` literal early — see AGENTS.md on raw strings and `##`.
    let body = r###"<xs:complexType name="T">
                    <xs:all>
                      <xs:element name="a" type="xs:string"/>
                      <xs:any namespace="##local" processContents="lax"/>
                    </xs:all>
                  </xs:complexType>"###;
    expect_code(
        &build(Version::Xsd10, body),
        DiagCode::AmbiguousContentModel,
    );
    expect_clean(&build(Version::Xsd11, body));
}

// ---------------------------------------------------------------------------
// Schema Information Set Contributions
// ---------------------------------------------------------------------------
//
// The sixteen contributions are *outputs*, not constraints: the question is
// not whether a schema is rejected but whether the fact reaches a consumer of
// the PSVI. The W3C suite cannot ask that at all — it reads one boolean per
// document — so every one of these rows rested on reading `src/instance.rs`
// until these fixtures existed. Five of the sixteen turned out to be claiming
// more than the API delivers, and their rows now say so.
//
// Only the contributions that *are* exposed get fixtures; the table's `no`
// rows are absences, and the gate refuses a fixture for one.

/// *Element Declaration*: the governing declaration reaches the consumer, and
/// is absent exactly when assessment found none.
#[test]
fn sic_elt_decl_hands_over_the_governing_declaration() {
    let s = schema(
        r###"<xs:element name="doc">
               <xs:complexType>
                 <xs:sequence>
                   <xs:element name="known" type="xs:string"/>
                   <xs:any namespace="##other" processContents="skip"/>
                 </xs:sequence>
               </xs:complexType>
             </xs:element>"###,
    );
    let mut seen = Vec::new();
    let r = s.document_validator().validate_with(
        r#"<t:doc xmlns:t="urn:t" xmlns:o="urn:other">
             <t:known>x</t:known><o:anything/>
           </t:doc>"#,
        |ev| {
            if let PsviEvent::StartElement {
                name, declaration, ..
            } = ev
            {
                seen.push((s.display_psvi_name(&name), declaration.is_some()));
            }
        },
    );
    assert!(r.is_valid(), "{}", r.diagnostics);
    // The root and `known` are assessed; the element under a `skip` wildcard
    // is not, and says so by having no declaration rather than by being
    // absent from the stream.
    //
    // And it is named, though the schema has no symbols for that name:
    // `PsviName::Foreign` carries what the document spelled. Writing this
    // fixture is what found the event reporting the parent's name there
    // instead — `{urn:t}doc`, confidently wrong.
    assert_eq!(
        seen,
        vec![
            ("{urn:t}doc".to_string(), true),
            ("{urn:t}known".to_string(), true),
            ("{urn:other}anything".to_string(), false),
        ]
    );
}

/// And the pair balances: a `StartElement` that reports `UNKNOWN` is closed by
/// an `EndElement` that reports `UNKNOWN`, because a consumer folds the stream
/// into a tree by matching them.
#[test]
fn sic_elt_decl_closes_what_it_opened() {
    let s = schema(
        r###"<xs:element name="doc">
               <xs:complexType>
                 <xs:sequence>
                   <xs:any namespace="##other" processContents="skip"/>
                 </xs:sequence>
               </xs:complexType>
             </xs:element>"###,
    );
    let mut opened = Vec::new();
    let mut closed = Vec::new();
    let r = s.document_validator().validate_with(
        r#"<t:doc xmlns:t="urn:t" xmlns:o="urn:other"><o:a><o:b/></o:a></t:doc>"#,
        // `PsviEvent` is `#[non_exhaustive]` on purpose, so a consumer's match
        // needs the catch-all even when it lists every variant there is today.
        |ev| match ev {
            PsviEvent::StartElement { name, .. } => opened.push(name),
            PsviEvent::EndElement { name, .. } => closed.push(name),
            _ => {}
        },
    );
    assert!(r.is_valid(), "{}", r.diagnostics);
    // `o:b` is inside the skipped subtree and never announced, so the stream
    // is the root and `o:a` only — and it balances.
    assert_eq!(opened.len(), 2);
    closed.reverse();
    assert_eq!(opened, closed);
}

/// *Attribute Declaration*: the same for attributes, including one a wildcard
/// admitted and `strict` assessment then found a declaration for.
#[test]
fn sic_attr_decl_hands_over_the_governing_declaration() {
    let s = schema(
        r#"<xs:attribute name="global" type="xs:integer"/>
           <xs:element name="doc">
             <xs:complexType>
               <xs:attribute name="own" type="xs:string"/>
               <xs:anyAttribute namespace="urn:t" processContents="strict"/>
             </xs:complexType>
           </xs:element>"#,
    );
    let mut seen = Vec::new();
    let r = s.document_validator().validate_with(
        r#"<t:doc xmlns:t="urn:t" own="a" t:global="7"/>"#,
        |ev| {
            if let PsviEvent::StartElement { attributes, .. } = ev {
                for a in attributes {
                    seen.push((s.display_name(a.name), a.declaration.is_some()));
                }
            }
        },
    );
    assert!(r.is_valid(), "{}", r.diagnostics);
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("own".to_string(), true),
            ("{urn:t}global".to_string(), true),
        ]
    );
}

/// *Element Validated by Type*: the type actually in force, which `xsi:type`
/// can replace. The flag beside it is the part a consumer cannot reconstruct —
/// the same type id could have come from either place.
#[test]
fn sic_elt_type_reports_the_type_in_force_and_where_it_came_from() {
    let s = schema(
        r#"<xs:complexType name="Base">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="Derived">
             <xs:complexContent>
               <xs:extension base="t:Base">
                 <xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence>
               </xs:extension>
             </xs:complexContent>
           </xs:complexType>
           <xs:element name="doc" type="t:Base"/>"#,
    );
    let mut seen = Vec::new();
    let r = s.document_validator().validate_with(
        r#"<t:doc xmlns:t="urn:t" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
                 xsi:type="t:Derived"><t:a>x</t:a><t:b>y</t:b></t:doc>"#,
        |ev| {
            if let PsviEvent::StartElement {
                name,
                type_id,
                type_from_instance,
                ..
            } = ev
            {
                if s.display_psvi_name(&name) == "{urn:t}doc" {
                    let ty = s[type_id].name().map(|n| s.display_name(n));
                    seen.push((ty, type_from_instance));
                }
            }
        },
    );
    assert!(r.is_valid(), "{}", r.diagnostics);
    assert_eq!(seen, vec![(Some("{urn:t}Derived".to_string()), true)]);
}

/// *Element Default Value*: an empty element whose declaration carries a
/// `default` arrives with the content the schema supplied, flagged as coming
/// from the schema rather than from the document.
///
/// Without the flag a consumer cannot tell a supplied value from a written
/// one, which is the whole point of the `fixed` idiom.
#[test]
fn sic_elt_default_flags_content_the_schema_supplied() {
    let s = schema(r#"<xs:element name="unit" type="xs:string" default="m"/>"#);
    let mut seen = Vec::new();
    let r = s
        .document_validator()
        .validate_with(r#"<t:unit xmlns:t="urn:t"/>"#, |ev| {
            if let PsviEvent::Text {
                lexical,
                from_schema,
                ..
            } = ev
            {
                seen.push((lexical, from_schema));
            }
        });
    assert!(r.is_valid(), "{}", r.diagnostics);
    assert_eq!(seen, vec![("m".to_string(), true)]);

    // And the near-miss: the same declaration with the value written out is
    // not from the schema.
    let mut seen = Vec::new();
    s.document_validator()
        .validate_with(r#"<t:unit xmlns:t="urn:t">ft</t:unit>"#, |ev| {
            if let PsviEvent::Text {
                lexical,
                from_schema,
                ..
            } = ev
            {
                seen.push((lexical, from_schema));
            }
        });
    assert_eq!(seen, vec![("ft".to_string(), false)]);
}

/// *Attribute Default Value*: the same for an attribute the document leaves
/// out, which is where it matters most — the attribute is not in the document
/// at all, so a consumer reading the XML would never see it.
#[test]
fn sic_attr_default_supplies_an_absent_attribute() {
    let s = schema(
        r#"<xs:element name="doc">
             <xs:complexType>
               <xs:attribute name="unit" type="xs:string" fixed="m"/>
               <xs:attribute name="n" type="xs:integer"/>
             </xs:complexType>
           </xs:element>"#,
    );
    let mut seen = Vec::new();
    let r = s
        .document_validator()
        .validate_with(r#"<t:doc xmlns:t="urn:t" n="3"/>"#, |ev| {
            if let PsviEvent::StartElement { attributes, .. } = ev {
                for a in attributes {
                    seen.push((s.display_name(a.name), a.lexical.clone(), a.from_schema));
                }
            }
        });
    assert!(r.is_valid(), "{}", r.diagnostics);
    seen.sort();
    assert_eq!(
        seen,
        vec![
            ("n".to_string(), "3".to_string(), false),
            ("unit".to_string(), "m".to_string(), true),
        ]
    );
}

/// *Schema Information*: the components themselves are part of the
/// contribution, and here they are the thing the ids in every event above are
/// indices into. A PSVI of bare ids with no schema to resolve them against
/// would satisfy none of the rules above.
#[test]
fn sic_schema_resolves_the_ids_the_psvi_hands_out() {
    let s = schema(r#"<xs:element name="doc" type="xs:integer"/>"#);
    let mut resolved = Vec::new();
    let r = s
        .document_validator()
        .validate_with(r#"<t:doc xmlns:t="urn:t">42</t:doc>"#, |ev| {
            if let PsviEvent::StartElement {
                declaration: Some(id),
                type_id,
                ..
            } = ev
            {
                // Both ids index back into the schema that produced them.
                resolved.push((
                    s.display_name(s[id].name),
                    s[type_id].name().map(|n| s.display_name(n)),
                ));
            }
        });
    assert!(r.is_valid(), "{}", r.diagnostics);
    assert_eq!(
        resolved,
        vec![(
            "{urn:t}doc".to_string(),
            Some("{http://www.w3.org/2001/XMLSchema}integer".to_string())
        )]
    );
}

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
use xsdkit::{Conformance, DiagCode, Diagnostics, Resolver, SchemaSetBuilder};

/// The rules with fixtures below, as `spec#anchor`.
pub const COVERED: &[&str] = &["structures#src-include", "structures#src-import"];

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

#[track_caller]
fn expect_code(d: &Diagnostics, code: DiagCode) {
    let found: Vec<&str> = d.errors().map(|e| e.code.as_str()).collect();
    assert!(
        found.contains(&code.as_str()),
        "expected {}, got {found:?}:\n{d}",
        code.as_str()
    );
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

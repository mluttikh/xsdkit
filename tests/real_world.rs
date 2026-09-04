//! Regression tests against a real, shipping schema.
//!
//! Synthetic schemas exercise features one at a time. This one exercises the
//! combinations that actually break implementations.

use xsdkit::model::Term;
use xsdkit::*;

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn schema_for_schemas() -> (Schemas, Diagnostics) {
    SchemaSetBuilder::new()
        .file("tests/fixtures/XMLSchema.xsd")
        .conformance(Conformance::Lax)
        .build_with_warnings()
}

#[test]
fn the_schema_for_schemas_loads() {
    let (s, d) = schema_for_schemas();

    // Exactly one diagnostic: the xml.xsd import we refuse to fetch over the
    // network and do not need, because the xml: attributes are predeclared.
    let noise: Vec<_> = d
        .iter()
        .filter(|x| x.code != DiagCode::UnresolvedSchemaLocation)
        .collect();
    assert!(noise.is_empty(), "unexpected diagnostics:\n{d}");

    assert!(
        s.globals().elements.len() > 30,
        "expected the top-level XSD vocabulary"
    );
    assert!(s.element(Some(XS), "schema").is_some());
    assert!(s.element(Some(XS), "element").is_some());
    assert!(s.element(Some(XS), "complexType").is_some());
}

/// An internal DTD subset must not stop the parse. Every real XSD toolchain
/// hits this, and the W3C's own schema is the first document to trip it.
#[test]
fn an_internal_dtd_subset_is_accepted() {
    let (s, _) = schema_for_schemas();
    assert_eq!(s.documents().len(), 1);
    assert!(s.component_counts().types > 100);
}

/// `xml:` is bound by the Namespaces spec, not by any declaration. A schema
/// using `xml:lang` must resolve it without importing `xml.xsd`.
#[test]
fn the_xml_prefix_resolves_without_an_import() {
    let (_, d) = schema_for_schemas();
    assert!(
        !d.iter()
            .any(|x| x.message.contains("xml") && x.code == DiagCode::InvalidAttributeValue),
        "the xml: prefix must be implicitly bound:\n{d}"
    );

    let (s, _) = schema_for_schemas();
    let lang = s.attribute(Some("http://www.w3.org/XML/1998/namespace"), "lang");
    assert!(lang.is_some(), "xml:lang should be predeclared");
}

/// The schema-for-schemas declares all 50 built-ins itself. Ours must win
/// without that being reported as a duplicate.
#[test]
fn redeclaring_the_builtins_is_not_a_duplicate() {
    let (s, d) = schema_for_schemas();
    assert!(
        !d.iter().any(|x| x.code == DiagCode::DuplicateGlobal),
        "built-in redeclaration must not collide:\n{d}"
    );
    // The built-in handle still points at our component, so `builtin()` and
    // a `type_()` lookup agree.
    assert_eq!(
        s.type_(Some(XS), "string"),
        Some(s.builtin(xsdkit::datatypes::Builtin::String))
    );
}

#[test]
fn named_model_groups_resolve_through_references() {
    let (s, _) = schema_for_schemas();
    assert!(s.component_counts().model_groups >= 10);

    // Every xs:group ref must have found its definition; a survivor would
    // have been pruned, so counting them proves resolution ran.
    let group_refs = s
        .iter_particles()
        .filter(|(_, p)| matches!(p.term, Term::GroupRef(_)))
        .count();
    assert!(
        group_refs > 0,
        "the schema-for-schemas references named groups"
    );

    for (_, p) in s.iter_particles() {
        if let Term::GroupRef(g) = p.term {
            // Dereferencing must not panic and must reach real particles.
            let _ = &s[g].group.compositor;
        }
    }
}

#[test]
fn identity_constraints_resolve() {
    let (s, _) = schema_for_schemas();
    let n = s.component_counts().identity_constraints;
    assert!(
        n >= 5,
        "the schema-for-schemas declares keys and keyrefs, found {n}"
    );

    // Every keyref must point at a key that exists.
    for (_, idc) in s.iter_identity_constraints() {
        if idc.kind == IdcKind::KeyRef {
            let target = idc.refer.expect("keyref must resolve to a key");
            assert_eq!(s[target].kind, IdcKind::Key);
        }
    }
}

/// `xs:element` has type `xs:topLevelElement`, whose content contains
/// `xs:element` — a genuine cycle. Walking it must terminate.
#[test]
fn recursive_types_do_not_hang() {
    let (s, _) = schema_for_schemas();
    let element = s.element(Some(XS), "element").expect("xs:element");
    let ty = s[element].type_id;

    // The base chain is bounded even though the content model is cyclic.
    let chain = s.base_chain(ty);
    assert!(
        chain.len() > 1 && chain.len() < 20,
        "chain was {}",
        chain.len()
    );
    assert_eq!(
        *chain.last().unwrap(),
        s.builtin(xsdkit::datatypes::Builtin::AnyType)
    );

    // A bounded walk of the content graph reaches xs:element again.
    let mut seen = std::collections::HashSet::new();
    let mut stack = s[ty]
        .as_complex()
        .and_then(|c| c.content.particle())
        .into_iter()
        .collect::<Vec<_>>();
    let mut steps = 0usize;
    while let Some(p) = stack.pop() {
        steps += 1;
        assert!(steps < 100_000, "content walk did not terminate");
        if !seen.insert(p) {
            continue;
        }
        stack.extend(s.child_particles(p));
        if let Term::Element(e) = s[p].term {
            if let Some(cp) = s[s[e].type_id]
                .as_complex()
                .and_then(|c| c.content.particle())
            {
                stack.push(cp);
            }
        }
    }
    assert!(
        seen.len() > 20,
        "expected a substantial content graph, saw {}",
        seen.len()
    );
}

#[test]
fn appinfo_survives_a_real_schema() {
    let (s, _) = schema_for_schemas();
    let with_appinfo = s
        .iter_annotations()
        .filter(|(_, a)| !a.appinfo.is_empty())
        .count();
    assert!(with_appinfo > 0, "appinfo must be kept for the units layer");
}

#[test]
fn strict_mode_rejects_what_lax_accepts() {
    let strict = SchemaSetBuilder::new()
        .file("tests/fixtures/XMLSchema.xsd")
        .build();
    assert!(
        strict.is_err(),
        "the unfetchable import is an error in strict mode"
    );
}

/// Compiling is not cheap, so `Schemas` must be reusable across queries and
/// cheap to hand around.
#[test]
fn a_compiled_schema_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Schemas>();

    let (s, _) = schema_for_schemas();
    let handle = std::thread::spawn(move || s.globals().types.len());
    assert!(handle.join().unwrap() > 50);
}

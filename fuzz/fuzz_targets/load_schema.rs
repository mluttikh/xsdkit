//! Arbitrary bytes into the schema loader.
//!
//! The largest untrusted surface in the crate: encoding detection, XML
//! parsing, component construction and compilation, all on input nobody
//! wrote on purpose.
//!
//! The interesting assertion is not "it did not panic" but what follows a
//! *successful* build. `Schemas` promises no unresolved reference survives
//! compilation, and its `Index` impls carry a `debug_assert` saying so — but
//! that only fires if something actually walks the model. So this target
//! walks all of it. That is precisely the shape of bug the W3C suite found:
//! a dangling content particle that built fine and panicked on first use.
#![no_main]

use libfuzzer_sys::fuzz_target;
use xsdkit::{Compilation, Conformance, SchemaSetBuilder, Version};

fuzz_target!(|data: &[u8]| {
    for version in [Version::Xsd10, Version::Xsd11] {
        let Compilation { schemas, .. } = SchemaSetBuilder::new()
            .version(version)
            .conformance(Conformance::Lax)
            // Bound the work: a fuzzer will happily find the deepest legal
            // nesting and sit there.
            .nodes_limit(20_000)
            .bytes(data.to_vec(), "fuzz://input.xsd")
            .compile();

        walk_everything(&schemas);
    }
});

/// Touches every component the model exposes, so a placeholder that survived
/// compilation is found here rather than by a user.
fn walk_everything(schemas: &xsdkit::Schemas) {
    for (id, def) in schemas.iter_types() {
        let _ = def.name().map(|n| schemas.display_name(n));
        let _ = schemas.base_chain(id);
        // Index the id-bearing fields *directly*. `base_chain` guards against
        // a placeholder and stops, so it cannot see one; indexing is what
        // trips the debug assertion. Three separate bugs have been a field
        // nothing indexed — a simple type's base, a list's item type, a
        // union's members — so every new one belongs here.
        let _ = &schemas[def.base()];
        if let Some(t) = def.as_simple() {
            if let Some(item) = t.item_type {
                let _ = &schemas[item];
            }
            for m in &t.member_types {
                let _ = &schemas[*m];
            }
        }
        if let Some(c) = def.as_complex() {
            for u in &c.attribute_uses {
                let _ = &schemas[u.attribute];
            }
            if let xsdkit::model::ContentType::Simple(t) = c.content {
                let _ = &schemas[t];
            }
        }
        let _ = schemas.attribute_uses(id);
        let _ = schemas.content(id);
        // `children()` answers for the whole type what the three singular
        // predicates answer one child at a time, and by a different
        // algorithm: one SCC pass and a dataflow over bitsets, rather than an
        // automaton walk per child. Two implementations of one question make
        // a differential oracle — the only thing in this target that can
        // catch a wrong *answer* rather than a panic. The same assertion runs
        // over the W3C suite; here it runs over schemas nobody wrote.
        let batched = schemas.children(id);
        assert_eq!(
            batched.iter().map(|c| c.element).collect::<Vec<_>>(),
            schemas.possible_children(id),
            "children() and possible_children() disagree on the children of a type",
        );
        for c in &batched {
            assert_eq!(
                (c.repeats, c.optional),
                (
                    schemas.child_repeats(id, c.element),
                    schemas.child_is_optional(id, c.element),
                ),
                "children() and the singular predicates disagree on an occurrence",
            );
        }
        if let Some(mut m) = schemas.match_content(id) {
            // A step with a name the schema knows, then end — enough to walk
            // the automaton's transitions.
            if let Some((name, _)) = schemas.globals().elements.iter().next() {
                let _ = m.step(*name);
            }
            let _ = m.accepts_end();
        }
    }
    for (id, e) in schemas.iter_elements() {
        let _ = schemas.display_name(e.name);
        // Membership and what may *actually* substitute are two walks:
        // `permitted_substitutes` applies `block` on top of the closure, and
        // it is the one the content model agrees with.
        let _ = schemas.substitution_group(id);
        let _ = schemas.permitted_substitutes(id);
        let _ = schemas[e.type_id].name();
    }
    for (_, a) in schemas.iter_attributes() {
        let _ = schemas[a.type_id].name();
    }
    for (id, _) in schemas.iter_particles() {
        let _ = schemas.child_particles(id);
    }
    for (_, idc) in schemas.iter_identity_constraints() {
        if let Some(r) = idc.refer {
            let _ = schemas[r].kind;
        }
    }
    let _ = schemas.content_stats();
    let _ = schemas.component_counts();

    // The value layer, over every simple type the schema declares.
    let v = schemas.value_validator();
    let _ = v.pattern_errors();
    for (id, def) in schemas.iter_types() {
        if def.is_simple() {
            let _ = v.validate(id, "0");
            let _ = v.validate(id, "");
            let _ = v.effective_facets(id);
        }
    }
}

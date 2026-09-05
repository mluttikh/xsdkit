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
use xsdkit::{Conformance, SchemaSetBuilder, Version};

fuzz_target!(|data: &[u8]| {
    for version in [Version::Xsd10, Version::Xsd11] {
        let (schemas, _diags) = SchemaSetBuilder::new()
            .version(version)
            .conformance(Conformance::Lax)
            // Bound the work: a fuzzer will happily find the deepest legal
            // nesting and sit there.
            .nodes_limit(20_000)
            .bytes(data.to_vec(), "fuzz://input.xsd")
            .build_with_warnings();

        walk_everything(&schemas);
    }
});

/// Touches every component the model exposes, so a placeholder that survived
/// compilation is found here rather than by a user.
fn walk_everything(schemas: &xsdkit::Schemas) {
    for (id, def) in schemas.iter_types() {
        let _ = def.name().map(|n| schemas.display_name(n));
        let _ = schemas.base_chain(id);
        let _ = schemas.attribute_uses(id);
        let _ = schemas.content(id);
        for child in schemas.possible_children(id) {
            let _ = schemas.child_repeats(id, child);
            let _ = schemas.child_is_optional(id, child);
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
        let _ = schemas.substitution_closure(id);
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
    let v = schemas.validator();
    let _ = v.pattern_errors();
    for (id, def) in schemas.iter_types() {
        if def.is_simple() {
            let _ = v.validate(id, "0");
            let _ = v.validate(id, "");
            let _ = v.effective_facets(id);
        }
    }
}

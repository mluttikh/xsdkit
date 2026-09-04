//! Extracting units of measure from a schema — a recipe, not a feature.
//!
//! ```text
//! cargo run --example units -- schema.xsd [--search DIR]
//! ```
//!
//! There is no standard for where a unit lives in an XSD. Units were proposed
//! to the XML Schema Working Group in 1999 as a datatype facet and not
//! adopted; UnitsML has been an OASIS Committee Specification Draft since
//! 2011. What *is* standardised is the vocabulary you write in the slot —
//! UCUM for GML and HL7, UN/CEFACT Rec. 20 for UBL and e-invoicing — never
//! the slot itself.
//!
//! So `xsdkit` deliberately has no units layer. It exposes attributes, their
//! fixed values, their enumerations and their annotations, and forty lines
//! like these turn that into whatever convention your schemas actually use.
//! A built-in heuristic would be right most of the time and silently wrong
//! the rest, which is the worst outcome for a schema reader.

use xsdkit::*;

/// Attribute names this schema family uses for a unit. The one thing you
/// must supply, because it is the one thing no standard fixes.
const UNIT_ATTRIBUTES: &[&str] = &["uom", "unit", "units", "unitcode"];

/// Where a value's unit comes from.
#[derive(Debug)]
enum Binding {
    /// The schema pins it. Known without reading any document — and the only
    /// shape that can compile to a constant scale/offset.
    Fixed(String),
    /// The schema constrains it to a vocabulary, but the document chooses.
    Enumerated(Vec<String>),
    /// The document carries it, unconstrained.
    PerInstance(String),
}

fn binding(schemas: &Schemas, ty: TypeId) -> Option<Binding> {
    for use_ in schemas.attribute_uses(ty) {
        let decl = &schemas[use_.attribute];
        let local = schemas.names().resolve(decl.name.local).to_lowercase();
        if !UNIT_ATTRIBUTES.contains(&local.as_str()) {
            continue;
        }

        // A `fixed` value may be declared here or inherited; `attribute_uses`
        // has already folded the base chain, so both look the same.
        let constraint = use_
            .value_constraint
            .as_ref()
            .or(decl.value_constraint.as_ref());
        if let Some(vc) = constraint.filter(|c| c.is_fixed()) {
            return Some(Binding::Fixed(vc.value().to_string()));
        }

        if let Some(enumeration) = schemas[decl.type_id]
            .as_simple()
            .and_then(|t| t.facets.enumeration.clone())
        {
            return Some(Binding::Enumerated(enumeration));
        }

        return Some(Binding::PerInstance(local));
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut builder = SchemaSetBuilder::new().conformance(Conformance::Lax);
    let mut files = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--search" => {
                i += 1;
                builder = builder.search_path(&args[i]);
            }
            f => files.push(f.to_string()),
        }
        i += 1;
    }
    if files.is_empty() {
        eprintln!("usage: units <schema.xsd>... [--search DIR]");
        std::process::exit(2);
    }
    for f in &files {
        builder = builder.file(f);
    }

    let (schemas, _) = builder.build_with_warnings();

    let mut fixed = 0;
    let mut enumerated = 0;
    let mut per_instance = 0;
    for (id, def) in schemas.iter_types() {
        let Some(b) = binding(&schemas, id) else {
            continue;
        };
        let name = def
            .name()
            .map(|n| schemas.display_name(n))
            .unwrap_or_else(|| "(anonymous)".into());
        match &b {
            Binding::Fixed(u) => {
                fixed += 1;
                println!("{name:50} fixed        {u}");
            }
            Binding::Enumerated(vs) => {
                enumerated += 1;
                let shown: Vec<_> = vs.iter().take(5).cloned().collect();
                println!(
                    "{name:50} enumerated   {shown:?}{}",
                    if vs.len() > 5 { " …" } else { "" }
                );
            }
            Binding::PerInstance(a) => {
                per_instance += 1;
                println!("{name:50} per-instance @{a}");
            }
        }
    }

    println!("\n{fixed} fixed, {enumerated} enumerated, {per_instance} per-instance");
    if fixed > 0 {
        println!("Only the fixed ones can become a constant scale/offset.");
    }
}

//! Loads a schema and prints what came out of it.
//!
//! ```text
//! cargo run --example inspect -- schema.xsd [--lax] [--search DIR]
//! ```

use xsdkit::model::Term;
use xsdkit::*;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut files = Vec::new();
    let mut builder = SchemaSetBuilder::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lax" => builder = builder.conformance(Conformance::Lax),
            "--search" => {
                i += 1;
                builder = builder.search_path(&args[i]);
            }
            f => files.push(f.to_string()),
        }
        i += 1;
    }
    if files.is_empty() {
        eprintln!("usage: inspect <schema.xsd>... [--lax] [--search DIR]");
        std::process::exit(2);
    }
    for f in &files {
        builder = builder.file(f);
    }

    let (schemas, diags) = builder.build_with_warnings();

    let c = schemas.component_counts();
    println!("documents           {}", schemas.documents().len());
    for d in schemas.documents() {
        let ns = d
            .target_namespace
            .map(|n| schemas.names().resolve_ns(n).to_string())
            .unwrap_or_else(|| "(none)".into());
        let cham = if d.chameleon { "  [chameleon]" } else { "" };
        println!("  {ns}{cham}\n    {}", d.uri);
    }

    println!("\ntypes               {}", c.types);
    println!("elements            {}", c.elements);
    println!("attributes          {}", c.attributes);
    println!("particles           {}", c.particles);
    println!("model groups        {}", c.model_groups);
    println!("attribute groups    {}", c.attribute_groups);
    println!("identity constraint {}", c.identity_constraints);
    println!("annotations         {}", c.annotations);

    let g = schemas.globals();
    println!("\nglobal elements     {}", g.elements.len());
    println!("global types        {}", g.types.len());

    // The two questions the future config generator is built on.
    let mut repeating = 0usize;
    let mut optional = 0usize;
    for (_, p) in schemas.iter_particles() {
        if p.is_repeating() {
            repeating += 1;
        }
        if p.is_optional() {
            optional += 1;
        }
        let _ = &p.term;
    }
    println!("\nrepeating particles {repeating}  (candidate tables)");
    println!("optional particles  {optional}  (candidate nullable columns)");

    let heads: Vec<_> = schemas
        .iter_elements()
        .filter(|(id, _)| schemas.substitution_closure(*id).len() > 1)
        .collect();
    if !heads.is_empty() {
        println!("\nsubstitution heads  {}", heads.len());
        for (id, e) in heads.iter().take(5) {
            println!(
                "  {} -> {} member(s)",
                schemas.display_name(e.name),
                schemas.substitution_closure(*id).len()
            );
        }
    }

    let with_appinfo = schemas
        .iter_annotations()
        .filter(|(_, a)| !a.appinfo.is_empty())
        .count();
    println!("\nannotations w/ appinfo {with_appinfo}  (units-layer candidates)");

    if !diags.is_empty() {
        println!("\n--- {} diagnostic(s) ---", diags.len());
        for d in diags.iter().take(15) {
            println!("{d}");
        }
        if diags.len() > 15 {
            println!("... and {} more", diags.len() - 15);
        }
    }

    // Keep the Term import honest.
    let _ = |t: &Term| matches!(t, Term::Element(_));
}

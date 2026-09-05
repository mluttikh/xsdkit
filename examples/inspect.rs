//! Loads a schema and prints what came out of it.
//!
//! ```text
//! cargo run --example inspect -- schema.xsd [--lax] [--11] [--search DIR]
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
            "--11" => builder = builder.version(Version::Xsd11),
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

    let cs = schemas.content_stats();
    println!("\ncontent models      {}", cs.models);
    println!(
        "  automata          {}  ({} positions)",
        cs.automata, cs.positions
    );
    println!("  xs:all groups     {}", cs.all_groups);
    println!("  empty             {}", cs.empty);
    if cs.approximated > 0 {
        println!(
            "  approximated      {}  (occurrence ranges widened)",
            cs.approximated
        );
    }

    // The two questions the future config generator is built on, answered
    // from the automata rather than guessed from the particle tree.
    let mut tables = 0usize;
    let mut nullable = 0usize;
    let mut columns = 0usize;
    for (tid, def) in schemas.iter_types() {
        if def.as_complex().is_none() {
            continue;
        }
        for child in schemas.possible_children(tid) {
            columns += 1;
            if schemas.child_repeats(tid, child) {
                tables += 1;
            }
            if schemas.child_is_optional(tid, child) {
                nullable += 1;
            }
        }
    }
    println!("\nparent/child pairs  {columns}");
    println!("  repeating         {tables}  (candidate tables)");
    println!("  optional          {nullable}  (candidate nullable columns)");

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
    println!("\nannotations w/ appinfo {with_appinfo}");

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

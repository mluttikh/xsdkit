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
            .map(|n| schemas.names().resolve_ns(n))
            .unwrap_or("(none)");
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
    // from the automata rather than guessed from the particle tree — and
    // answered together, because each of them costs a walk of the content
    // model and one walk answers both.
    let mut tables = 0usize;
    let mut nullable = 0usize;
    let mut columns = 0usize;
    for (tid, def) in schemas.iter_types() {
        if def.as_complex().is_none() {
            continue;
        }
        for child in schemas.get(tid).children() {
            columns += 1;
            tables += usize::from(child.repeats());
            nullable += usize::from(child.optional());
        }
    }
    println!("\nparent/child pairs  {columns}");
    println!("  repeating         {tables}  (candidate tables)");
    println!("  optional          {nullable}  (candidate nullable columns)");

    // One closure per head, not one to filter and another to print.
    let mut heads: Vec<(String, usize)> = schemas
        .iter_elements()
        .filter_map(|(id, _)| {
            let e = schemas.get(id);
            let members = e.substitutes().count();
            (members > 1).then(|| (e.display_name(), members))
        })
        .collect();
    if !heads.is_empty() {
        println!("\nsubstitution heads  {}", heads.len());
        heads.sort();
        for (name, members) in heads.iter().take(5) {
            println!("  {name} -> {members} member(s)");
        }
    }

    // What browsing the schema actually looks like: a reference is a borrow
    // and an id, so following it allocates nothing.
    if let Some(root) = schemas.global_elements().find(|e| !e.is_abstract()) {
        println!("\n{}", root.display_name());
        for a in root.attributes() {
            println!(
                "  @{}{}: {}",
                a.local_name(),
                if a.is_required() { "" } else { "?" },
                a.type_of().display_name()
            );
        }
        for c in root.children() {
            println!(
                "  {}{}{}: {}",
                c.local_name(),
                if c.repeats() { "+" } else { "" },
                if c.optional() { "?" } else { "" },
                c.type_of().display_name()
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

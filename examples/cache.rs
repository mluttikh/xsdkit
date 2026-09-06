//! Measures what the `serde` feature buys: compile once, load thereafter.
//!
//! Point it at your own schema — whether caching pays depends entirely on
//! how big that is:
//!
//! ```text
//! cargo run --release --features serde --example cache -- main.xsd [search/path ...]
//! ```

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: cache <schema.xsd> [search path ...]");

    let t = std::time::Instant::now();
    let mut builder = xsdkit::SchemaSetBuilder::new();
    for dir in args {
        builder = builder.search_path(dir);
    }
    let schemas = builder
        .file(&path)
        .compile()
        .into_result()
        .unwrap_or_else(|d| panic!("{d}"));
    let compile = t.elapsed();

    let t = std::time::Instant::now();
    let bytes = postcard::to_allocvec(&schemas).unwrap();
    let write = t.elapsed();

    let t = std::time::Instant::now();
    let copy: xsdkit::Schemas = postcard::from_bytes(&bytes).unwrap();
    let read = t.elapsed();

    let counts = copy.component_counts();
    println!("{path}");
    println!("  compile   {compile:>10.2?}");
    println!("  serialize {write:>10.2?}   {} bytes", bytes.len());
    println!(
        "  load      {read:>10.2?}   {:.1}x faster",
        compile.as_secs_f64() / read.as_secs_f64()
    );
    println!("  {counts:?}");
}

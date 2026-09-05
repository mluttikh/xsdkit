//! Which valid documents does xsdkit reject, and with what?
//!
//! The instance-side counterpart to `w3c_why`. Groups the false alarms by
//! diagnostic code and by the message's shape, so a regression in one datatype
//! shows up as a cluster rather than as a number two lower than yesterday.
//!
//! ```text
//! XSDTESTS=/tmp/xsdtests cargo run --release --example w3c_docs
//! ```
use std::collections::BTreeMap;
use std::path::PathBuf;
use xsdkit::{Conformance, SchemaSetBuilder, Version};

fn main() {
    let root = PathBuf::from(std::env::var("XSDTESTS").expect("XSDTESTS"));
    let filter = std::env::args().nth(1);
    // How many examples to print per code. Raise it to diff two runs against
    // each other, which is how a regression of two documents gets a name.
    let samples: usize = std::env::var("SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let mut files = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() && p.file_name().is_some_and(|n| n != ".git") {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "testSet") {
                files.push(p);
            }
        }
    }
    files.sort();

    let mut by_code: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    let mut cache: BTreeMap<String, Option<xsdkit::Schemas>> = BTreeMap::new();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        };
        let Ok(doc) = roxmltree::Document::parse_with_options(&text, opts) else {
            continue;
        };
        let dir = f.parent().unwrap().to_path_buf();
        for g in doc.descendants().filter(|n| n.has_tag_name("testGroup")) {
            let v = g.attribute("version").unwrap_or("1.0 1.1");
            let version = if v.contains("1.1") && !v.contains("1.0") {
                Version::Xsd11
            } else {
                Version::Xsd10
            };
            let schemas: Vec<PathBuf> = g
                .descendants()
                .filter(|n| n.has_tag_name("schemaDocument"))
                .filter_map(|n| n.attribute(("http://www.w3.org/1999/xlink", "href")))
                .map(|h| dir.join(h))
                .collect();
            if schemas.is_empty() {
                continue;
            }
            for it in g.children().filter(|n| n.has_tag_name("instanceTest")) {
                // Only the documents the suite says must be accepted.
                let expects_valid = it
                    .children()
                    .find(|n| n.has_tag_name("expected"))
                    .and_then(|n| n.attribute("validity"))
                    == Some("valid");
                if !expects_valid {
                    continue;
                }
                let Some(href) = it
                    .children()
                    .find(|n| n.has_tag_name("instanceDocument"))
                    .and_then(|n| n.attribute(("http://www.w3.org/1999/xlink", "href")))
                else {
                    continue;
                };
                let path = dir.join(href);
                let Ok(xml) = std::fs::read_to_string(&path) else {
                    continue;
                };

                let key = format!("{version:?}|{}", schemas[0].display());
                let entry = cache.entry(key).or_insert_with(|| {
                    let mut b = SchemaSetBuilder::new()
                        .version(version)
                        .conformance(Conformance::Lax);
                    if let Some(p) = schemas[0].parent() {
                        b = b.search_path(p);
                    }
                    for d in &schemas {
                        b = b.file(d.display().to_string());
                    }
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let (s, d) = b.build_with_warnings();
                        (!d.has_errors()).then_some(s)
                    }))
                    .unwrap_or(None)
                });
                // A schema we could not load says nothing about the document.
                let Some(s) = entry else { continue };
                let report = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    s.instance_validator().validate(&xml)
                })) {
                    Ok(r) => r,
                    Err(_) => {
                        // Name the document. A panic count with no example is
                        // a number you cannot act on.
                        let e = by_code.entry("PANIC".into()).or_default();
                        e.0 += 1;
                        if e.1.len() < samples {
                            e.1.push(format!("{}  |  panicked", path.display()));
                        }
                        continue;
                    }
                };
                if report.is_valid() {
                    continue;
                }
                for d in report.diagnostics.errors().take(1) {
                    if filter
                        .as_ref()
                        .is_some_and(|f| !d.message.contains(f.as_str()))
                    {
                        continue;
                    }
                    let e = by_code
                        .entry(format!("{} {:?}", d.code, d.code))
                        .or_default();
                    e.0 += 1;
                    if e.1.len() < samples {
                        e.1.push(format!(
                            "{}  |  {}",
                            path.file_name().unwrap().to_string_lossy(),
                            d.message.chars().take(78).collect::<String>()
                        ));
                    }
                }
            }
        }
    }
    std::panic::set_hook(hook);

    let total: usize = by_code.values().map(|(n, _)| n).sum();
    println!("valid documents we reject: {total}\n");
    let mut v: Vec<_> = by_code.into_iter().collect();
    v.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    for (code, (n, samples)) in v {
        println!("{n:5}  {code}");
        for s in samples {
            println!("         {s}");
        }
    }
}

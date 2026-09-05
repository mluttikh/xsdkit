//! Which invalid schemas does xsdkit accept, and what rule would catch them?
//!
//! The mirror of `w3c_why`. That one asks why we reject schemas the suite
//! calls valid; this one asks what we *miss* — the 21.4% figure in the README
//! is entirely unimplemented validity constraints, and this says which ones
//! are worth implementing first.
//!
//! The suite does not label a case with the constraint it tests, so this
//! clusters by test-group name with the trailing digits stripped: the suite's
//! own naming (`particlesZ001`, `particlesZ002`, …) is a family per rule, and
//! a family with fifty misses is fifty cases one implementation buys.
//!
//! ```text
//! XSDTESTS=/tmp/xsdtests cargo run --release --example w3c_gap
//! ```
use std::collections::BTreeMap;
use std::path::PathBuf;
use xsdkit::{Conformance, SchemaSetBuilder, Version};

/// `particlesZ033a` -> `particlesZ`. The suite numbers cases within a family,
/// sometimes with a letter suffix, so drop both from the tail.
fn family(group: &str) -> String {
    let s = group.trim_end_matches(|c: char| c.is_ascii_alphabetic() && c.is_lowercase());
    let s = s.trim_end_matches(|c: char| c.is_ascii_digit());
    if s.is_empty() {
        group.to_string()
    } else {
        s.to_string()
    }
}

fn main() {
    let root = PathBuf::from(std::env::var("XSDTESTS").expect("XSDTESTS"));
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

    // family -> (missed, caught, a few sample documents)
    let mut by_family: BTreeMap<String, (usize, usize, Vec<String>)> = BTreeMap::new();
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
            let version = g.attribute("version").unwrap_or("1.0 1.1");
            let group = g.attribute("name").unwrap_or("?").to_string();
            for st in g.children().filter(|n| n.has_tag_name("schemaTest")) {
                // Only the cases the suite says must be rejected.
                if st
                    .children()
                    .find(|n| n.has_tag_name("expected"))
                    .and_then(|n| n.attribute("validity"))
                    != Some("invalid")
                {
                    continue;
                }
                let docs: Vec<PathBuf> = st
                    .children()
                    .filter(|n| n.has_tag_name("schemaDocument"))
                    .filter_map(|n| n.attribute(("http://www.w3.org/1999/xlink", "href")))
                    .map(|h| dir.join(h))
                    .collect();
                if docs.is_empty() {
                    continue;
                }
                let v = if version.contains("1.1") && !version.contains("1.0") {
                    Version::Xsd11
                } else {
                    Version::Xsd10
                };
                let mut b = SchemaSetBuilder::new()
                    .version(v)
                    .conformance(Conformance::Strict);
                if let Some(p) = docs[0].parent() {
                    b = b.search_path(p);
                }
                for d in &docs {
                    b = b.file(d.display().to_string());
                }
                // A panic is not an acceptance, but it is not a rejection
                // either; count it with the misses so it cannot hide.
                let accepted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    !b.build_with_warnings().1.has_errors()
                }))
                .unwrap_or(true);

                let e = by_family.entry(family(&group)).or_default();
                if accepted {
                    e.0 += 1;
                    if e.2.len() < 3 {
                        e.2.push(format!(
                            "{}  ({})",
                            docs[0].file_name().unwrap().to_string_lossy(),
                            group
                        ));
                    }
                } else {
                    e.1 += 1;
                }
            }
        }
    }
    std::panic::set_hook(hook);

    let missed: usize = by_family.values().map(|(m, _, _)| m).sum();
    let caught: usize = by_family.values().map(|(_, c, _)| c).sum();
    println!(
        "invalid schemas: {caught} rejected, {missed} accepted ({} cases)\n",
        caught + missed
    );
    println!("families by how many cases one rule would buy:\n");
    let mut v: Vec<_> = by_family
        .into_iter()
        .filter(|(_, (m, ..))| *m > 0)
        .collect();
    v.sort_by_key(|(_, (m, ..))| std::cmp::Reverse(*m));
    for (fam, (m, c, samples)) in v {
        println!("{m:4} missed, {c:4} caught   {fam}");
        for s in samples {
            println!("         {s}");
        }
    }
}

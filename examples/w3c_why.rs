//! Why does xsdkit reject schemas the W3C suite says are valid?
use std::collections::BTreeMap;
use std::path::PathBuf;
use xsdkit::{Conformance, SchemaSetBuilder, Version};

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

    let mut by_code: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
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
            for st in g.children().filter(|n| n.has_tag_name("schemaTest")) {
                if st
                    .children()
                    .find(|n| n.has_tag_name("expected"))
                    .and_then(|n| n.attribute("validity"))
                    != Some("valid")
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
                let diags = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    b.build_with_warnings().1
                })) {
                    Ok(d) => d,
                    Err(_) => {
                        let e = by_code.entry("PANIC".into()).or_default();
                        e.0 += 1;
                        if e.1.len() < 3 {
                            e.1.push(docs[0].display().to_string());
                        }
                        continue;
                    }
                };
                if !diags.has_errors() {
                    continue;
                }
                for d in diags.errors().take(1) {
                    let e = by_code
                        .entry(format!("{} {:?}", d.code, d.code))
                        .or_default();
                    e.0 += 1;
                    if e.1.len() < 3 {
                        e.1.push(format!(
                            "{}  |  {}",
                            docs[0].file_name().unwrap().to_string_lossy(),
                            d.message.chars().take(70).collect::<String>()
                        ));
                    }
                }
            }
        }
    }
    let total: usize = by_code.values().map(|(n, _)| n).sum();
    println!("valid schemas we reject: {total}\n");
    let mut v: Vec<_> = by_code.into_iter().collect();
    v.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    for (code, (n, samples)) in v {
        println!("{n:4}  {code}");
        for s in samples {
            println!("        {s}");
        }
    }
}

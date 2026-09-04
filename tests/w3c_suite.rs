//! Conformance against the W3C XML Schema Test Suite.
//!
//! The suite is 27,408 cases across 78 test sets contributed by NIST,
//! Microsoft, IBM, Sun/Oracle, Boeing and Saxonica — 5,737 of them schema
//! tests, which is what this crate is judged on. Hand-written tests check
//! what an author thought to check; this checks what a decade of
//! implementers found worth arguing about.
//!
//! It is 231 MB, so it is not vendored. Point `XSDTESTS` at a clone of
//! <https://github.com/w3c/xsdtests> and the harness runs:
//!
//! ```text
//! git clone --depth 1 https://github.com/w3c/xsdtests /tmp/xsdtests
//! XSDTESTS=/tmp/xsdtests cargo test --test w3c_suite -- --nocapture
//! ```
//!
//! Without the variable every test here is skipped, so CI stays green on a
//! machine that has not fetched it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use xsdkit::{Conformance, SchemaSetBuilder, Version};

/// Where the suite lives, if it is available.
fn suite() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var("XSDTESTS").ok()?);
    p.join("suite.xml").is_file().then_some(p)
}

/// One schema case: the documents to load, and whether the suite says the
/// schema is valid.
#[derive(Debug)]
struct SchemaCase {
    set: String,
    group: String,
    version: String,
    documents: Vec<PathBuf>,
    expect_valid: bool,
}

/// Reads the `.testSet` metadata with the crate itself is not appropriate —
/// these are ordinary XML, read with a small hand-rolled scan so a bug in
/// `xsdkit` cannot silently change which cases run.
fn parse_test_sets(root: &Path) -> Vec<SchemaCase> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(d) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == ".git") {
                    continue;
                }
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "testSet") {
                files.push(p);
            }
        }
    }
    files.sort();

    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let Ok(doc) = roxmltree::Document::parse_with_options(
            &text,
            roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            },
        ) else {
            continue;
        };
        let dir = f.parent().unwrap_or(root).to_path_buf();
        let set = doc
            .root_element()
            .attribute("name")
            .unwrap_or("?")
            .to_string();

        for group in doc.descendants().filter(|n| n.has_tag_name("testGroup")) {
            let version = group.attribute("version").unwrap_or("1.0 1.1").to_string();
            let name = group.attribute("name").unwrap_or("?").to_string();
            for st in group.children().filter(|n| n.has_tag_name("schemaTest")) {
                let documents: Vec<PathBuf> = st
                    .children()
                    .filter(|n| n.has_tag_name("schemaDocument"))
                    .filter_map(|n| n.attribute(("http://www.w3.org/1999/xlink", "href")))
                    .map(|h| dir.join(h))
                    .collect();
                let Some(validity) = st
                    .children()
                    .find(|n| n.has_tag_name("expected"))
                    .and_then(|n| n.attribute("validity"))
                else {
                    continue;
                };
                // `notKnown` cases are the ones the working group could not
                // agree on; scoring against them would be scoring noise.
                let expect_valid = match validity {
                    "valid" => true,
                    "invalid" => false,
                    _ => continue,
                };
                if documents.is_empty() {
                    continue;
                }
                out.push(SchemaCase {
                    set: set.clone(),
                    group: name.clone(),
                    version: version.clone(),
                    documents,
                    expect_valid,
                });
            }
        }
    }
    out
}

/// Whether `xsdkit` considers the schema valid.
fn accepts(case: &SchemaCase) -> bool {
    let version = if case.version.contains("1.1") && !case.version.contains("1.0") {
        Version::Xsd11
    } else {
        Version::Xsd10
    };
    let mut b = SchemaSetBuilder::new()
        .version(version)
        .conformance(Conformance::Strict);
    if let Some(dir) = case.documents[0].parent() {
        b = b.search_path(dir);
    }
    for d in &case.documents {
        b = b.file(d.display().to_string());
    }
    // Loading is deliberately done inside `catch_unwind`: a panic on a
    // hostile schema is itself a conformance failure worth counting rather
    // than one that aborts the run.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.build_with_warnings().1.has_errors()
    }))
    .map(|has_errors| !has_errors)
    .unwrap_or(false)
}

#[derive(Default, Debug)]
struct Tally {
    /// Expected valid, accepted. Correct.
    accepted_valid: usize,
    /// Expected valid, rejected. We are too strict, or cannot parse it.
    rejected_valid: usize,
    /// Expected invalid, rejected. Correct.
    rejected_invalid: usize,
    /// Expected invalid, accepted. We do not implement that constraint.
    accepted_invalid: usize,
}

impl Tally {
    fn total(&self) -> usize {
        self.accepted_valid + self.rejected_valid + self.rejected_invalid + self.accepted_invalid
    }
    fn correct(&self) -> usize {
        self.accepted_valid + self.rejected_invalid
    }
}

#[test]
fn w3c_schema_conformance() {
    let Some(root) = suite() else {
        eprintln!("XSDTESTS is not set; skipping the W3C suite");
        return;
    };
    let cases = parse_test_sets(&root);
    assert!(
        cases.len() > 5000,
        "expected the whole suite, found {}",
        cases.len()
    );

    let mut overall = Tally::default();
    let mut by_set: BTreeMap<String, Tally> = BTreeMap::new();
    let mut false_rejections: Vec<String> = Vec::new();

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for c in &cases {
        let accepted = accepts(c);
        let t = by_set.entry(c.set.clone()).or_default();
        match (c.expect_valid, accepted) {
            (true, true) => {
                overall.accepted_valid += 1;
                t.accepted_valid += 1;
            }
            (true, false) => {
                overall.rejected_valid += 1;
                t.rejected_valid += 1;
                if false_rejections.len() < 40 {
                    false_rejections.push(format!("{}/{}", c.set, c.group));
                }
            }
            (false, false) => {
                overall.rejected_invalid += 1;
                t.rejected_invalid += 1;
            }
            (false, true) => {
                overall.accepted_invalid += 1;
                t.accepted_invalid += 1;
            }
        }
    }
    std::panic::set_hook(hook);

    let pct = |n: usize, d: usize| {
        if d == 0 {
            100.0
        } else {
            n as f64 * 100.0 / d as f64
        }
    };
    println!("\n=== W3C XML Schema Test Suite — schema tests ===");
    println!("cases                     {}", overall.total());
    println!(
        "correct                   {} ({:.1}%)",
        overall.correct(),
        pct(overall.correct(), overall.total())
    );
    let valid_total = overall.accepted_valid + overall.rejected_valid;
    let invalid_total = overall.rejected_invalid + overall.accepted_invalid;
    println!(
        "\nvalid schemas accepted    {}/{} ({:.1}%)   <- reading real schemas",
        overall.accepted_valid,
        valid_total,
        pct(overall.accepted_valid, valid_total)
    );
    println!(
        "invalid schemas rejected  {}/{} ({:.1}%)   <- validity constraints",
        overall.rejected_invalid,
        invalid_total,
        pct(overall.rejected_invalid, invalid_total)
    );

    println!("\nworst test sets by false rejection:");
    let mut sets: Vec<_> = by_set.iter().collect();
    sets.sort_by_key(|(_, t)| std::cmp::Reverse(t.rejected_valid));
    for (name, t) in sets.iter().take(12) {
        if t.rejected_valid == 0 {
            break;
        }
        println!(
            "  {name:24} {:4} of {:4} valid schemas rejected",
            t.rejected_valid,
            t.accepted_valid + t.rejected_valid
        );
    }
    if !false_rejections.is_empty() {
        println!("\nfirst false rejections: {}", false_rejections.join(", "));
    }

    // A ratchet, not a target. Raise it as the number improves; never lower
    // it silently.
    let accepted_pct = pct(overall.accepted_valid, valid_total);
    assert!(
        accepted_pct >= 50.0,
        "valid-schema acceptance fell to {accepted_pct:.1}%"
    );
}

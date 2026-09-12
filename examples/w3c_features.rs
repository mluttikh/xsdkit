//! How does xsdkit do on each XSD 1.1 feature, by the suite's own taxonomy?
//!
//! `w3c_gap` clusters what we miss by test-group name, which is a guess at
//! where the rules are. This needs no guess: the suite ships a feature
//! taxonomy in `XSD1_1TestCategories.xml` — 17 features broken into 99
//! categories — and each test group points at the categories it exercises.
//! 94 of the 99 are referenced by at least one group, so this is a per-feature
//! conformance table that has been sitting in metadata the harness parsed and
//! threw away.
//!
//! The taxonomy is the authority on what a category is, because the references
//! do not all agree with it: of 113 distinct fragments referenced, 94 are
//! declared categories, 7 name a feature directly, and 12 are declared as
//! neither — including `xsd1_1-Assertions-SimpleTypesr` and
//! `xsd1_1-Assertions-SimplexTypes`, which are one category and two typos.
//! Those are reported apart rather than folded in, or a misspelling becomes a
//! feature with its own score.
//!
//! ```text
//! XSDTESTS=/tmp/xsdtests cargo run --release --example w3c_features
//! ```
//!
//! Read it as a map of the 1.1 surface rather than as a score. A category at
//! 0/3 is a feature that is not there; one at 8/9 is a feature with a bug in
//! it, and the second is much cheaper to fix.
use std::collections::{BTreeMap, BTreeSet};

#[path = "../tests/suite/mod.rs"]
mod suite;
use suite::*;

/// One line of the report.
#[derive(Default)]
struct Score {
    schema_ok: usize,
    schema_total: usize,
    doc_ok: usize,
    doc_total: usize,
}

/// The taxonomy: every declared category, and the feature it sits under.
///
/// Read from the suite rather than inferred from the category names, because a
/// name is not a reliable parse of its own hierarchy — `xsd1_1-ID-IDREF-*`
/// would split to a feature called `ID`, and the feature is `ID-IDREF`.
fn taxonomy(root: &std::path::Path) -> (BTreeMap<String, String>, BTreeSet<String>) {
    let path = root.join("XSD1_1TestCategories.xml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let doc = roxmltree::Document::parse(&text).expect("the suite's own taxonomy");
    let mut categories = BTreeMap::new();
    let mut features = BTreeSet::new();
    for feature in doc.descendants().filter(|n| n.has_tag_name("test-feature")) {
        let Some(name) = feature.attribute("name") else {
            continue;
        };
        features.insert(name.to_string());
        for c in feature
            .children()
            .filter(|n| n.has_tag_name("feature-category"))
        {
            if let Some(cat) = c.attribute("name") {
                categories.insert(cat.to_string(), name.to_string());
            }
        }
    }
    (categories, features)
}

fn main() {
    let Some(root) = suite_root() else {
        eprintln!("XSDTESTS is not set; nothing to report");
        return;
    };
    let (schema_cases, instance_cases) = parse_test_sets(&root);
    let (categories, features) = taxonomy(&root);

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));

    let mut by_category: BTreeMap<String, Score> = BTreeMap::new();
    for case in &schema_cases {
        if case.features.is_empty() {
            continue;
        }
        let outcome = schema_outcome(case);
        if !outcome.scored() {
            continue;
        }
        let correct = outcome.accepted() == case.expect_valid;
        for f in &case.features {
            let s = by_category.entry(f.clone()).or_default();
            s.schema_total += 1;
            s.schema_ok += usize::from(correct);
        }
    }

    // The document half matters more for some features than the schema half:
    // an assertion that is stored and never evaluated builds a perfectly good
    // schema and then fails to reject anything.
    let mut cache = BTreeMap::new();
    for case in &instance_cases {
        if case.features.is_empty() {
            continue;
        }
        let outcome = instance_outcome(case, &mut cache);
        if !outcome.scored() {
            continue;
        }
        let correct = outcome.accepted() == case.expect_valid;
        for f in &case.features {
            let s = by_category.entry(f.clone()).or_default();
            s.doc_total += 1;
            s.doc_ok += usize::from(correct);
        }
    }
    std::panic::set_hook(hook);

    let pct = |n: usize, d: usize| {
        if d == 0 {
            None
        } else {
            Some(n as f64 * 100.0 / d as f64)
        }
    };
    let show = |n: usize, d: usize| match pct(n, d) {
        None => "      -".to_string(),
        Some(p) => format!("{n:4}/{d:<4} {p:3.0}%"),
    };

    println!("\n=== XSD 1.1 features, by the suite's own categories ===\n");
    println!("{:<54} {:>14} {:>14}", "category", "schemas", "documents");

    // Grouped by the feature the taxonomy puts each category under, so the
    // order is the taxonomy's rather than alphabetical.
    let mut last = String::new();
    let mut totals: BTreeMap<String, Score> = BTreeMap::new();
    let mut undeclared: Vec<(&String, &Score)> = Vec::new();
    let mut feature_level: Vec<(&String, &Score)> = Vec::new();
    let mut in_taxonomy: Vec<(&String, &String, &Score)> = Vec::new();
    for (category, s) in &by_category {
        match categories.get(category) {
            Some(feature) => in_taxonomy.push((category, feature, s)),
            None if features.contains(category) => feature_level.push((category, s)),
            None => undeclared.push((category, s)),
        }
    }
    in_taxonomy.sort_by(|a, b| (a.1, a.0).cmp(&(b.1, b.0)));
    for (category, feature, s) in &in_taxonomy {
        if *feature != &last {
            println!();
            last = (*feature).clone();
        }
        let t = totals.entry((*feature).clone()).or_default();
        t.schema_ok += s.schema_ok;
        t.schema_total += s.schema_total;
        t.doc_ok += s.doc_ok;
        t.doc_total += s.doc_total;
        println!(
            "{:<54} {:>14} {:>14}",
            category.trim_start_matches("xsd1_1-"),
            show(s.schema_ok, s.schema_total),
            show(s.doc_ok, s.doc_total)
        );
    }

    println!("\n\n=== by feature ===\n");
    let mut rows: Vec<_> = totals.iter().collect();
    rows.sort_by(|(_, a), (_, b)| {
        let key = |s: &Score| {
            let n = s.schema_ok + s.doc_ok;
            let d = s.schema_total + s.doc_total;
            pct(n, d).unwrap_or(100.0)
        };
        key(a).partial_cmp(&key(b)).expect("no NaN")
    });
    for (feature, s) in rows {
        println!(
            "{:<54} {:>14} {:>14}",
            feature,
            show(s.schema_ok, s.schema_total),
            show(s.doc_ok, s.doc_total)
        );
    }
    println!(
        "\n{} of the taxonomy's {} categories are referenced by a test group.",
        in_taxonomy.len(),
        categories.len()
    );
    // The suite's references do not all land on the taxonomy. Kept out of the
    // table above and printed here, because a typo with a score looks like a
    // feature.
    for (label, list) in [
        (
            "named at the feature level rather than a category",
            &feature_level,
        ),
        (
            "referenced but declared nowhere in the taxonomy",
            &undeclared,
        ),
    ] {
        if list.is_empty() {
            continue;
        }
        println!("\n{} ({}):", label, list.len());
        for (name, s) in list {
            println!(
                "  {:<52} {:>14} {:>14}",
                name.trim_start_matches("xsd1_1-"),
                show(s.schema_ok, s.schema_total),
                show(s.doc_ok, s.doc_total)
            );
        }
    }
}

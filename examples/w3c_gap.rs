//! Which invalid schemas does xsdkit accept, and what rule would catch them?
//!
//! The mirror of `w3c_why`. That one asks why we reject schemas the suite
//! calls valid; this one asks what we *miss* — the gap between the two
//! conformance figures is entirely unimplemented validity constraints, and
//! this says which ones are worth implementing first.
//!
//! The suite does not label a case with the constraint it tests, so this
//! clusters by test-group name with the trailing digits stripped: the suite's
//! own naming (`particlesZ001`, `particlesZ002`, …) is a family per rule, and
//! a family with fifty misses is fifty cases one implementation buys.
//!
//! ```text
//! XSDTESTS=/tmp/xsdtests cargo run --release --example w3c_gap
//! ```
//!
//! Case selection comes from `tests/suite/mod.rs`, shared with
//! `tests/w3c_suite.rs`, so this counts what the gate counts. It did not
//! always: reading the first `<expected>` regardless of version made this
//! report 476 cases where the test scored 480.
use std::collections::BTreeMap;

#[path = "../tests/suite/mod.rs"]
mod suite;
use suite::*;

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
    let Some(root) = suite_root() else {
        eprintln!("XSDTESTS is not set; nothing to report");
        return;
    };
    let (cases, _) = parse_test_sets(&root);

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    // family -> (missed, caught, a few sample documents)
    let mut by_family: BTreeMap<String, (usize, usize, Vec<String>)> = BTreeMap::new();
    for case in cases.iter().filter(|c| !c.expect_valid) {
        // A panic is not an acceptance, but it is not a rejection either;
        // count it with the misses so it cannot hide. The gate scores it as
        // "did not accept", which is the right call there and the wrong one in
        // a list of work still to do.
        let missed = !matches!(schema_outcome(case), Outcome::Rejected(_));

        let e = by_family.entry(family(&case.group)).or_default();
        if missed {
            e.0 += 1;
            if e.2.len() < 3 {
                e.2.push(format!(
                    "{}  ({})",
                    case.documents[0]
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                    case.group
                ));
            }
        } else {
            e.1 += 1;
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

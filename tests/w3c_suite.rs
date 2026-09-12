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
//! machine that has not fetched it. The `conformance` job does set it: it
//! fetches the single commit pinned in `tests/conformance/SUITE`, which is a
//! few seconds and 16 MB where the checkout is 231 MB.
//!
//! Both halves score themselves against a **committed per-case baseline** in
//! `tests/conformance/`, which is the gate; the percentages are a summary of
//! it. Re-bless after a deliberate change:
//!
//! ```text
//! XSDTESTS=/tmp/xsdtests XSDKIT_BLESS=1 cargo test --test w3c_suite
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use xsdkit::{Conformance, SchemaSetBuilder};

#[path = "suite/mod.rs"]
mod suite;
use suite::*;

/// Where the committed baselines live. Excluded from the published crate: 2.6
/// MB of test data that no downstream build reads.
fn baseline_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("conformance")
        .join(name)
}

/// Compares this run against the committed baseline, or rewrites it under
/// `XSDKIT_BLESS`.
///
/// This is the gate, and the percentages printed alongside it are a summary.
/// A floor on a percentage cannot see three fixes landing beside three
/// regressions; a file with one row per case sees both, and says which cases.
///
/// `rows` are `(key, rest)`: the key identifies the case and the rest is what
/// we did with it, so a case that changed is reported as one `~` line rather
/// than as an unpaired removal and addition forty lines apart.
fn check_baseline(name: &str, summary: &[String], rows: Vec<(String, String)>) {
    let mut rows = rows;
    rows.sort();

    // Keys have to be unique, because the diff pairs a changed case by key and
    // a collapsed pair would hide one side of it. Two instance cases do
    // collide: `introspection` validates the suite's own metadata twice, and
    // `sg-abstract-upa2` lists `e1.xml` twice with *contradictory*
    // expectations — one valid, one invalid. Suffix the later occurrences
    // rather than lose them; a suite quirk should be visible in the file.
    let mut total: BTreeMap<&str, usize> = BTreeMap::new();
    for (key, _) in &rows {
        *total.entry(key.as_str()).or_default() += 1;
    }
    let repeated: BTreeSet<String> = total
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(k, _)| (*k).to_string())
        .collect();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for (key, _) in rows.iter_mut() {
        if repeated.contains(key) {
            let n = seen.entry(key.clone()).or_default();
            *n += 1;
            if *n > 1 {
                key.push_str(&format!("#{n}"));
            }
        }
    }
    rows.sort();

    let mut text = String::new();
    for line in summary {
        if line.is_empty() {
            text.push_str("#\n");
        } else {
            text.push_str("# ");
            text.push_str(line);
            text.push('\n');
        }
    }
    text.push('\n');
    for (key, rest) in &rows {
        text.push_str(key);
        text.push('\t');
        text.push_str(rest);
        text.push('\n');
    }

    let path = baseline_path(name);
    if std::env::var_os("XSDKIT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().expect("baseline has a parent"))
            .expect("create tests/conformance");
        std::fs::write(&path, &text).expect("write the baseline");
        eprintln!("blessed {} — {} rows", path.display(), rows.len());
        return;
    }

    let Ok(committed) = std::fs::read_to_string(&path) else {
        panic!(
            "no baseline at {}. Write it with:\n    \
             XSDTESTS=… XSDKIT_BLESS=1 cargo test --test w3c_suite",
            path.display()
        );
    };
    // A CRLF checkout must not read as a whole-file change. `.gitattributes`
    // pins these to LF, which makes this belt and braces — and belt and braces
    // is right for the one gate that fails on a byte.
    let split = |s: &str| -> Vec<(String, String)> {
        s.lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| match l.split_once('\t') {
                Some((k, rest)) => (k.to_string(), rest.to_string()),
                None => (l.to_string(), String::new()),
            })
            .collect()
    };
    let old: BTreeMap<String, String> = split(&committed).into_iter().collect();
    let new: BTreeMap<String, String> = split(&text).into_iter().collect();
    if old == new {
        return;
    }

    // Tabs are right in the file and wrong in a terminal report.
    let show = |row: &str| row.replace('\t', "  ");
    let mut changed = Vec::new();
    let mut gone = Vec::new();
    let mut added = Vec::new();
    for (key, was) in &old {
        match new.get(key) {
            None => gone.push(format!("  - {key}  ({})", show(was))),
            Some(now) if now != was => changed.push(format!(
                "  ~ {key}\n        was  {}\n        now  {}",
                show(was),
                show(now)
            )),
            Some(_) => {}
        }
    }
    for (key, now) in &new {
        if !old.contains_key(key) {
            added.push(format!("  + {key}  ({})", show(now)));
        }
    }

    let mut report = format!("\n{name} does not match the committed baseline.\n");
    for (label, list) in [
        ("cases whose outcome changed", &changed),
        ("cases no longer in the suite", &gone),
        ("cases new in the suite", &added),
    ] {
        if list.is_empty() {
            continue;
        }
        report.push_str(&format!("\n{} ({}):\n", label, list.len()));
        for line in list.iter().take(40) {
            report.push_str(line);
            report.push('\n');
        }
        if list.len() > 40 {
            report.push_str(&format!("  … and {} more\n", list.len() - 40));
        }
    }
    report.push_str(
        "\nIf every line above is an improvement, re-bless it in the same change:\n    \
         XSDTESTS=… XSDKIT_BLESS=1 cargo test --test w3c_suite\n",
    );
    panic!("{report}");
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

/// `Schemas::children` and the per-child predicates must agree on every
/// complex type the suite can build.
///
/// They are two implementations of one question: the plural form answers it
/// with a components pass and an intersection dataflow over the whole
/// automaton, the singular ones by re-walking the model per child. Synthetic
/// tests pin the shapes that were thought of; this pins the tens of thousands
/// that were not, over real content models that no one wrote for this.
#[test]
fn children_agrees_with_the_predicates_across_the_suite() {
    let Some(root) = suite_root() else {
        eprintln!("XSDTESTS is not set; skipping the W3C suite");
        return;
    };
    let (cases, _) = parse_test_sets(&root);
    let (mut types, mut pairs) = (0usize, 0usize);

    for case in &cases {
        let version = version_of(&case.version);
        let mut b = SchemaSetBuilder::new()
            .version(version)
            .conformance(Conformance::Lax);
        if let Some(dir) = case.documents[0].parent() {
            b = b.search_path(dir);
        }
        for d in &case.documents {
            b = b.file(d.display().to_string());
        }
        // Every case, not just the ones the suite calls valid: a schema with
        // errors still compiles to components, and its content models are as
        // good an oracle as any. A panic is another test's business.
        let Ok(compiled) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.compile()))
        else {
            continue;
        };
        let s = compiled.schemas;

        for (tid, def) in s.iter_types() {
            if def.as_complex().is_none() {
                continue;
            }
            types += 1;
            let plural = s.children(tid);
            assert_eq!(
                plural.iter().map(|c| c.element).collect::<Vec<_>>(),
                s.possible_children(tid),
                "{} disagrees on which children {} has",
                case.group,
                s.display_name(def.name().unwrap_or(xsdkit::QName::UNKNOWN)),
            );
            for c in &plural {
                pairs += 1;
                assert_eq!(
                    (c.repeats, c.optional),
                    (
                        s.child_repeats(tid, c.element),
                        s.child_is_optional(tid, c.element)
                    ),
                    "{} disagrees on {}",
                    case.group,
                    s.display_name(s[c.element].name),
                );
            }
        }
    }
    // A floor, not a target: it exists so a suite that failed to load cannot
    // pass this test by checking nothing.
    assert!(
        pairs > 2_000,
        "expected the whole suite; only {pairs} pairs over {types} types"
    );
    eprintln!("children: {pairs} parent/child pairs over {types} complex types");
}

#[test]
fn w3c_schema_conformance() {
    let Some(root) = suite_root() else {
        eprintln!("XSDTESTS is not set; skipping the W3C suite");
        return;
    };
    let (cases, _) = parse_test_sets(&root);
    assert!(
        cases.len() > 5000,
        "expected the whole suite, found {}",
        cases.len()
    );

    let mut overall = Tally::default();
    let mut by_set: BTreeMap<String, Tally> = BTreeMap::new();
    let mut false_rejections: Vec<String> = Vec::new();
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut unscored = 0usize;

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for c in &cases {
        let outcome = schema_outcome(c);
        rows.push((
            format!("{}/{}", c.set, c.group),
            format!(
                "{}\t{}\t{}",
                version_token(version_of(&c.version)),
                if c.expect_valid { "valid" } else { "invalid" },
                outcome.columns(),
            ),
        ));
        let t = by_set.entry(c.set.clone()).or_default();
        if !outcome.scored() {
            unscored += 1;
            continue;
        }
        match (c.expect_valid, outcome.accepted()) {
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
    println!("not scored (queried)      {unscored}");
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

    // The gate. The floor below is a second, far weaker one; this is the check
    // that fails on one case moving in either direction.
    check_baseline(
        "schema-cases.tsv",
        &[
            "W3C XML Schema Test Suite — one row per schema case.".to_string(),
            String::new(),
            "set/group  version-run  expected  verdict  error-codes".to_string(),
            String::new(),
            format!("cases                     {}", overall.total()),
            format!("not scored (queried)      {unscored}"),
            format!(
                "valid schemas accepted    {}/{}",
                overall.accepted_valid, valid_total
            ),
            format!(
                "invalid schemas rejected  {}/{}",
                overall.rejected_invalid, invalid_total
            ),
            String::new(),
            "Re-bless with XSDKIT_BLESS=1; see tests/w3c_suite.rs.".to_string(),
        ],
        rows,
    );

    // A ratchet, not a target. Raise it as the number improves; never lower
    // it silently. It survives the baseline because it is the one assertion
    // that still means something on a machine whose baseline was blessed
    // against a half-fetched suite.
    let accepted_pct = pct(overall.accepted_valid, valid_total);
    assert!(
        accepted_pct >= 50.0,
        "valid-schema acceptance fell to {accepted_pct:.1}%"
    );
}

/// 21,671 documents, and no longer expensive: 3.3s in `--release` and 19s in a
/// debug build, because the schema cache below turned it from "compile per
/// document" into "compile per group". It was `#[ignore]`d on the strength of
/// a 4.5-minute figure that predates that cache, and ran nowhere as a result.
#[test]
fn w3c_instance_conformance() {
    let Some(root) = suite_root() else {
        eprintln!("XSDTESTS is not set; skipping the W3C suite");
        return;
    };
    let (_, cases) = parse_test_sets(&root);
    assert!(
        cases.len() > 10_000,
        "expected the instance suite, found {}",
        cases.len()
    );

    let mut tally = Tally::default();
    let mut unscored = 0usize;
    let mut by_set: BTreeMap<String, Tally> = BTreeMap::new();
    let mut rows: Vec<(String, String)> = Vec::new();
    // Many groups share one schema, and compiling is the expensive half.
    // "Compile once, validate many" is the crate's own claim; the harness
    // takes it at its word or the run takes ten minutes instead of one.
    let mut cache: BTreeMap<String, Option<xsdkit::Schemas>> = BTreeMap::new();

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for c in &cases {
        let outcome = instance_outcome(c, &mut cache);
        rows.push((
            format!(
                "{}/{}/{}",
                c.set,
                c.group,
                c.instance.file_name().unwrap_or_default().to_string_lossy()
            ),
            format!(
                "{}\t{}\t{}",
                version_token(version_of(&c.version)),
                if c.expect_valid { "valid" } else { "invalid" },
                outcome.columns(),
            ),
        ));
        let t = by_set.entry(c.set.clone()).or_default();
        if !outcome.scored() {
            unscored += 1;
            continue;
        }
        match (c.expect_valid, outcome.accepted()) {
            (true, true) => {
                tally.accepted_valid += 1;
                t.accepted_valid += 1;
            }
            (true, false) => {
                tally.rejected_valid += 1;
                t.rejected_valid += 1;
            }
            (false, false) => {
                tally.rejected_invalid += 1;
                t.rejected_invalid += 1;
            }
            (false, true) => {
                tally.accepted_invalid += 1;
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
    let valid_total = tally.accepted_valid + tally.rejected_valid;
    let invalid_total = tally.rejected_invalid + tally.accepted_invalid;
    println!("\n=== W3C XML Schema Test Suite — instance tests ===");
    println!("cases scored              {}", tally.total());
    println!("not scored                 {unscored}");
    println!(
        "correct                   {} ({:.1}%)",
        tally.correct(),
        pct(tally.correct(), tally.total())
    );
    println!(
        "\nvalid documents accepted  {}/{} ({:.1}%)   <- false alarms",
        tally.accepted_valid,
        valid_total,
        pct(tally.accepted_valid, valid_total)
    );
    println!(
        "invalid documents rejected {}/{} ({:.1}%)   <- what validation catches",
        tally.rejected_invalid,
        invalid_total,
        pct(tally.rejected_invalid, invalid_total)
    );

    println!("\nworst sets by false alarm:");
    let mut sets: Vec<_> = by_set.iter().collect();
    sets.sort_by_key(|(_, t)| std::cmp::Reverse(t.rejected_valid));
    for (name, t) in sets.iter().take(8) {
        if t.rejected_valid == 0 {
            break;
        }
        println!(
            "  {name:24} {:5} of {:5} valid documents rejected",
            t.rejected_valid,
            t.accepted_valid + t.rejected_valid
        );
    }

    check_baseline(
        "instance-cases.tsv",
        &[
            "W3C XML Schema Test Suite — one row per instance case.".to_string(),
            String::new(),
            "set/group/document  version-run  expected  verdict  error-codes".to_string(),
            String::new(),
            format!("cases scored               {}", tally.total()),
            format!("not scored                 {unscored}"),
            format!(
                "valid documents accepted   {}/{}",
                tally.accepted_valid, valid_total
            ),
            format!(
                "invalid documents rejected {}/{}",
                tally.rejected_invalid, invalid_total
            ),
            String::new(),
            "Re-bless with XSDKIT_BLESS=1; see tests/w3c_suite.rs.".to_string(),
        ],
        rows,
    );

    // A ratchet on false alarms: rejecting a valid document is the failure
    // that makes a validator unusable.
    let accepted = pct(tally.accepted_valid, valid_total);
    assert!(
        accepted >= 40.0,
        "valid-document acceptance fell to {accepted:.1}%"
    );
}

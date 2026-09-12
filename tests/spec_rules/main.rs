//! The specification as a checklist, and the joins that keep it true.
//!
//! `tests/conformance/spec-rules.tsv` has one row for each of the 143
//! individually named rules in the two XSD 1.1 Recommendations — Appendix B of
//! each is, in effect, an index of them — and says what this crate does about
//! each one. It exists because **coverage of the specification is not coverage
//! of the code**: 88.9% of regions says nothing at all about a rule nobody
//! wrote, and the W3C suite cannot fill the gap either, since its schema half
//! carries only ~220 negative cases per version to share among 66 Schema
//! Component Constraints.
//!
//! The rule columns are generated (`scripts/extract-spec-rules.py`). The
//! status columns are a judgement, and a judgement rots, so this file is the
//! part that does not depend on anyone's diligence:
//!
//! - a claim needs a site — you cannot write `yes` without saying where;
//! - a non-claim needs a note — you cannot write `no` without saying so;
//! - every site names a file that exists;
//! - **every `DiagCode` is attributable to a rule**, or is listed below as one
//!   of the few that answers to something other than XSD.
//!
//! That last one is the join worth having. A new diagnostic either implements
//! a named rule — in which case the table says which, and the rule's status
//! probably just changed — or it does not, in which case saying why is the
//! useful part.
//!
//! # The corpus
//!
//! [`fixtures`] is the other half: per rule, the smallest schema that violates
//! it and a near-miss that must still load. The W3C suite cannot supply this —
//! about 220 negative schema cases per version, shared among 66 Schema
//! Component Constraints — so a rule can be enforced by a check nobody has
//! ever seen fire. The `fixtures` column of the table says which rules have
//! one, and the tests here keep the column and the corpus in step.

mod fixtures;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use xsdkit::DiagCode;

/// One row of the table.
struct Rule {
    spec: String,
    anchor: String,
    kind: String,
    name: String,
    status: String,
    site: String,
    note: String,
    /// Whether this crate has its own fixtures for the rule, kept in step with
    /// [`fixtures::COVERED`].
    fixtures: bool,
}

impl Rule {
    /// `structures#cos-nonambig`, which is also how the rule is cited in the
    /// specification and in a URL.
    fn id(&self) -> String {
        format!("{}#{}", self.spec, self.anchor)
    }
}

fn rules() -> Vec<Rule> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("conformance")
        .join("spec-rules.tsv");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 8, "expected 8 columns, got {}: {l}", f.len());
            Rule {
                spec: f[0].to_string(),
                anchor: f[1].to_string(),
                kind: f[2].to_string(),
                name: f[3].to_string(),
                status: f[4].to_string(),
                site: f[5].to_string(),
                note: f[6].to_string(),
                fixtures: match f[7] {
                    "fixtures" => true,
                    "-" => false,
                    other => panic!("column 8 is `fixtures` or `-`, got `{other}`"),
                },
            }
        })
        .collect()
}

/// Codes that exist and nothing emits.
///
/// Empty, and worth keeping empty. It was found non-empty by the test below on
/// its first run: `XSD1102` and `XSD1103` had been declared since the loader
/// was written and were never emitted — the chameleon-include machinery was
/// there and the error half was not. Both are now implemented, with fixtures
/// in [`fixtures`], and the suite gained three cases for it.
///
/// An entry here is a promise in the enum that the implementation does not
/// keep, so the list should only ever shrink.
const DEFINED_BUT_UNREACHABLE: &[(DiagCode, &str)] = &[];

/// The codes that are not an XSD rule, and what they answer to instead.
///
/// Kept here rather than as rows in the table, because the table is generated
/// from the RECs and these are not in them. Short by design: a code that
/// belongs here and is not listed is a code nobody has thought about.
const NOT_A_SPEC_RULE: &[(DiagCode, &str)] = &[
    (
        DiagCode::MalformedXml,
        "XML 1.0 well-formedness, before XSD applies",
    ),
    (
        DiagCode::UnsupportedEncoding,
        "the encoding declaration; see src/encoding.rs",
    ),
    (
        DiagCode::MalformedEncoding,
        "bytes that are not valid in their declared encoding",
    ),
    (
        DiagCode::NotASchemaDocument,
        "a root element that is not xs:schema — the schema for schemas, not a named rule",
    ),
    (
        DiagCode::UnknownSchemaElement,
        "an element in the XSD namespace this version does not define; the schema for schemas again",
    ),
    (
        DiagCode::UnresolvedSchemaLocation,
        "a resolver could not fetch a document, which the specification leaves implementation-defined",
    ),
    (
        DiagCode::Unsupported,
        "this crate declining a construct it recognises; the opposite of a rule",
    ),
    (
        DiagCode::MissingAttribute,
        "a required attribute of an XSD element, prescribed by the schema for schemas",
    ),
    (
        DiagCode::MissingElement,
        "a required child of an XSD element, likewise",
    ),
    (
        DiagCode::InvalidAttributeValue,
        "an XSD attribute whose value is not of its declared form, likewise",
    ),
    (
        DiagCode::MisplacedAnnotation,
        "xs:annotation out of position, which the schema for schemas prescribes",
    ),
];

#[test]
fn the_table_covers_every_named_rule() {
    let rules = rules();
    // The RECs are Recommendations and do not change; a change to this number
    // means either an erratum — run scripts/extract-spec-rules.py --check —
    // or a hand edit to a generated column.
    assert_eq!(rules.len(), 143, "expected 143 named rules");

    let mut by_spec: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    let mut anchors = BTreeSet::new();
    for r in &rules {
        assert!(
            anchors.insert(r.id()),
            "two rows for {} — the anchor is the rule's identity",
            r.id()
        );
        assert!(!r.name.is_empty(), "{} has no name", r.id());
        *by_spec.entry(r.spec.as_str()).or_default() += 1;
        *by_kind.entry(r.kind.as_str()).or_default() += 1;
    }
    assert_eq!(by_spec["structures"], 92);
    assert_eq!(by_spec["datatypes"], 51);
    assert_eq!(by_kind["Schema Component Constraint"], 66);
    assert_eq!(by_kind["Validation Rule"], 35);
    assert_eq!(by_kind["Schema Representation Constraint"], 20);
    assert_eq!(by_kind["Schema Information Set Contribution"], 16);
}

#[test]
fn every_claim_names_a_site_that_exists() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for r in rules() {
        match r.status.as_str() {
            "yes" | "partial" | "by-construction" => assert!(
                r.site != "-" && !r.site.is_empty(),
                "{} claims `{}` without naming where",
                r.id(),
                r.status
            ),
            "no" => assert!(
                !r.note.is_empty(),
                "{} claims nothing is enforced without saying so",
                r.id()
            ),
            other => panic!("{}: unknown status `{other}`", r.id()),
        }
        // `no` may still name a site — several rules are enforced against
        // values and unenforced across a restriction step, and pointing at the
        // half that exists is more use than a dash.
        if r.site != "-" && !r.site.is_empty() {
            for site in r.site.split(", ") {
                assert!(
                    root.join(site).is_file(),
                    "{} names {site}, which is not a file",
                    r.id()
                );
            }
        }
        assert!(
            r.status != "partial" || !r.note.is_empty(),
            "{} is partial without saying what is missing",
            r.id()
        );
    }
}

/// Every diagnostic answers to a named rule, or to something that is not XSD.
///
/// The direction that matters is this one: a code with no rule behind it is
/// either mis-modelled or implements a rule whose row still says `no`.
#[test]
fn every_diagnostic_code_is_attributable() {
    let exempt: BTreeMap<&str, &str> = NOT_A_SPEC_RULE
        .iter()
        .map(|(c, why)| (c.as_str(), *why))
        .collect();
    let rules = rules();
    // A rule's row does not name codes — the site column names files, which
    // is what stays true as diagnostics are split or merged — so attribution
    // is by file: a code emitted from a module no rule points at is one
    // nothing in the specification asked for.
    let claimed: BTreeSet<&str> = rules
        .iter()
        .filter(|r| r.site != "-" && !r.site.is_empty())
        .flat_map(|r| r.site.split(", "))
        .collect();

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let unreachable: BTreeMap<&str, &str> = DEFINED_BUT_UNREACHABLE
        .iter()
        .map(|(c, rule)| (c.as_str(), *rule))
        .collect();
    // An unreachable code has to name the rule it is for, and that rule has to
    // be one the table admits is unenforced. Otherwise the two lists could
    // drift into disagreeing about the same rule.
    for (code, rule) in &unreachable {
        let r = rules
            .iter()
            .find(|r| r.id() == *rule)
            .unwrap_or_else(|| panic!("{code} names {rule}, which is not a rule"));
        assert_eq!(
            r.status, "no",
            "{code} is unreachable but {rule} claims `{}`",
            r.status
        );
    }

    let mut orphans = Vec::new();
    for code in DiagCode::ALL {
        if exempt.contains_key(code.as_str()) || unreachable.contains_key(code.as_str()) {
            continue;
        }
        // Where is it emitted? Anything under src/, excluding the module that
        // merely defines it.
        let mut emitted_in = Vec::new();
        for entry in std::fs::read_dir(root.join("src")).expect("src/") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|e| e != "rs")
                || path.file_name().is_some_and(|n| n == "diagnostics.rs")
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read");
            if text.contains(&format!("DiagCode::{code:?}")) {
                let rel = path.strip_prefix(root).expect("under root");
                emitted_in.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
        assert!(
            !emitted_in.is_empty(),
            "{} ({code:?}) is never emitted. Remove it, or it is a rule \
             somebody stopped enforcing.",
            code.as_str()
        );
        if !emitted_in.iter().any(|m| claimed.contains(m.as_str())) {
            orphans.push(format!(
                "  {} ({code:?}) is emitted from {} — no rule in spec-rules.tsv \
                 names any of those",
                code.as_str(),
                emitted_in.join(", ")
            ));
        }
    }
    assert!(
        orphans.is_empty(),
        "diagnostics with no rule behind them:\n{}\n\nEither add the rule's \
         site to tests/conformance/spec-rules.tsv — its status has probably \
         just changed — or list the code in NOT_A_SPEC_RULE with what it \
         answers to instead.",
        orphans.join("\n")
    );
}

/// The fixture corpus and the table's `fixtures` column say the same thing.
///
/// Two directions, and both matter. A rule with fixtures that the column does
/// not mark makes the published page understate the corpus; a rule the column
/// marks with no fixtures behind it overstates it, which is worse.
#[test]
fn the_fixture_corpus_matches_the_table() {
    let rules = rules();
    let covered: BTreeSet<&str> = fixtures::COVERED.iter().copied().collect();
    assert_eq!(
        covered.len(),
        fixtures::COVERED.len(),
        "a rule is listed twice in fixtures::COVERED"
    );
    let marked: BTreeSet<String> = rules
        .iter()
        .filter(|r| r.fixtures)
        .map(|r| r.id())
        .collect();

    for id in &covered {
        let r = rules
            .iter()
            .find(|r| r.id() == **id)
            .unwrap_or_else(|| panic!("fixtures::COVERED names {id}, which is not a rule"));
        // A passing negative fixture for a rule nothing enforces is a
        // contradiction: either the fixture does not assert what it claims, or
        // the status is stale.
        assert_ne!(
            r.status, "no",
            "{id} has fixtures but the table says nothing enforces it"
        );
        assert!(
            r.fixtures,
            "{id} has fixtures; set its `fixtures` column in spec-rules.tsv"
        );
    }
    for id in &marked {
        assert!(
            covered.contains(id.as_str()),
            "{id} is marked as having fixtures, but fixtures::COVERED does not list it"
        );
    }
}

/// The counts in the file's own header have to be the counts in the file.
///
/// It is the first thing anyone reads and the easiest thing to leave behind
/// after re-judging one row.
#[test]
fn the_header_counts_match_the_rows() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("conformance")
        .join("spec-rules.tsv");
    let text = std::fs::read_to_string(path).expect("spec-rules.tsv");
    let summary = text
        .lines()
        .find(|l| l.contains("rules:") && l.contains("yes"))
        .expect("a `# N rules: ...` line in the header");

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for r in rules() {
        *counts.entry(r.status.clone()).or_default() += 1;
    }
    let want = format!(
        "# 143 rules: {} yes, {} partial, {} no, {} by-construction",
        counts["yes"], counts["partial"], counts["no"], counts["by-construction"]
    );
    assert_eq!(summary, want, "the header does not match the rows");
}

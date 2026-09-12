//! What the W3C XML Schema Test Suite says, and what `xsdkit` does with it.
//!
//! **One implementation of case selection, on purpose.** There used to be
//! three — `tests/w3c_suite.rs` and the two `w3c_*` examples each walked the
//! `.testSet` metadata themselves — and they disagreed: the test scored 480
//! invalid schemas and 16 false rejections where the examples said 476 and 17.
//! The examples were reading the *first* `<expected>` regardless of version,
//! which is the exact bug the test had already fixed and documented. A number
//! you cannot reproduce from the tool that printed it is not a measurement.
//!
//! This is not a crate, so the three callers reach it with `#[path]`:
//!
//! ```ignore
//! #[path = "../tests/suite/mod.rs"]
//! mod suite;
//! ```
//!
//! Cargo does not treat `tests/suite/` as a test target of its own — it looks
//! for `tests/*.rs` and `tests/*/main.rs` — so it compiles only as part of
//! whoever includes it. Each caller uses some of it and not the rest, hence
//! the blanket `dead_code` allow: "unused" here means "unused by this one of
//! the three".
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use xsdkit::{Compilation, Conformance, Diagnostics, SchemaSetBuilder, Version};

/// The `validity` the suite prescribes for a test, read at the version we
/// actually run it as.
///
/// A test may carry several `<expected>` elements, each qualified by
/// `version`. Per the suite's own schema those tokens are **and**ed: the
/// result is prescribed only for a processor supporting all of them. So a
/// version-qualified `expected` that names our version wins, an unqualified
/// one is the fallback, and one naming only the *other* version says nothing
/// about us.
///
/// Taking the first `<expected>` regardless — which this harness used to do —
/// scores 49 cases against the wrong expectation.
///
/// The same and-ing rule disposes of the CTA feature tokens without a special
/// case: ten `saxonData/CTA` groups prescribe `valid` for
/// `full-xpath-in-CTA` and `invalid` for `restricted-xpath-in-CTA`, we
/// implement neither XPath subset because we implement no CTA at all, and so
/// neither expectation is about us. No unqualified `<expected>` to fall back
/// to means the case is not scored, which is the honest answer rather than a
/// coin toss between the two.
pub fn expected_validity<'a>(test: roxmltree::Node<'a, 'a>, version: Version) -> Option<&'a str> {
    let token = version_token(version);
    let expects: Vec<_> = test
        .children()
        .filter(|n| n.has_tag_name("expected"))
        .collect();

    expects
        .iter()
        .find(|n| n.attribute("version").is_some_and(|v| v.contains(token)))
        .or_else(|| expects.iter().find(|n| n.attribute("version").is_none()))
        .and_then(|n| n.attribute("validity"))
}

/// Whether the working group's own metadata says this expectation is in doubt.
///
/// `<current status="queried">` means the recorded result has been challenged,
/// normally with a W3C bugzilla entry beside it. Scoring against an
/// expectation the suite itself will not stand behind is scoring noise, so
/// these are recorded as `skip queried` and left out of the percentages —
/// visible in the baseline, absent from the denominator.
///
/// Four cases carry it, two schema and two instance, so this is worth nothing
/// to the score and something to the definition of the score. The other
/// statuses stay in: `stable` and `accepted` are settled, and `submitted`
/// means newly contributed rather than doubted.
///
/// This is the only exclusion rule here, and it is the suite's own. The
/// permanently-unwinnable pairs — `saxonData/Simple`, where `simple001` needs
/// a 1.1 lexical form and `simple004` a 1.0 prohibition, in one unversioned
/// group — are *not* excluded. A per-case baseline records them as the
/// rejections they are, which is more honest than a hand-written list of
/// cases we have decided not to count, and is why this harness needs almost
/// none of the exclusion machinery other processors carry.
pub fn disputed(test: roxmltree::Node<'_, '_>) -> bool {
    test.children()
        .find(|n| n.has_tag_name("current"))
        .and_then(|n| n.attribute("status"))
        == Some("queried")
}

/// How a version prints, in the suite's spelling and the baseline's.
pub fn version_token(version: Version) -> &'static str {
    match version {
        Version::Xsd10 => "1.0",
        Version::Xsd11 => "1.1",
    }
}

/// Which XSD a test group is run as.
///
/// A group listing both versions is run as 1.0: it is the stricter reading, so
/// a schema that passes there passes in either.
pub fn version_of(v: &str) -> Version {
    // `full-xpath-in-CTA` and `restricted-xpath-in-CTA` are not versions, they
    // are which XPath subset a processor allows in conditional type
    // assignment — and CTA exists only in 1.1, so both name 1.1. Twenty
    // `saxonData/CTA` groups carry the first of them on the group itself, and
    // `contains("1.1")` said no to all of them: ten were scored against XSD
    // 1.0, a language in which `xs:alternative` is not a thing.
    if v.contains("CTA") || (v.contains("1.1") && !v.contains("1.0")) {
        Version::Xsd11
    } else {
        Version::Xsd10
    }
}

/// Where the suite lives, if it is available.
pub fn suite_root() -> Option<PathBuf> {
    // Unset means "not asked to run this", which is a legitimate skip.
    let Ok(var) = std::env::var("XSDTESTS") else {
        return None;
    };
    // Set but wrong is a different thing entirely, and used to skip in the
    // same silence — so a conformance run against a directory that had been
    // cleaned up reported success having measured nothing.
    let p = PathBuf::from(&var);
    assert!(
        p.join("suite.xml").is_file(),
        "XSDTESTS is set to `{var}`, which contains no suite.xml. Refusing to \
         skip: a run that measures nothing must not look like a run that passed."
    );
    Some(p)
}

/// One instance case: a document, the schema it belongs to, and whether the
/// suite says the document is valid against it.
#[derive(Debug)]
pub struct InstanceCase {
    pub set: String,
    pub group: String,
    pub version: String,
    pub schema_documents: Vec<PathBuf>,
    pub instance: PathBuf,
    pub expect_valid: bool,
    /// The working group has challenged this expectation. See [`disputed`].
    pub disputed: bool,
}

/// What one case did, in the two columns the baseline records.
///
/// `Panicked` is its own verdict rather than folded into a rejection. A panic
/// still scores as "did not accept", which is what it was before, but a case
/// that starts or stops panicking is the single most interesting row in the
/// file and it must not hide inside a percentage.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing was an error.
    Accepted,
    /// Something was, and these are the distinct codes.
    Rejected(String),
    /// The loader or the validator panicked on it.
    Panicked,
    /// Not scored, and the reason why: the suite disputes its own
    /// expectation, the schema did not compile, or the file would not read.
    Skipped(&'static str),
}

impl Outcome {
    pub fn accepted(&self) -> bool {
        matches!(self, Outcome::Accepted)
    }

    /// Whether this case counts towards the score at all.
    pub fn scored(&self) -> bool {
        !matches!(self, Outcome::Skipped(_))
    }

    /// The `verdict` and `codes` columns.
    pub fn columns(&self) -> String {
        match self {
            Outcome::Accepted => "accept\t-".to_string(),
            Outcome::Rejected(codes) => format!("reject\t{codes}"),
            Outcome::Panicked => "panic\t-".to_string(),
            Outcome::Skipped(why) => format!("skip\t{why}"),
        }
    }
}

/// The distinct error codes of a diagnostic set, sorted and joined with `+`.
///
/// Sorted and deduplicated rather than "the first one": the order diagnostics
/// come out in is an implementation detail, but *which* rules fired is worth
/// pinning. A case that starts being caught by a second rule as well is a
/// change a reviewer should see, and one whose only rule changes is a case
/// that used to pass for a different reason than it does now.
pub fn error_codes(d: &Diagnostics) -> String {
    let codes: BTreeSet<&str> = d.errors().map(|e| e.code.as_str()).collect();
    if codes.is_empty() {
        // Unreachable while callers only ask after `has_errors`, but a silent
        // empty column would be worse than a visible marker.
        return "-".to_string();
    }
    codes.into_iter().collect::<Vec<_>>().join("+")
}

/// One schema case: the documents to load, and whether the suite says the
/// schema is valid.
#[derive(Debug)]
pub struct SchemaCase {
    pub set: String,
    pub group: String,
    pub version: String,
    pub documents: Vec<PathBuf>,
    pub expect_valid: bool,
    /// The working group has challenged this expectation. See [`disputed`].
    pub disputed: bool,
}

/// Reads the `.testSet` metadata with the crate itself is not appropriate —
/// these are ordinary XML, read with a small hand-rolled scan so a bug in
/// `xsdkit` cannot silently change which cases run.
pub fn parse_test_sets(root: &Path) -> (Vec<SchemaCase>, Vec<InstanceCase>) {
    let mut out = Vec::new();
    let mut instances = Vec::new();
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
                let Some(validity) = expected_validity(st, version_of(&version)) else {
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
                    disputed: disputed(st),
                    documents: documents.clone(),
                    expect_valid,
                });

                // Instance cases only mean anything against a schema the
                // suite says is valid; a document cannot be judged against a
                // schema that should not have compiled.
                if !expect_valid {
                    continue;
                }
                for it in group.children().filter(|n| n.has_tag_name("instanceTest")) {
                    let Some(href) = it
                        .children()
                        .find(|n| n.has_tag_name("instanceDocument"))
                        .and_then(|n| n.attribute(("http://www.w3.org/1999/xlink", "href")))
                    else {
                        continue;
                    };
                    let Some(validity) = expected_validity(it, version_of(&version)) else {
                        continue;
                    };
                    let expect = match validity {
                        "valid" => true,
                        "invalid" => false,
                        _ => continue,
                    };
                    instances.push(InstanceCase {
                        set: set.clone(),
                        group: name.clone(),
                        version: version.clone(),
                        disputed: disputed(it),
                        schema_documents: documents.clone(),
                        instance: dir.join(href),
                        expect_valid: expect,
                    });
                }
            }
        }
    }
    (out, instances)
}

/// Compiles one schema case, at the version its group is run as. `None` means
/// it panicked.
///
/// Every diagnostic, not a verdict: the test wants the codes, `w3c_why` wants
/// the messages, and neither should be compiling the case its own way to get
/// them.
pub fn compile_case(case: &SchemaCase) -> Option<Diagnostics> {
    let mut b = SchemaSetBuilder::new()
        .version(version_of(&case.version))
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
        let Compilation { diagnostics, .. } = b.compile();
        diagnostics
    }))
    .ok()
}

/// What `xsdkit` makes of the schema, and why not if it rejects it.
pub fn schema_outcome(case: &SchemaCase) -> Outcome {
    if case.disputed {
        return Outcome::Skipped("queried");
    }
    match compile_case(case) {
        None => Outcome::Panicked,
        Some(d) if d.has_errors() => Outcome::Rejected(error_codes(&d)),
        Some(_) => Outcome::Accepted,
    }
}

/// One instance document against its schema, with the schema compiled at most
/// once per group.
///
/// A schema this crate could not load says nothing about the document, so those
/// are skipped rather than scored. A panic is not: it scores as "did not
/// accept", the same way the schema half treats one.
pub fn instance_outcome(
    c: &InstanceCase,
    cache: &mut BTreeMap<String, Option<xsdkit::Schemas>>,
) -> Outcome {
    if c.disputed {
        return Outcome::Skipped("queried");
    }
    let Ok(xml) = std::fs::read_to_string(&c.instance) else {
        return Outcome::Skipped("unreadable");
    };
    let cache_key = format!(
        "{}|{}",
        c.version,
        c.schema_documents
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let entry = cache.entry(cache_key).or_insert_with(|| {
        let version = version_of(&c.version);
        let mut b = SchemaSetBuilder::new()
            .version(version)
            .conformance(Conformance::Lax);
        if let Some(dir) = c.schema_documents[0].parent() {
            b = b.search_path(dir);
        }
        for d in &c.schema_documents {
            b = b.file(d.display().to_string());
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let Compilation {
                schemas,
                diagnostics: diags,
            } = b.compile();
            (!diags.has_errors()).then_some(schemas)
        }))
        .unwrap_or(None)
    });
    let Some(schemas) = entry else {
        return Outcome::Skipped("schema");
    };
    let verdict = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let report = schemas.document_validator().validate(&xml);
        (!report.is_valid()).then(|| error_codes(&report.diagnostics))
    }));
    match verdict {
        Ok(None) => Outcome::Accepted,
        Ok(Some(codes)) => Outcome::Rejected(codes),
        Err(_) => Outcome::Panicked,
    }
}

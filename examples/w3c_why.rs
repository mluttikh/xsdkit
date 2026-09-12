//! Why does xsdkit reject schemas the W3C suite says are valid?
//!
//! The mirror of `w3c_gap`. Every case here is one the suite calls valid and
//! we refuse, clustered by the diagnostic that refused it, so the fix with the
//! most cases behind it is the top line.
//!
//! ```text
//! XSDTESTS=/tmp/xsdtests cargo run --release --example w3c_why
//! ```
//!
//! Case selection comes from `tests/suite/mod.rs`, shared with
//! `tests/w3c_suite.rs`, so this counts what the gate counts. It did not
//! always: reading the first `<expected>` regardless of version made this
//! report 17 rejections where the test scored 16.
use std::collections::BTreeMap;

#[path = "../tests/suite/mod.rs"]
mod suite;
use suite::*;

fn main() {
    let Some(root) = suite_root() else {
        eprintln!("XSDTESTS is not set; nothing to report");
        return;
    };
    let (cases, _) = parse_test_sets(&root);

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    // code -> (count, a few samples)
    let mut by_code: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    for case in cases.iter().filter(|c| c.expect_valid) {
        let name = case.documents[0]
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let Some(diags) = compile_case(case) else {
            let e = by_code.entry("PANIC".into()).or_default();
            e.0 += 1;
            if e.1.len() < 3 {
                e.1.push(name);
            }
            continue;
        };
        // The first error is the one that explains the rejection; the rest are
        // usually its consequences.
        for d in diags.errors().take(1) {
            let e = by_code
                .entry(format!("{} {:?}", d.code, d.code))
                .or_default();
            e.0 += 1;
            if e.1.len() < 3 {
                e.1.push(format!(
                    "{name}  |  {}",
                    d.message.chars().take(70).collect::<String>()
                ));
            }
        }
    }
    std::panic::set_hook(hook);

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

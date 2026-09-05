//! Arbitrary strings into the XSD pattern translator.
//!
//! Hand-written recursive-descent over a grammar with class subtraction,
//! nested escapes and Unicode blocks — the likeliest place in the crate for
//! an index to run off the end of a `Vec<char>`.
//!
//! Whatever comes out must also be a pattern the `regex` crate can match with
//! without panicking, since translating to something that compiles but
//! explodes on use would be worse than failing outright.
#![no_main]

use libfuzzer_sys::fuzz_target;
use xsdkit::regex::{PatternStep, Patterns};

fuzz_target!(|data: &str| {
    if data.len() > 4096 {
        return;
    }
    if let Ok(step) = PatternStep::compile(&[data.to_string()]) {
        let _ = step.is_match("");
        let _ = step.is_match(data);
        let _ = step.is_match("abc123");
        let _ = step.as_str();
    }
    // Several alternatives at one step, and several steps, exercise the
    // joining as well as the translation.
    if let Ok(p) = Patterns::compile(&[
        vec![data.to_string(), "[a-z]".to_string()],
        vec![".*".to_string()],
    ]) {
        let _ = p.is_match(data);
        let _ = p.first_failure(data);
    }
});

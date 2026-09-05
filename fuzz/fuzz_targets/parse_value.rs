//! Arbitrary lexical forms into every built-in datatype.
//!
//! The value layer is the most specification-dense code in the crate and the
//! lowest-covered. Each built-in has its own lexical grammar, and several are
//! hand-written: the integer chain, the XML name productions, hexBinary and
//! base64.
//!
//! A parsed value must also render and compare without panicking — a value
//! that cannot be displayed is not usable.
#![no_main]

use libfuzzer_sys::fuzz_target;
use xsdkit::datatypes::Builtin;
use xsdkit::values;

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let Ok(lexical) = std::str::from_utf8(rest) else {
        return;
    };
    if lexical.len() > 4096 {
        return;
    }
    let all = Builtin::all();
    let builtin = all[selector as usize % all.len()];

    if let Ok(v) = values::parse(builtin, lexical) {
        let _ = v.to_string();
        let _ = v.facet_length();
        let _ = v.partial_cmp_value(&v);
        // Round-tripping the canonical form must not panic, and for most
        // types must land back in the value space.
        let _ = values::parse(builtin, &v.to_string());
    }
});

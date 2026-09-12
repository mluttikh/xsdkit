//! `xs:decimal` held exactly, and in the scale it was written with.

use std::cmp::Ordering::{Equal, Greater, Less};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use xsdkit::atomic::Decimal;
use xsdkit::datatypes::Builtin;
use xsdkit::values::{Value, parse};
use xsdkit::*;

fn dec(lexical: &str) -> Decimal {
    match parse(Builtin::Decimal, lexical) {
        Ok(Value::Decimal(d)) => d,
        other => panic!("`{lexical}` should be a decimal, got {other:?}"),
    }
}

fn hash(d: Decimal) -> u64 {
    let mut h = DefaultHasher::new();
    d.hash(&mut h);
    h.finish()
}

/// Whether `value` is valid for a decimal restricted by one facet.
fn valid(facet: &str, value: &str) -> bool {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      targetNamespace="urn:d" elementFormDefault="qualified">
             <xs:element name="p"><xs:simpleType>
               <xs:restriction base="xs:decimal">{facet}</xs:restriction>
             </xs:simpleType></xs:element>
           </xs:schema>"#
    );
    let schemas = SchemaSetBuilder::new()
        .text(xsd, "mem://d.xsd")
        .compile()
        .into_result()
        .unwrap_or_else(|d| panic!("{d}"));
    schemas
        .document_validator()
        .validate(&format!(r#"<p xmlns="urn:d">{value}</p>"#))
        .is_valid()
}

#[test]
fn facets_compare_every_digit_the_document_wrote() {
    // Each of these used to get the wrong answer. Digits past the eighteenth
    // were dropped before comparing, so the facet compared a value the
    // document did not contain.
    assert!(
        !valid(
            r#"<xs:maxInclusive value="0.123456789012345678"/>"#,
            "0.1234567890123456789"
        ),
        "above the maximum"
    );
    assert!(
        valid(
            r#"<xs:maxExclusive value="0.1234567890123456789"/>"#,
            "0.1234567890123456781"
        ),
        "below the bound"
    );
    assert!(
        !valid(
            r#"<xs:enumeration value="0.1234567890123456789"/>"#,
            "0.1234567890123456788"
        ),
        "not the listed value"
    );
    assert!(
        !valid(
            r#"<xs:fractionDigits value="18"/>"#,
            "0.1234567890123456789"
        ),
        "nineteen fraction digits"
    );
}

#[test]
fn an_enumeration_still_matches_across_scale() {
    assert!(valid(r#"<xs:enumeration value="1.00"/>"#, "1.0"));
    assert!(valid(r#"<xs:enumeration value="1"/>"#, "1.000"));
}

#[test]
fn the_written_scale_is_kept_and_equality_ignores_it() {
    let (a, b) = (dec("4.50"), dec("4.5"));
    assert_eq!(a.as_written().to_string(), "4.50");
    assert_eq!(a.to_string(), "4.5", "Display is still the canonical form");
    assert_eq!((a.coefficient(), a.exponent()), (450, -2));
    assert_eq!(a, b);
    assert_eq!(a.cmp(&b), Equal);
    assert_eq!(hash(a), hash(b), "equal values must hash alike");
}

#[test]
fn thirty_eight_significant_digits_are_exact_and_thirty_nine_are_refused() {
    let nines = "9".repeat(Decimal::MAX_DIGITS as usize);
    assert_eq!(dec(&nines).to_string(), nines);
    let err = format!(
        "{:?}",
        parse(Builtin::Decimal, &"9".repeat(39)).unwrap_err()
    );
    assert!(
        err.contains("limit of xsdkit"),
        "the message should say whose limit: {err}"
    );
}

#[test]
fn zeroes_that_carry_no_digits_do_not_count_against_the_limit() {
    // Sixty zeroes after the point: still one significant digit.
    assert_eq!(dec(&format!("1.{}", "0".repeat(60))), dec("1"));
    // An integer's trailing zeroes live in the exponent and come back out.
    let big = format!("9{}", "0".repeat(45));
    assert_eq!(dec(&big).to_string(), big);
    assert_eq!(dec(&big).as_written().to_string(), big);
    // And a value far below one is exact too.
    let tiny = dec(&format!("0.{}1", "0".repeat(60)));
    assert!(tiny > dec("0"));
    assert!(tiny < dec(&format!("0.{}1", "0".repeat(59))));
}

#[test]
fn zero_has_no_sign() {
    let z = dec("-0.00");
    assert!(!z.is_negative());
    assert_eq!(z, dec("0"));
    assert_eq!(z.to_string(), "0");
    assert_eq!(z.as_written().to_string(), "0.00");
}

#[test]
fn decimals_compare_with_integers_of_any_size() {
    // An xs:integer of 10^21 used to be incomparable with an xs:decimal:
    // scaling it to the old fixed point overflowed, and a decimal that large
    // could not be parsed at all.
    let int = parse(Builtin::Integer, "1000000000000000000000").unwrap();
    let d = parse(Builtin::Decimal, "1000000000000000000000.0").unwrap();
    assert_eq!(int.partial_cmp_value(&d), Some(Equal));
    assert_eq!(d.partial_cmp_value(&int), Some(Equal));
}

#[test]
fn ordering_holds_where_aligning_would_overflow() {
    // The same leading digit position with coefficients of very different
    // lengths: widening the short one overflows a u128, which happens exactly
    // when it is the larger.
    let max = parse(Builtin::Integer, &i128::MAX.to_string()).unwrap();
    let nine = parse(Builtin::Decimal, &format!("9{}", "0".repeat(38))).unwrap();
    assert_eq!(max.partial_cmp_value(&nine), Some(Less));
    assert_eq!(nine.partial_cmp_value(&max), Some(Greater));
}

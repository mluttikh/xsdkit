//! `xs:precisionDecimal`, the optional XSD 1.1 datatype.
//!
//! A decimal that remembers how it was written. `1.0` and `1.00` are the same
//! *number* and compare equal, but they are different values — the second has
//! a scale of two — and `minScale`/`maxScale` tell them apart. That is the
//! point of the type: the trailing zeroes carry the precision of a
//! measurement.
//!
//! Every expectation here is derived from the W3C suite's own data
//! (`saxonData/PDecimal`), which is the only complete statement of these
//! semantics that survived into a normative document.

use xsdkit::datatypes::Builtin as B;
use xsdkit::diagnostics::DiagCode;
use xsdkit::values::parse;
use xsdkit::*;

const NS: &str = "urn:example";

#[track_caller]
fn ok(lexical: &str) -> Value {
    parse(B::PrecisionDecimal, lexical)
        .unwrap_or_else(|e| panic!("{lexical:?} rejected: {}", e.reason))
}

#[track_caller]
fn rejects(lexical: &str) {
    assert!(
        parse(B::PrecisionDecimal, lexical).is_err(),
        "{lexical:?} should be rejected"
    );
}

#[test]
fn the_lexical_space_is_floats_plus_a_signed_zero() {
    for s in [
        "12", "12.", ".001", "12.000", "12.02", "-123.456", "+123.456", "0.001", ".001000", "12e8",
        "12e-8", "012.5E-8", "-0", "-0.00", "INF", "+INF", "-INF", "NaN",
    ] {
        ok(s);
    }
    for s in [
        "--2",
        "1 e3",
        "1.2.5",
        "Infinity",
        "NAN",
        "fried chicken",
        "",
        ".",
        "1e",
        "e5",
    ] {
        rejects(s);
    }
}

/// Order and equality ignore the scale — the number is what is compared.
#[test]
fn equality_is_numeric_and_ignores_the_scale() {
    let eq = |a: &str, b: &str| ok(a) == ok(b);
    assert!(eq("1.0", "1"));
    assert!(eq("1.00000000000", "1"));
    assert!(eq("10e-1", "1.0"));
    assert!(eq("1e0", "1"));
    assert!(eq("0.0000", "0.0"));
    // Both zeroes are one number, however they are signed.
    assert!(eq("-0.0", "0.0"));
    assert!(eq("-0", "0"));
    assert!(!eq("17.3", "1.0"));

    // The specials behave as they do for float: infinities are ordered, and
    // `NaN` equals nothing at all, itself included.
    assert!(eq("INF", "+INF"));
    assert!(!eq("NaN", "NaN"));
    assert!(!eq("INF", "-INF"));

    use std::cmp::Ordering::*;
    let cmp = |a: &str, b: &str| ok(a).partial_cmp_value(&ok(b));
    assert_eq!(cmp("-INF", "0"), Some(Less));
    assert_eq!(cmp("0", "INF"), Some(Less));
    assert_eq!(cmp("-INF", "INF"), Some(Less));
    assert_eq!(cmp("NaN", "0"), None);
    assert_eq!(cmp("-1", "1"), Some(Less));
    assert_eq!(cmp("-0", "1"), Some(Less));
    assert_eq!(cmp("-1", "-0"), Some(Less));
    // Wildly different exponents, where scaling either side would overflow.
    assert_eq!(cmp("1e-2000000", "1e2000000"), Some(Less));
    assert_eq!(cmp("9.99e10", "1e11"), Some(Less));
}

/// The scale is what the type adds, and it is signed: `2e2` is a multiple of a
/// hundred, so its scale is -2.
#[test]
fn the_scale_survives_parsing() {
    let scale = |s: &str| match ok(s) {
        Value::PrecisionDecimal(p) => p.scale(),
        other => panic!("expected a precisionDecimal, got {other}"),
    };
    assert_eq!(scale("0.0000"), Some(4));
    assert_eq!(scale("0.0030"), Some(4));
    assert_eq!(scale("2.0e-3"), Some(4));
    assert_eq!(scale("200"), Some(0));
    assert_eq!(scale("2e2"), Some(-2));
    // A special value has no scale at all.
    assert_eq!(scale("INF"), None);
    assert_eq!(scale("NaN"), None);
}

/// `totalDigits` counts the digits as *written*, where for an `xs:decimal` it
/// counts them canonically. `1.000` has four here and one there.
#[test]
fn total_digits_counts_written_digits() {
    let digits = |s: &str| match ok(s) {
        Value::PrecisionDecimal(p) => p.total_digits(),
        other => panic!("expected a precisionDecimal, got {other}"),
    };
    assert_eq!(digits("1.000"), Some(4));
    assert_eq!(digits("1.0"), Some(2));
    assert_eq!(digits("1234"), Some(4));
    assert_eq!(digits("1.234e20"), Some(4));
    assert_eq!(digits("10e-1"), Some(2));
    // Zero is one digit however it is written, which is what keeps
    // `0.000000` inside a `totalDigits` of four.
    assert_eq!(digits("0.000000"), Some(1));
    assert_eq!(digits("0"), Some(1));
    assert_eq!(digits("12345"), Some(5));
    assert_eq!(digits("INF"), None);
}

// ---------------------------------------------------------------------------
// Through a schema
// ---------------------------------------------------------------------------

fn build(body: &str) -> Schemas {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .version(Version::Xsd11)
        .text(xsd, "mem://main.xsd")
        .compile()
        .into_result()
        .unwrap_or_else(|d| panic!("{d}"))
}

fn diags(body: &str) -> Diagnostics {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .version(Version::Xsd11)
        .text(xsd, "mem://main.xsd")
        .compile()
        .diagnostics
}

#[test]
fn the_scale_facets_bound_the_scale() {
    let s = build(
        r#"<xs:simpleType name="T">
             <xs:restriction base="xs:precisionDecimal">
               <xs:minScale value="4"/><xs:maxScale value="8"/>
             </xs:restriction>
           </xs:simpleType>"#,
    );
    let t = s.type_id(Some(NS), "T").expect("type");
    let v = s.value_validator();
    for good in ["0.0000", "0.0030", "2.0e-3", "0.0000003", "-0.0000"] {
        assert!(v.validate(t, good).is_ok(), "{good} should be in scale");
    }
    for bad in ["0.0", "-0.0", "0.003", "200", "-0.000000003"] {
        assert!(v.validate(t, bad).is_err(), "{bad} is out of scale");
    }
    // A special value has no scale, so the facets cannot exclude one.
    for special in ["INF", "-INF", "+INF", "NaN"] {
        assert!(v.validate(t, special).is_ok(), "{special} has no scale");
    }
}

#[test]
fn total_digits_applies_to_the_written_form() {
    let s = build(
        r#"<xs:simpleType name="T">
             <xs:restriction base="xs:precisionDecimal">
               <xs:totalDigits value="4"/>
             </xs:restriction>
           </xs:simpleType>"#,
    );
    let t = s.type_id(Some(NS), "T").expect("type");
    let v = s.value_validator();
    for good in [
        "1.234", "1234", "1.000", "0.000000", "1.234e20", "INF", "NaN",
    ] {
        assert!(
            v.validate(t, good).is_ok(),
            "{good} has four digits or fewer"
        );
    }
    for bad in ["12345", "1234.0", "-0.12340", "12340e20"] {
        assert!(v.validate(t, bad).is_err(), "{bad} has five");
    }
}

/// The scale facets belong to this type alone, and they narrow like any other.
#[test]
fn the_scale_facets_are_constrained_like_the_rest() {
    // Not applicable to an ordinary decimal.
    assert!(
        diags(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:decimal"><xs:minScale value="2"/></xs:restriction>
               </xs:simpleType>"#
        )
        .errors()
        .any(|d| d.code == DiagCode::FacetNotApplicable)
    );
    // A restriction may not widen them.
    for derived in [r#"<xs:minScale value="3"/>"#, r#"<xs:maxScale value="9"/>"#] {
        let body = format!(
            r#"<xs:simpleType name="A">
                 <xs:restriction base="xs:precisionDecimal">
                   <xs:minScale value="4"/><xs:maxScale value="8"/>
                 </xs:restriction>
               </xs:simpleType>
               <xs:simpleType name="B">
                 <xs:restriction base="tns:A">{derived}</xs:restriction>
               </xs:simpleType>"#
        );
        assert!(
            diags(&body)
                .errors()
                .any(|d| d.code == DiagCode::ConflictingFacets),
            "{derived} widens what it inherited"
        );
    }
    // And minScale above maxScale is empty.
    assert!(
        diags(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:precisionDecimal">
                   <xs:minScale value="8"/><xs:maxScale value="4"/>
                 </xs:restriction>
               </xs:simpleType>"#
        )
        .errors()
        .any(|d| d.code == DiagCode::ConflictingFacets)
    );
}

/// An enumeration compares numbers, so a member written with a different scale
/// still matches.
#[test]
fn an_enumeration_matches_the_number_not_the_spelling() {
    let s = build(
        r##"<xs:simpleType name="T">
              <xs:restriction base="xs:precisionDecimal">
                <xs:enumeration value="-INF"/>
                <xs:enumeration value="+INF"/>
                <xs:enumeration value="0.0"/>
                <xs:enumeration value="1.0"/>
              </xs:restriction>
            </xs:simpleType>"##,
    );
    let t = s.type_id(Some(NS), "T").expect("type");
    let v = s.value_validator();
    for good in [
        "-0.0", "-INF", "+INF", "0.0", "0.0000", "0", "1.0", "1", "10e-1", "1e0",
    ] {
        assert!(v.validate(t, good).is_ok(), "{good} is one of the four");
    }
    for bad in ["17.3", "NaN"] {
        assert!(v.validate(t, bad).is_err(), "{bad} is not");
    }
}

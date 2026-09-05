//! Validating values against a schema's own simple types.

use xsdkit::atomic::Decimal;
use xsdkit::validate::ValidationError;
use xsdkit::*;

const NS: &str = "urn:example";

fn build(body: &str) -> Schemas {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .text(xsd, "mem://main.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"))
}

/// Validates `lexical` against the named global simple type.
fn check(s: &Schemas, name: &str, lexical: &str) -> Result<Value, ValidationError> {
    let v = s.validator();
    v.validate(s.type_(Some(NS), name).expect("type"), lexical)
}

// ---------------------------------------------------------------------------
// Facets composing up the chain
// ---------------------------------------------------------------------------

/// A type stores only the facets it declares; the set in force is the fold
/// down the whole chain.
#[test]
fn facets_compose_down_a_restriction_chain() {
    let s = build(
        r#"<xs:simpleType name="Base">
             <xs:restriction base="xs:int"><xs:minInclusive value="0"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="Mid">
             <xs:restriction base="tns:Base"><xs:maxInclusive value="100"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="Leaf">
             <xs:restriction base="tns:Mid"><xs:maxInclusive value="10"/></xs:restriction>
           </xs:simpleType>"#,
    );
    // The inherited lower bound still applies three levels down.
    assert!(
        check(&s, "Leaf", "-1").is_err(),
        "Base's minInclusive must survive"
    );
    assert!(check(&s, "Leaf", "5").is_ok());
    assert!(
        check(&s, "Leaf", "11").is_err(),
        "Leaf narrowed the upper bound"
    );
    assert!(check(&s, "Mid", "50").is_ok(), "Mid keeps the looser bound");
}

/// Patterns AND across steps even though they OR within one.
#[test]
fn patterns_and_across_the_chain() {
    let s = build(
        r#"<xs:simpleType name="Letters">
             <xs:restriction base="xs:string"><xs:pattern value="[A-Za-z]+"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="Three">
             <xs:restriction base="tns:Letters"><xs:pattern value=".{3}"/></xs:restriction>
           </xs:simpleType>"#,
    );
    assert!(check(&s, "Three", "abc").is_ok());
    assert!(
        check(&s, "Three", "ab").is_err(),
        "fails the length pattern"
    );
    assert!(
        check(&s, "Three", "a1c").is_err(),
        "fails the inherited letters pattern"
    );
}

/// `whiteSpace` comes from the nearest built-in ancestor, not the primitive.
/// A token-derived type collapses; its primitive `xs:string` preserves.
#[test]
fn whitespace_follows_the_builtin_ancestor_not_the_primitive() {
    let s = build(
        r#"<xs:simpleType name="Tok">
             <xs:restriction base="xs:token"><xs:maxLength value="5"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="Str">
             <xs:restriction base="xs:string"><xs:maxLength value="5"/></xs:restriction>
           </xs:simpleType>"#,
    );
    // Collapsed to "a b" (3 chars), so it fits.
    assert_eq!(
        check(&s, "Tok", "  a   b  ").unwrap(),
        Value::String("a b".into())
    );
    // Preserved at 9 characters, so maxLength rejects it.
    assert!(check(&s, "Str", "  a   b  ").is_err());
    assert_eq!(
        check(&s, "Str", "abc").unwrap(),
        Value::String("abc".into())
    );
}

// ---------------------------------------------------------------------------
// Varieties
// ---------------------------------------------------------------------------

#[test]
fn list_items_are_validated_individually() {
    let s = build(
        r#"<xs:simpleType name="Small">
             <xs:restriction base="xs:int"><xs:maxInclusive value="9"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="Smalls">
             <xs:list itemType="tns:Small"/>
           </xs:simpleType>"#,
    );
    let Value::List(items) = check(&s, "Smalls", "1 2 3").unwrap() else {
        panic!("expected a list")
    };
    assert_eq!(items.len(), 3);
    assert!(
        check(&s, "Smalls", "1 20 3").is_err(),
        "each item faces the item type's facets"
    );
}

/// A list's own length facets count items, not characters.
#[test]
fn list_length_facets_count_items() {
    let s = build(
        r#"<xs:simpleType name="Pair">
             <xs:restriction>
               <xs:simpleType><xs:list itemType="xs:int"/></xs:simpleType>
               <xs:length value="2"/>
             </xs:restriction>
           </xs:simpleType>"#,
    );
    assert!(check(&s, "Pair", "1 2").is_ok());
    assert!(check(&s, "Pair", "1 2 3").is_err());
    assert!(check(&s, "Pair", "1").is_err());
}

/// Member order decides which type the value *has*, not merely whether it is
/// valid.
#[test]
fn union_members_are_tried_in_declaration_order() {
    let s = build(
        r#"<xs:simpleType name="IntFirst">
             <xs:union memberTypes="xs:int xs:string"/>
           </xs:simpleType>
           <xs:simpleType name="StringFirst">
             <xs:union memberTypes="xs:string xs:int"/>
           </xs:simpleType>"#,
    );
    // Same lexical form, different resulting value, decided by order.
    assert_eq!(check(&s, "IntFirst", "42").unwrap(), Value::Integer(42));
    assert_eq!(
        check(&s, "StringFirst", "42").unwrap(),
        Value::String("42".into())
    );
    // Falls through to the second member when the first rejects it.
    assert_eq!(
        check(&s, "IntFirst", "abc").unwrap(),
        Value::String("abc".into())
    );
}

#[test]
fn a_union_reports_which_member_matched() {
    let s = build(
        r#"<xs:simpleType name="U"><xs:union memberTypes="xs:int xs:date"/></xs:simpleType>"#,
    );
    let v = s.validator();
    let u = s.type_(Some(NS), "U").unwrap();
    assert_eq!(
        v.union_member(u, "42"),
        Some(s.builtin(xsdkit::datatypes::Builtin::Int))
    );
    assert_eq!(
        v.union_member(u, "2024-01-01"),
        Some(s.builtin(xsdkit::datatypes::Builtin::Date))
    );
    assert_eq!(v.union_member(u, "nope"), None);
}

#[test]
fn a_union_with_no_matching_member_says_so() {
    let s = build(
        r#"<xs:simpleType name="U"><xs:union memberTypes="xs:int xs:date"/></xs:simpleType>"#,
    );
    match check(&s, "U", "nope").unwrap_err() {
        ValidationError::NoUnionMember { tried } => assert_eq!(tried, 2),
        other => panic!("expected NoUnionMember, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Enumerations, values, errors
// ---------------------------------------------------------------------------

#[test]
fn enumerations_compare_in_the_value_space() {
    let s = build(
        r#"<xs:simpleType name="Rate">
             <xs:restriction base="xs:decimal">
               <xs:enumeration value="1.00"/><xs:enumeration value="2.5"/>
             </xs:restriction>
           </xs:simpleType>"#,
    );
    // Same decimal, different spelling.
    assert!(check(&s, "Rate", "1.0").is_ok());
    assert!(check(&s, "Rate", "1").is_ok());
    assert!(check(&s, "Rate", "2.50").is_ok());
    assert!(check(&s, "Rate", "3").is_err());
}

#[test]
fn typed_values_come_back_typed() {
    let s = build(
        r#"<xs:simpleType name="Count"><xs:restriction base="xs:int"/></xs:simpleType>
           <xs:simpleType name="When"><xs:restriction base="xs:dateTime"/></xs:simpleType>
           <xs:simpleType name="Flag"><xs:restriction base="xs:boolean"/></xs:simpleType>"#,
    );
    assert_eq!(check(&s, "Count", "42").unwrap(), Value::Integer(42));
    assert_eq!(check(&s, "Flag", "true").unwrap(), Value::Boolean(true));
    assert_eq!(
        check(&s, "When", "2024-12-30T12:39:15Z")
            .unwrap()
            .to_string(),
        "2024-12-30T12:39:15Z"
    );
}

#[test]
fn errors_distinguish_a_bad_lexical_form_from_a_bad_value() {
    let s = build(
        r#"<xs:simpleType name="Small">
             <xs:restriction base="xs:int"><xs:maxInclusive value="9"/></xs:restriction>
           </xs:simpleType>"#,
    );
    assert!(matches!(
        check(&s, "Small", "nope").unwrap_err(),
        ValidationError::Lexical(_)
    ));
    let ValidationError::Facet(v) = check(&s, "Small", "10").unwrap_err() else {
        panic!("expected a facet violation")
    };
    assert_eq!(v.facet, "maxInclusive");
}

#[test]
fn a_complex_type_has_no_value_space() {
    let s = build(r#"<xs:complexType name="T"><xs:sequence/></xs:complexType>"#);
    assert!(matches!(
        check(&s, "T", "anything").unwrap_err(),
        ValidationError::NotSimple
    ));
}

#[test]
fn uncompilable_patterns_are_reported_not_ignored() {
    // A pattern that never runs makes a type quietly more permissive than it
    // declares, so it must surface.
    let s = build(
        r#"<xs:simpleType name="Bad">
             <xs:restriction base="xs:string"><xs:pattern value="[a-z"/></xs:restriction>
           </xs:simpleType>"#,
    );
    let v = s.validator();
    assert!(
        !v.pattern_errors().is_empty(),
        "an invalid pattern must be surfaced"
    );
}

// ---------------------------------------------------------------------------
// Against the real schema
// ---------------------------------------------------------------------------

#[test]
fn the_schema_for_schemas_validates_its_own_vocabulary() {
    let (s, _) = SchemaSetBuilder::new()
        .file("tests/fixtures/XMLSchema.xsd")
        .conformance(Conformance::Lax)
        .build_with_warnings();
    let v = s.validator();
    let xs = "http://www.w3.org/2001/XMLSchema";

    // xs:derivationControl is an enumeration of five tokens.
    let dc = s
        .type_(Some(xs), "derivationControl")
        .expect("derivationControl");
    assert!(v.validate(dc, "extension").is_ok());
    assert!(v.validate(dc, "restriction").is_ok());
    assert!(v.validate(dc, "nonsense").is_err());

    // xs:allNNI is a union of nonNegativeInteger and the token "unbounded".
    let nni = s.type_(Some(xs), "allNNI").expect("allNNI");
    assert!(v.validate(nni, "0").is_ok());
    assert!(v.validate(nni, "unbounded").is_ok());
    assert!(v.validate(nni, "-1").is_err());

    assert!(
        v.pattern_errors().is_empty(),
        "every pattern in the W3C schema must compile: {:?}",
        v.pattern_errors()
    );
}

/// Regression: a user type restricting `xs:byte` must keep the byte range.
///
/// Parsing against the *primitive* would make this an `xs:decimal`, and
/// `999` would validate — silent corruption rather than a rejection.
#[test]
fn integer_bounds_survive_a_user_restriction() {
    let s = build(
        r#"<xs:simpleType name="Small">
             <xs:restriction base="xs:byte"><xs:minInclusive value="0"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="Big">
             <xs:restriction base="xs:unsignedLong"/>
           </xs:simpleType>"#,
    );
    assert_eq!(check(&s, "Small", "127").unwrap(), Value::Integer(127));
    assert!(
        check(&s, "Small", "128").is_err(),
        "xs:byte tops out at 127"
    );
    assert!(
        check(&s, "Small", "-1").is_err(),
        "the restriction's own bound"
    );
    assert!(
        check(&s, "Small", "1.5").is_err(),
        "an integer type takes no fraction"
    );

    // unsignedLong's maximum does not fit in i64, so this also pins the
    // i128 representation.
    assert_eq!(
        check(&s, "Big", "18446744073709551615").unwrap(),
        Value::Integer(18_446_744_073_709_551_615)
    );
}

/// Regression: `xs:token`-derived types collapse; the primitive would not.
#[test]
fn token_derived_types_collapse_through_a_user_restriction() {
    let s = build(
        r#"<xs:simpleType name="Code">
             <xs:restriction base="xs:token"><xs:length value="3"/></xs:restriction>
           </xs:simpleType>"#,
    );
    assert_eq!(
        check(&s, "Code", "  a b  ").unwrap(),
        Value::String("a b".into())
    );
    assert!(check(&s, "Code", "abcd").is_err());
}

/// An enumeration on a list names whole lists, so it has to be compared item
/// by item against the item type. Comparing the list against the *literal* as
/// a string never matches, which used to make any such enumeration reject
/// every value — documents included, not only the schema's own defaults.
#[test]
fn an_enumeration_on_a_list_compares_lists() {
    let s = build(
        r#"<xs:simpleType name="Sizes">
             <xs:restriction base="xs:NMTOKENS">
               <xs:enumeration value="small large"/>
               <xs:enumeration value="one two three"/>
             </xs:restriction>
           </xs:simpleType>"#,
    );
    assert!(check(&s, "Sizes", "small large").is_ok());
    // Lists collapse, so the extra whitespace names the same value.
    assert!(check(&s, "Sizes", "  small   large ").is_ok());
    assert!(check(&s, "Sizes", "one two three").is_ok());
    // Order is part of the value, and a prefix is a different list.
    assert!(check(&s, "Sizes", "large small").is_err());
    assert!(check(&s, "Sizes", "small").is_err());
    assert!(check(&s, "Sizes", "small medium").is_err());
}

/// XSD 1.1 widened two lexical spaces, and 1.0 must still refuse them. The
/// underlying date-time library implements 1.1, so without the version these
/// went through in either mode.
#[test]
fn the_two_lexical_spaces_xsd11_widened() {
    fn accepts(version: Version, body: &str, lexical: &str) -> bool {
        let xsd = format!(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          xmlns:tns="{NS}" targetNamespace="{NS}">{body}</xs:schema>"#
        );
        let s = SchemaSetBuilder::new()
            .version(version)
            .text(xsd, "mem://main.xsd")
            .build()
            .unwrap_or_else(|d| panic!("{d}"));
        let t = s.type_(Some(NS), "T").expect("type");
        s.validator().validate(t, lexical).is_ok()
    }

    let date = r#"<xs:simpleType name="T"><xs:restriction base="xs:date"/></xs:simpleType>"#;
    let dbl = r#"<xs:simpleType name="T"><xs:restriction base="xs:double"/></xs:simpleType>"#;

    // The year 0000 is 1 BCE in 1.1 and prohibited outright in 1.0.
    assert!(accepts(Version::Xsd11, date, "0000-01-01"));
    assert!(!accepts(Version::Xsd10, date, "0000-01-01"));
    // 1.0 spells 1 BCE `-0001`, and both versions take it.
    assert!(accepts(Version::Xsd10, date, "-0001-01-01"));
    assert!(accepts(Version::Xsd11, date, "-0001-01-01"));
    // An ordinary year is unaffected in either.
    assert!(accepts(Version::Xsd10, date, "2024-02-29"));

    // 1.1 added `+INF` to the special values; 1.0 has only `INF`.
    assert!(accepts(Version::Xsd11, dbl, "+INF"));
    assert!(!accepts(Version::Xsd10, dbl, "+INF"));
    for v in [Version::Xsd10, Version::Xsd11] {
        assert!(accepts(v, dbl, "INF"));
        assert!(accepts(v, dbl, "-INF"));
        assert!(accepts(v, dbl, "NaN"));
        assert!(accepts(v, dbl, "1.5E3"));
    }
}

/// The bare `values::parse` has no schema to ask, so it reads the 1.1
/// superset. `parse_in` is the one that takes a side.
#[test]
fn a_bare_parse_reads_the_superset() {
    use xsdkit::datatypes::Builtin;
    use xsdkit::values::{parse, parse_in};

    assert!(parse(Builtin::Double, "+INF").is_ok());
    assert!(parse_in(Builtin::Double, "+INF", Version::Xsd11).is_ok());
    assert!(parse_in(Builtin::Double, "+INF", Version::Xsd10).is_err());

    // The rule reaches into a list's items, not just the top-level form.
    assert!(parse_in(Builtin::Entities, "a b", Version::Xsd10).is_ok());
}

/// `Value` is public, so whatever it holds is this crate's API. These wrappers
/// exist so that is *our* API — a consumer needs no extra dependency to take a
/// value apart, and swapping the implementation behind them stays an internal
/// change rather than a breaking one.
#[test]
fn a_parsed_value_can_be_taken_apart_without_another_dependency() {
    use xsdkit::datatypes::Builtin;
    use xsdkit::values::parse;

    let Value::DateTime(dt) = parse(Builtin::DateTime, "2024-02-29T13:45:06.5+02:00").unwrap()
    else {
        panic!("expected a dateTime")
    };
    assert_eq!((dt.year(), dt.month(), dt.day()), (2024, 2, 29));
    assert_eq!((dt.hour(), dt.minute()), (13, 45));
    assert_eq!(dt.second().to_string(), "6.5");
    assert_eq!(dt.timezone_offset().map(|t| t.minutes()), Some(120));
    // And it still renders canonically.
    assert_eq!(dt.to_string(), "2024-02-29T13:45:06.5+02:00");

    let Value::Date(d) = parse(Builtin::Date, "2024-03-01").unwrap() else {
        panic!("expected a date")
    };
    assert_eq!((d.year(), d.month(), d.day()), (2024, 3, 1));
    assert_eq!(d.timezone_offset(), None, "no timezone was written");

    let Value::Duration(dur) = parse(Builtin::Duration, "P1Y2M3DT4H5M6.5S").unwrap() else {
        panic!("expected a duration")
    };
    assert_eq!((dur.years(), dur.months()), (1, 2));
    assert_eq!((dur.days(), dur.hours(), dur.minutes()), (3, 4, 5));
    assert_eq!(dur.seconds().to_string(), "6.5");

    // A decimal converts exactly, without going through a string.
    let Value::Decimal(dec) = parse(Builtin::Decimal, "1.5").unwrap() else {
        panic!("expected a decimal")
    };
    assert_eq!(dec.to_i128_scaled(), 15 * (Decimal::SCALE / 10));

    let Value::Double(x) = parse(Builtin::Double, "-1.5E3").unwrap() else {
        panic!("expected a double")
    };
    assert_eq!(f64::from(x), -1500.0);
    let Value::Float(y) = parse(Builtin::Float, "NaN").unwrap() else {
        panic!("expected a float")
    };
    assert!(f32::from(y).is_nan());
}

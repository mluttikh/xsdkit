//! Validating values against a schema's own simple types.

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

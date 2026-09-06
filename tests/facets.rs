//! Facets that are not legal facets for the type they constrain.
//!
//! These are *schema* errors, not document errors: nothing an instance could
//! say would make `length` mean something on a duration.

use xsdkit::diagnostics::DiagCode;
use xsdkit::*;

const NS: &str = "urn:example";

fn schema(body: &str) -> String {
    format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    )
}

fn diags(body: &str) -> Diagnostics {
    SchemaSetBuilder::new()
        .version(Version::Xsd11)
        .text(schema(body), "mem://main.xsd")
        .build_with_warnings()
        .1
}

/// The one code, and how many errors carry it.
fn count(body: &str, code: DiagCode) -> usize {
    diags(body).errors().filter(|d| d.code == code).count()
}

fn clean(body: &str) {
    let d = diags(body);
    assert!(!d.has_errors(), "expected a clean build, got:\n{d}");
}

// ---------------------------------------------------------------------------
// Applicable facets
// ---------------------------------------------------------------------------

/// The rule is about the *primitive*. `xs:yearMonthDuration` reads like
/// something with a length, but it derives from `xs:duration`, and no duration
/// admits `length`.
#[test]
fn length_is_not_a_facet_of_a_duration() {
    for base in ["xs:duration", "xs:yearMonthDuration", "xs:dayTimeDuration"] {
        let body = format!(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="{base}"><xs:length value="5"/></xs:restriction>
               </xs:simpleType>"#
        );
        assert_eq!(
            count(&body, DiagCode::FacetNotApplicable),
            1,
            "length on {base}"
        );
    }
}

#[test]
fn each_primitive_admits_only_its_own_facets() {
    // Bounds order a value space; a string has none.
    assert_eq!(
        count(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:string"><xs:maxInclusive value="m"/></xs:restriction>
               </xs:simpleType>"#,
            DiagCode::FacetNotApplicable
        ),
        1
    );
    // Digit counts belong to decimal, not to every number.
    assert_eq!(
        count(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:double"><xs:fractionDigits value="2"/></xs:restriction>
               </xs:simpleType>"#,
            DiagCode::FacetNotApplicable
        ),
        1
    );
    // explicitTimezone is for the date and time types.
    assert_eq!(
        count(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:int">
                   <xs:explicitTimezone value="required"/>
                 </xs:restriction>
               </xs:simpleType>"#,
            DiagCode::FacetNotApplicable
        ),
        1
    );
    // With two values, enumerating a boolean either says nothing or
    // contradicts the type.
    assert_eq!(
        count(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:boolean"><xs:enumeration value="true"/></xs:restriction>
               </xs:simpleType>"#,
            DiagCode::FacetNotApplicable
        ),
        1
    );
}

#[test]
fn the_applicable_facets_are_still_accepted() {
    clean(
        r#"<xs:simpleType name="A">
             <xs:restriction base="xs:string"><xs:maxLength value="5"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="B">
             <xs:restriction base="xs:decimal">
               <xs:totalDigits value="6"/><xs:fractionDigits value="2"/>
               <xs:minInclusive value="0"/>
             </xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="C">
             <xs:restriction base="xs:dateTime">
               <xs:explicitTimezone value="required"/>
             </xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="D">
             <xs:restriction base="xs:duration">
               <xs:maxInclusive value="P1Y"/>
             </xs:restriction>
           </xs:simpleType>"#,
    );
}

/// A list measures items, so it takes the length facets whatever its item type
/// is — including an item type that would refuse them itself.
#[test]
fn a_list_takes_length_even_when_its_item_type_would_not() {
    clean(
        r#"<xs:simpleType name="L">
             <xs:list itemType="xs:duration"/>
           </xs:simpleType>
           <xs:simpleType name="T">
             <xs:restriction base="tns:L"><xs:maxLength value="3"/></xs:restriction>
           </xs:simpleType>"#,
    );
    // But it still has no order to bound.
    assert_eq!(
        count(
            r#"<xs:simpleType name="L"><xs:list itemType="xs:int"/></xs:simpleType>
               <xs:simpleType name="T">
                 <xs:restriction base="tns:L"><xs:minInclusive value="1"/></xs:restriction>
               </xs:simpleType>"#,
            DiagCode::FacetNotApplicable
        ),
        1
    );
}

/// A union has no lexical space of its own to measure and no single order to
/// bound; all it can do is name values and shapes.
#[test]
fn a_union_takes_only_pattern_and_enumeration() {
    clean(
        r#"<xs:simpleType name="U">
             <xs:union memberTypes="xs:int xs:date"/>
           </xs:simpleType>
           <xs:simpleType name="T">
             <xs:restriction base="tns:U"><xs:enumeration value="1"/></xs:restriction>
           </xs:simpleType>"#,
    );
    assert_eq!(
        count(
            r#"<xs:simpleType name="U"><xs:union memberTypes="xs:int xs:short"/></xs:simpleType>
               <xs:simpleType name="T">
                 <xs:restriction base="tns:U"><xs:maxInclusive value="9"/></xs:restriction>
               </xs:simpleType>"#,
            DiagCode::FacetNotApplicable
        ),
        1
    );
}

// ---------------------------------------------------------------------------
// Facet values
// ---------------------------------------------------------------------------

/// A bound has to name a value of the type it bounds. `xs:dateTimeStamp`
/// requires a timezone, so a bound without one names no instant at all.
#[test]
fn a_bound_must_be_a_value_of_the_type_it_bounds() {
    assert_eq!(
        count(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:dateTimeStamp">
                   <xs:minInclusive value="2001-01-01T00:00:00+09:00"/>
                   <xs:maxInclusive value="2005-01-01T00:00:00"/>
                 </xs:restriction>
               </xs:simpleType>"#,
            DiagCode::InvalidFacetValue
        ),
        1,
        "only the bound without a timezone is wrong"
    );
    assert_eq!(
        count(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:int"><xs:maxInclusive value="ten"/></xs:restriction>
               </xs:simpleType>"#,
            DiagCode::InvalidFacetValue
        ),
        1
    );
}

#[test]
fn an_enumerated_value_must_be_a_value_of_the_base() {
    assert_eq!(
        count(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:date">
                   <xs:enumeration value="2024-02-29"/>
                   <xs:enumeration value="2023-02-29"/>
                   <xs:enumeration value="not-a-date"/>
                 </xs:restriction>
               </xs:simpleType>"#,
            DiagCode::InvalidFacetValue
        ),
        2,
        "2023 is not a leap year, and the third is not a date at all"
    );
}

/// A QName's prefix means something only against the bindings in scope where
/// it was written, and those are not in the model. Checking one would mean
/// guessing, so these go unchecked rather than falsely rejected.
#[test]
fn qname_and_notation_facet_values_are_left_alone() {
    clean(
        r#"<xs:simpleType name="T">
             <xs:restriction base="xs:QName">
               <xs:enumeration value="tns:anything"/>
               <xs:enumeration value="bare"/>
             </xs:restriction>
           </xs:simpleType>"#,
    );
}

// ---------------------------------------------------------------------------
// Facets that contradict each other
// ---------------------------------------------------------------------------

#[test]
fn facets_that_cannot_both_hold_are_rejected() {
    let cases = [
        // `length` fixes what the other two bound.
        r#"<xs:restriction base="xs:string">
             <xs:length value="3"/><xs:minLength value="3"/></xs:restriction>"#,
        r#"<xs:restriction base="xs:string">
             <xs:length value="3"/><xs:maxLength value="9"/></xs:restriction>"#,
        r#"<xs:restriction base="xs:string">
             <xs:minLength value="9"/><xs:maxLength value="3"/></xs:restriction>"#,
        r#"<xs:restriction base="xs:int">
             <xs:minInclusive value="1"/><xs:minExclusive value="1"/></xs:restriction>"#,
        r#"<xs:restriction base="xs:int">
             <xs:maxInclusive value="1"/><xs:maxExclusive value="1"/></xs:restriction>"#,
        r#"<xs:restriction base="xs:decimal">
             <xs:totalDigits value="2"/><xs:fractionDigits value="5"/></xs:restriction>"#,
        r#"<xs:restriction base="xs:decimal"><xs:totalDigits value="0"/></xs:restriction>"#,
    ];
    for c in cases {
        let body = format!(r#"<xs:simpleType name="T">{c}</xs:simpleType>"#);
        assert_eq!(count(&body, DiagCode::ConflictingFacets), 1, "{c}");
    }
}

/// The pairs are only a contradiction on one step. Narrowing a range across
/// two restrictions is ordinary, and belongs to the derivation rules, which
/// are not implemented.
#[test]
fn narrowing_across_two_steps_is_not_a_conflict() {
    clean(
        r#"<xs:simpleType name="A">
             <xs:restriction base="xs:string"><xs:minLength value="1"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="B">
             <xs:restriction base="tns:A"><xs:maxLength value="9"/></xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="C">
             <xs:restriction base="tns:B"><xs:length value="4"/></xs:restriction>
           </xs:simpleType>"#,
    );
}

// ---------------------------------------------------------------------------
// A restriction may only narrow
// ---------------------------------------------------------------------------

/// A bound that widens the one it inherits admits values the base rejects, so
/// the derived type is not a subset of its base.
#[test]
fn a_bound_may_not_widen_the_one_it_inherits() {
    let cases = [
        (
            r#"<xs:minInclusive value="10"/>"#,
            r#"<xs:minInclusive value="5"/>"#,
        ),
        (
            r#"<xs:maxInclusive value="10"/>"#,
            r#"<xs:maxInclusive value="50"/>"#,
        ),
        (
            r#"<xs:minExclusive value="10"/>"#,
            r#"<xs:minExclusive value="5"/>"#,
        ),
        (
            r#"<xs:maxExclusive value="10"/>"#,
            r#"<xs:maxExclusive value="50"/>"#,
        ),
    ];
    for (base, derived) in cases {
        let body = format!(
            r#"<xs:simpleType name="A">
                 <xs:restriction base="xs:int">{base}</xs:restriction>
               </xs:simpleType>
               <xs:simpleType name="B">
                 <xs:restriction base="tns:A">{derived}</xs:restriction>
               </xs:simpleType>"#
        );
        assert_eq!(count(&body, DiagCode::ConflictingFacets), 1, "{derived}");
    }
}

#[test]
fn narrowing_the_same_bound_is_fine() {
    clean(
        r#"<xs:simpleType name="A">
             <xs:restriction base="xs:int">
               <xs:minInclusive value="0"/><xs:maxInclusive value="100"/>
             </xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="B">
             <xs:restriction base="tns:A">
               <xs:minInclusive value="10"/><xs:maxInclusive value="90"/>
             </xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="C">
             <xs:restriction base="tns:B"><xs:maxInclusive value="90"/></xs:restriction>
           </xs:simpleType>"#,
    );
}

/// Sizes and digit counts narrow the same way, and `length` fixes the size
/// outright — an inherited one it disagrees with is a contradiction, not a
/// widening.
#[test]
fn sizes_and_digit_counts_may_only_narrow() {
    let cases = [
        (
            r#"<xs:string"#,
            r#"<xs:minLength value="5"/>"#,
            r#"<xs:minLength value="2"/>"#,
        ),
        (
            r#"<xs:string"#,
            r#"<xs:maxLength value="5"/>"#,
            r#"<xs:maxLength value="9"/>"#,
        ),
        (
            r#"<xs:string"#,
            r#"<xs:length value="5"/>"#,
            r#"<xs:length value="4"/>"#,
        ),
        (
            r#"<xs:decimal"#,
            r#"<xs:totalDigits value="4"/>"#,
            r#"<xs:totalDigits value="8"/>"#,
        ),
        (
            r#"<xs:decimal"#,
            r#"<xs:fractionDigits value="1"/>"#,
            r#"<xs:fractionDigits value="3"/>"#,
        ),
    ];
    for (prim, base, derived) in cases {
        let body = format!(
            r#"<xs:simpleType name="A">
                 <xs:restriction base="{p}">{base}</xs:restriction>
               </xs:simpleType>
               <xs:simpleType name="B">
                 <xs:restriction base="tns:A">{derived}</xs:restriction>
               </xs:simpleType>"#,
            p = prim.trim_start_matches('<')
        );
        assert_eq!(count(&body, DiagCode::ConflictingFacets), 1, "{derived}");
    }
}

/// The narrowing rule alone misses a minimum raised above an *inherited*
/// maximum: nothing widened, and the type still accepts nothing. The composed
/// set is what catches it.
#[test]
fn the_composed_range_must_hold_something() {
    assert_eq!(
        count(
            r#"<xs:simpleType name="A">
                 <xs:restriction base="xs:int"><xs:maxInclusive value="10"/></xs:restriction>
               </xs:simpleType>
               <xs:simpleType name="B">
                 <xs:restriction base="tns:A"><xs:minInclusive value="20"/></xs:restriction>
               </xs:simpleType>"#,
            DiagCode::ConflictingFacets
        ),
        1
    );
    // Both bounds on one step, which is the ordinary way to get it wrong.
    assert_eq!(
        count(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:yearMonthDuration">
                   <xs:minInclusive value="P1Y10M"/>
                   <xs:maxInclusive value="P1Y1M"/>
                 </xs:restriction>
               </xs:simpleType>"#,
            DiagCode::ConflictingFacets
        ),
        1
    );
    // Equal bounds admit that one value — unless an end excludes it, and then
    // the type accepts nothing at all.
    clean(
        r#"<xs:simpleType name="T">
             <xs:restriction base="xs:int">
               <xs:minInclusive value="7"/><xs:maxInclusive value="7"/>
             </xs:restriction>
           </xs:simpleType>"#,
    );
    assert_eq!(
        count(
            r#"<xs:simpleType name="T">
                 <xs:restriction base="xs:yearMonthDuration">
                   <xs:minExclusive value="P1Y1M"/>
                   <xs:maxExclusive value="P1Y1M"/>
                 </xs:restriction>
               </xs:simpleType>"#,
            DiagCode::ConflictingFacets
        ),
        1
    );
}

/// A facet may be declared once per restriction step — including on a
/// `simpleContent` restriction, whose facets used to be read by nothing at
/// all and so could not collide.
#[test]
fn a_repeated_facet_under_simple_content_is_reported() {
    let xsd = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                   xmlns:tns="urn:example" targetNamespace="urn:example">
        <xs:complexType name="Base">
          <xs:simpleContent><xs:extension base="xs:string"/></xs:simpleContent>
        </xs:complexType>
        <xs:complexType name="T">
          <xs:simpleContent>
            <xs:restriction base="tns:Base">
              <xs:maxLength value="2"/>
              <xs:maxLength value="2"/>
            </xs:restriction>
          </xs:simpleContent>
        </xs:complexType>
      </xs:schema>"#;
    let (_, d) = SchemaSetBuilder::new()
        .text(xsd.to_string(), "mem://sc.xsd")
        .build_with_warnings();
    assert!(
        d.errors().any(|e| e.code == DiagCode::ConflictingFacets),
        "expected a duplicate-facet report, got:\n{d}"
    );
}

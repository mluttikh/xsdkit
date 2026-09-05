//! A guard on how loading scales, not on how fast it is.
//!
//! The loader builds a `Span` for every component, and a span needs a line
//! number. Asking the XML parser for one costs a scan from the start of the
//! document, so doing it per declaration is quadratic — a 1600-element schema
//! took 2.8 seconds before `LineIndex` turned the lookup into a binary search,
//! and 8 milliseconds after.
//!
//! The bound below is deliberately loose. It is not a benchmark: it exists so
//! that reintroducing a per-component rescan fails a test instead of quietly
//! making large schemas unusable.

use std::time::{Duration, Instant};
use xsdkit::SchemaSetBuilder;

const XS: &str = "http://www.w3.org/2001/XMLSchema";

/// A schema with `n` complex types, each with two elements and an attribute,
/// and `n` global elements referring to them. Every one of those carries a
/// span, which is the point.
fn schema_with(n: usize) -> String {
    let mut s =
        format!(r#"<xs:schema xmlns:xs="{XS}" xmlns:tns="urn:scale" targetNamespace="urn:scale">"#);
    for i in 0..n {
        s.push_str(&format!(
            r#"<xs:complexType name="T{i}"><xs:sequence>
                 <xs:element name="a{i}" type="xs:string"/>
                 <xs:element name="b{i}" type="xs:int" minOccurs="0"/>
               </xs:sequence>
               <xs:attribute name="k{i}" type="xs:string"/>
             </xs:complexType>
             <xs:element name="e{i}" type="tns:T{i}"/>"#
        ));
    }
    s.push_str("</xs:schema>");
    s
}

fn load(src: &str) -> Duration {
    let start = Instant::now();
    let set = SchemaSetBuilder::new()
        .text(src, "urn:scale")
        .build()
        .expect("the generated schema must compile");
    let elapsed = start.elapsed();
    assert!(set.element(Some("urn:scale"), "e0").is_some());
    elapsed
}

#[test]
fn loading_a_large_schema_does_not_scale_quadratically() {
    // Warm up: the first load pays for lazily built built-in types.
    load(&schema_with(16));

    let small = schema_with(750);
    let large = schema_with(3000);

    // Take the best of three. A loaded CI machine inflates individual runs,
    // but it cannot make one run faster than the work it does.
    let best = |src: &str| (0..3).map(|_| load(src)).min().unwrap();
    let small = best(&small);
    let large = best(&large);

    // Four times the input. Linear says four times the time; quadratic says
    // sixteen. Eight leaves room for cache effects and a noisy machine while
    // still failing on a rescan.
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-6);
    assert!(
        ratio < 8.0,
        "loading grew {ratio:.1}x for 4x the input ({small:?} -> {large:?}); \
         a per-component line scan is the usual cause"
    );

    // An absolute ceiling as well, so a uniformly slow loader is caught too.
    // A debug build does this in about 0.2 s; the quadratic version took 5 s
    // here and would grow with the schema.
    assert!(
        large < Duration::from_secs(20),
        "3000 declarations took {large:?}, which is far past anything reasonable"
    );
}

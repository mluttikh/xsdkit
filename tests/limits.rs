//! What a schema document may not do to the process loading it.

use xsdkit::*;

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn compile(xsd: String, max_depth: Option<u32>) -> Compilation {
    let b = SchemaSetBuilder::new().text(xsd, "mem://deep.xsd");
    match max_depth {
        Some(limit) => b.max_depth(limit),
        None => b,
    }
    .compile()
}

fn codes(c: &Compilation) -> Vec<&'static str> {
    c.diagnostics.errors().map(|d| d.code.as_str()).collect()
}

/// `levels` anonymous types, each inside the last: three elements deep per
/// level, and the deepest recursion the loader has.
fn nested_types(levels: usize) -> String {
    format!(
        r#"<xs:schema xmlns:xs="{XS}">{}{}</xs:schema>"#,
        r#"<xs:element name="e"><xs:complexType><xs:sequence>"#.repeat(levels),
        "</xs:sequence></xs:complexType></xs:element>".repeat(levels)
    )
}

/// Runs `f` where a debug build's recursion has room. The limit is sized for a
/// release build; unoptimised, the loader spends several times the stack per
/// level, and a test thread has 2 MiB.
fn with_room<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(256 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("join")
}

/// Parsing recurses once per level of nesting, and so does the loader. A
/// schema nested a few thousand deep used to overflow the stack and abort the
/// process, which no caller can catch.
///
/// On an ordinary test thread, deliberately: refusing it before any recursion
/// starts is the thing under test.
#[test]
fn a_schema_nested_past_the_limit_is_refused_before_it_is_parsed() {
    assert_eq!(codes(&compile(nested_types(100_000), None)), ["XSD1001"]);

    let plain = format!(
        r#"<xs:schema xmlns:xs="{XS}"><xs:annotation><xs:appinfo>{}{}</xs:appinfo></xs:annotation></xs:schema>"#,
        "<a>".repeat(1_000_000),
        "</a>".repeat(1_000_000)
    );
    assert_eq!(codes(&compile(plain, None)), ["XSD1001"]);
}

#[test]
fn the_default_admits_realistic_nesting_and_the_limit_is_adjustable() {
    assert_eq!(DEFAULT_MAX_DEPTH, 256);
    // 80 levels of anonymous types is 241 elements deep, with the root.
    with_room(|| assert!(codes(&compile(nested_types(80), None)).is_empty()));
    // 90 is 271, past the default, and loads once the limit allows it.
    assert_eq!(codes(&compile(nested_types(90), None)), ["XSD1001"]);
    with_room(|| assert!(codes(&compile(nested_types(90), Some(300))).is_empty()));
    // And a lower limit refuses what the default would take.
    assert_eq!(codes(&compile(nested_types(10), Some(20))), ["XSD1001"]);
}

/// An entity's replacement text opens elements wherever it is referenced, so
/// markup in the internal subset counts toward the limit, or a document could
/// hide its depth there.
#[test]
fn nesting_an_entity_would_insert_counts_toward_the_limit() {
    let inner = "<a>".repeat(200) + &"</a>".repeat(200);
    let xsd = format!(
        r#"<!DOCTYPE xs:schema [<!ENTITY deep "{inner}">]><xs:schema xmlns:xs="{XS}"><xs:annotation><xs:appinfo>{}&deep;{}</xs:appinfo></xs:annotation></xs:schema>"#,
        "<b>".repeat(100),
        "</b>".repeat(100)
    );
    assert_eq!(codes(&compile(xsd, None)), ["XSD1001"]);
}

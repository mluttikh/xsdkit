//! Decoding a document into a tree of typed values.

use xsdkit::*;

const NS: &str = "urn:example";

fn schema(body: &str) -> Schemas {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .text(xsd, "mem://main.xsd")
        .compile()
        .into_result()
        .unwrap_or_else(|d| panic!("{d}"))
}

fn decode(s: &Schemas, xml: &str) -> Decoded {
    s.decode(xml)
        .into_result()
        .unwrap_or_else(|d| panic!("expected a valid document, got:\n{d}"))
}

/// The schema most of these use: a `report` of `item`s.
fn reports() -> Schemas {
    schema(
        r#"<xs:element name="report">
             <xs:complexType>
               <xs:sequence>
                 <xs:element name="title" type="xs:string"/>
                 <xs:element name="count" type="xs:int"/>
                 <xs:element name="item" maxOccurs="unbounded" minOccurs="0">
                   <xs:complexType>
                     <xs:sequence>
                       <xs:element name="price" type="xs:decimal"/>
                     </xs:sequence>
                     <xs:attribute name="sku" type="xs:string"/>
                   </xs:complexType>
                 </xs:element>
               </xs:sequence>
               <xs:attribute name="id" type="xs:ID"/>
             </xs:complexType>
           </xs:element>"#,
    )
}

#[test]
fn values_arrive_in_their_value_space() {
    let s = reports();
    let d = decode(
        &s,
        r#"<report xmlns="urn:example" id="r1">
             <title>Q3</title><count>42</count>
           </report>"#,
    );

    let title = d.child(s.qname(Some(NS), "title").unwrap()).unwrap();
    let count = d.child(s.qname(Some(NS), "count").unwrap()).unwrap();

    assert_eq!(title.value(), Some(&Value::String("Q3".into())));
    // Not `"42"`. The whole point of decoding over a PSVI is that the parse
    // already happened.
    assert_eq!(count.value(), Some(&Value::Integer(42)));
}

#[test]
fn attributes_are_typed_too() {
    let s = reports();
    let d = decode(
        &s,
        r#"<report xmlns="urn:example" id="r1"><title>t</title><count>1</count></report>"#,
    );
    let id = d.attribute(s.qname(None, "id").unwrap()).unwrap();
    assert_eq!(id.lexical, "r1");
    assert!(matches!(id.value, Some(Value::String(_))));
    assert!(!id.from_schema);
}

#[test]
fn repeated_children_keep_document_order() {
    let s = reports();
    let d = decode(
        &s,
        r#"<report xmlns="urn:example"><title>t</title><count>2</count>
             <item sku="a"><price>1.50</price></item>
             <item sku="b"><price>2.50</price></item>
           </report>"#,
    );
    let item = s.qname(Some(NS), "item").unwrap();
    let skus: Vec<_> = d
        .children()
        .iter()
        .filter(|c| c.name == item)
        .map(|c| {
            c.attribute(s.qname(None, "sku").unwrap())
                .unwrap()
                .lexical
                .clone()
        })
        .collect();
    assert_eq!(skus, vec!["a", "b"]);
}

#[test]
fn nesting_is_preserved() {
    let s = reports();
    let d = decode(
        &s,
        r#"<report xmlns="urn:example"><title>t</title><count>1</count>
             <item sku="a"><price>9.99</price></item>
           </report>"#,
    );
    let item = d.child(s.qname(Some(NS), "item").unwrap()).unwrap();
    let price = item.child(s.qname(Some(NS), "price").unwrap()).unwrap();
    assert_eq!(price.text(), "9.99");
    assert!(matches!(price.value(), Some(Value::Decimal(_))));
}

#[test]
fn nil_is_not_the_same_as_empty() {
    let s = schema(
        r#"<xs:element name="e" type="xs:int" nillable="true"/>
           <xs:element name="f" type="xs:string"/>"#,
    );
    let nil = decode(
        &s,
        r#"<e xmlns="urn:example"
              xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
              xsi:nil="true"/>"#,
    );
    assert!(nil.nil);
    assert_eq!(nil.content, DecodedContent::Empty);

    // An empty string is a value; nil is the absence of one.
    let empty = decode(&s, r#"<f xmlns="urn:example"></f>"#);
    assert!(!empty.nil);
    assert_eq!(empty.value(), Some(&Value::String(String::new())));
}

#[test]
fn a_schema_supplied_default_says_so() {
    let s = schema(r#"<xs:element name="e" type="xs:string" default="fallback"/>"#);
    let d = decode(&s, r#"<e xmlns="urn:example"/>"#);
    assert_eq!(d.text(), "fallback");
    match d.content {
        DecodedContent::Simple { from_schema, .. } => assert!(
            from_schema,
            "the document did not carry this value; the schema did"
        ),
        other => panic!("expected simple content, got {other:?}"),
    }
}

#[test]
fn mixed_content_keeps_both_halves() {
    let s = schema(
        r#"<xs:element name="p">
             <xs:complexType mixed="true">
               <xs:sequence>
                 <xs:element name="b" type="xs:string" maxOccurs="unbounded"/>
               </xs:sequence>
             </xs:complexType>
           </xs:element>"#,
    );
    let d = decode(&s, r#"<p xmlns="urn:example">before<b>bold</b>after</p>"#);

    assert_eq!(d.children().len(), 1);
    assert_eq!(d.children()[0].text(), "bold");
    // The runs are concatenated: what was said, not where. Documented on
    // `DecodedContent::Elements`.
    assert_eq!(d.text(), "beforeafter");
}

#[test]
fn an_invalid_document_still_decodes_and_still_reports() {
    let s = reports();
    let decoding = s.decode(
        r#"<report xmlns="urn:example"><title>t</title><count>not-a-number</count></report>"#,
    );
    assert!(!decoding.is_valid());
    // The tree is still there: a document worth reading can still be wrong,
    // and which of those matters is the caller's call.
    let d = decoding
        .decoded
        .as_ref()
        .expect("a tree, despite the error");
    let count = d.child(s.qname(Some(NS), "count").unwrap()).unwrap();
    assert_eq!(count.text(), "not-a-number");
    assert_eq!(
        count.value(),
        None,
        "it did not parse, so there is no value"
    );
    assert!(decoding.into_result().is_err());
}

#[test]
fn an_empty_document_decodes_to_nothing() {
    let s = reports();
    let decoding = s.decode("");
    assert!(decoding.decoded.is_none());
    assert!(!decoding.is_valid());
}

#[test]
fn text_that_was_never_a_document_says_so() {
    // A file *name* where the file's *contents* belong is the overwhelmingly
    // common way to get here, so the diagnostic names it.
    let s = reports();
    let d = s.decode("report.xml").diagnostics;
    let rendered = format!("{d}");
    assert!(rendered.contains("no root element"), "{rendered}");
    assert!(rendered.contains("not a path"), "{rendered}");

    // Genuinely empty input is a different mistake and gets no such guess.
    let empty = format!("{}", s.decode("").diagnostics);
    assert!(empty.contains("no root element"), "{empty}");
    assert!(!empty.contains("not a path"), "{empty}");
}

#[test]
fn a_skipped_wildcard_subtree_does_not_swallow_the_document() {
    // The validator announces a skipped element with `StartElement` but used
    // to close it with nothing, so a tree builder leaked a frame: every later
    // element was attributed to its grandparent and the root was never
    // popped, which decoded a perfectly valid document to `None`.
    let s = schema(
        r###"<xs:element name="root">
             <xs:complexType><xs:sequence>
               <xs:element name="a" type="xs:string"/>
               <xs:any namespace="##other" processContents="skip" minOccurs="0"/>
             </xs:sequence></xs:complexType>
           </xs:element>"###,
    );
    let d = decode(
        &s,
        r#"<root xmlns="urn:example" xmlns:o="urn:other">
             <a>x</a><o:junk><o:deep/></o:junk>
           </root>"#,
    );
    assert_eq!(d.children().len(), 2, "the wildcard child belongs to root");
    assert_eq!(d.children()[0].text(), "x");
}

#[test]
fn an_empty_complex_element_is_not_a_value() {
    // `children().is_empty()` is true for an element-only type whose children
    // are all absent, which is not the same question as "has a value".
    let s = schema(
        r#"<xs:element name="e">
             <xs:complexType>
               <xs:sequence>
                 <xs:element name="c" type="xs:string" minOccurs="0"/>
               </xs:sequence>
               <xs:attribute name="a" type="xs:string"/>
             </xs:complexType>
           </xs:element>"#,
    );
    let d = decode(&s, r#"<e xmlns="urn:example" a="x"/>"#);
    assert_eq!(d.content, DecodedContent::Empty);
    assert_eq!(d.value(), None, "empty complex content carries no value");
    assert_eq!(d.attributes.len(), 1);
}

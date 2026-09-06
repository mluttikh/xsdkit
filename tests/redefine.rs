//! `xs:redefine` and `xs:override` — including a document, then changing it.

use fxhash::FxHashMap;
use xsdkit::model::Term;
use xsdkit::*;

const NS: &str = "urn:example";

#[derive(Default)]
struct MapResolver(FxHashMap<String, String>);

impl MapResolver {
    fn with(mut self, name: &str, xsd: &str) -> Self {
        self.0.insert(name.to_string(), xsd.to_string());
        self
    }
}

impl Resolver for MapResolver {
    fn resolve(&self, location: &str, _base: Option<&str>) -> Result<(String, Vec<u8>), String> {
        self.0
            .get(location)
            .map(|t| (location.to_string(), t.clone().into_bytes()))
            .ok_or_else(|| format!("not in map: {location}"))
    }
}

fn part(body: &str) -> String {
    format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:tns="{NS}"
                      targetNamespace="{NS}">{body}</xs:schema>"#
    )
}

fn build(part_body: &str, main_body: &str) -> Schemas {
    let main = part(main_body);
    SchemaSetBuilder::new()
        .resolver(MapResolver::default().with("part.xsd", &part(part_body)))
        .text(main, "mem://main.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"))
}

/// Local names of a complex type's child elements.
fn children(s: &Schemas, name: &str) -> Vec<String> {
    let t = s.type_id(Some(NS), name).expect("type");
    s.possible_children(t)
        .into_iter()
        .map(|e| s.names().resolve(s[e].name.local).to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// redefine
// ---------------------------------------------------------------------------

/// The whole trick: inside a redefinition, a reference to the name being
/// redefined means the **original**, not the thing being declared.
#[test]
fn a_redefined_type_extends_the_original() {
    let s = build(
        r#"<xs:complexType name="Person">
             <xs:sequence><xs:element name="name" type="xs:string"/></xs:sequence>
           </xs:complexType>"#,
        r#"<xs:redefine schemaLocation="part.xsd">
             <xs:complexType name="Person">
               <xs:complexContent>
                 <xs:extension base="tns:Person">
                   <xs:sequence><xs:element name="email" type="xs:string"/></xs:sequence>
                 </xs:extension>
               </xs:complexContent>
             </xs:complexType>
           </xs:redefine>"#,
    );
    // The name now resolves to the redefinition, which carries both.
    assert_eq!(children(&s, "Person"), ["name", "email"]);
}

/// Without the self-reference trick this would be an infinite regress; with a
/// naive implementation it silently produces the original instead.
#[test]
fn a_redefinition_replaces_what_the_name_resolves_to() {
    let s = build(
        r#"<xs:complexType name="Person">
             <xs:sequence><xs:element name="name" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:element name="person" type="tns:Person"/>"#,
        r#"<xs:redefine schemaLocation="part.xsd">
             <xs:complexType name="Person">
               <xs:complexContent><xs:extension base="tns:Person">
                 <xs:sequence><xs:element name="email" type="xs:string"/></xs:sequence>
               </xs:extension></xs:complexContent>
             </xs:complexType>
           </xs:redefine>"#,
    );
    // An element declared in the *included* document sees the redefinition.
    let e = s.element_id(Some(NS), "person").unwrap();
    let names: Vec<_> = s
        .possible_children(s[e].type_id)
        .into_iter()
        .map(|x| s.names().resolve(s[x].name.local).to_string())
        .collect();
    assert_eq!(
        names,
        ["name", "email"],
        "the element must see the redefined type"
    );
}

#[test]
fn a_redefined_simple_type_restricts_the_original() {
    let s = build(
        r#"<xs:simpleType name="Code">
             <xs:restriction base="xs:string"><xs:maxLength value="10"/></xs:restriction>
           </xs:simpleType>"#,
        r#"<xs:redefine schemaLocation="part.xsd">
             <xs:simpleType name="Code">
               <xs:restriction base="tns:Code"><xs:maxLength value="4"/></xs:restriction>
             </xs:simpleType>
           </xs:redefine>"#,
    );
    let v = s.validator();
    let code = s.type_id(Some(NS), "Code").unwrap();
    assert!(v.validate(code, "abcd").is_ok());
    assert!(
        v.validate(code, "abcde").is_err(),
        "the redefinition narrowed it"
    );
}

#[test]
fn a_redefined_group_references_the_original() {
    let s = build(
        r#"<xs:group name="G">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:group>"#,
        r#"<xs:redefine schemaLocation="part.xsd">
             <xs:group name="G">
               <xs:sequence>
                 <xs:group ref="tns:G"/>
                 <xs:element name="b" type="xs:string"/>
               </xs:sequence>
             </xs:group>
           </xs:redefine>
           <xs:complexType name="Uses">
             <xs:sequence><xs:group ref="tns:G"/></xs:sequence>
           </xs:complexType>"#,
    );
    assert_eq!(children(&s, "Uses"), ["a", "b"], "the group grew by one");
}

#[test]
fn a_redefined_attribute_group_references_the_original() {
    let s = build(
        r#"<xs:attributeGroup name="Common">
             <xs:attribute name="id" type="xs:string"/>
           </xs:attributeGroup>"#,
        r#"<xs:redefine schemaLocation="part.xsd">
             <xs:attributeGroup name="Common">
               <xs:attributeGroup ref="tns:Common"/>
               <xs:attribute name="lang" type="xs:string"/>
             </xs:attributeGroup>
           </xs:redefine>
           <xs:complexType name="Doc">
             <xs:attributeGroup ref="tns:Common"/>
           </xs:complexType>"#,
    );
    let mut names: Vec<_> = s
        .attribute_uses(s.type_id(Some(NS), "Doc").unwrap())
        .iter()
        .map(|u| s.names().resolve(s[u.attribute].name.local).to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["id", "lang"]);
}

/// Only the name being redefined is special; every other reference in the
/// redefinition means what it normally means.
#[test]
fn other_references_inside_a_redefinition_are_ordinary() {
    let s = build(
        r#"<xs:complexType name="Person">
             <xs:sequence><xs:element name="name" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:simpleType name="Code">
             <xs:restriction base="xs:string"><xs:maxLength value="4"/></xs:restriction>
           </xs:simpleType>"#,
        r#"<xs:redefine schemaLocation="part.xsd">
             <xs:complexType name="Person">
               <xs:complexContent><xs:extension base="tns:Person">
                 <xs:sequence><xs:element name="code" type="tns:Code"/></xs:sequence>
               </xs:extension></xs:complexContent>
             </xs:complexType>
           </xs:redefine>"#,
    );
    assert_eq!(children(&s, "Person"), ["name", "code"]);
    // `tns:Code` is not being redefined, so it is the ordinary one.
    let t = s.type_id(Some(NS), "Person").unwrap();
    let code_el = s
        .possible_children(t)
        .into_iter()
        .find(|e| s.names().resolve(s[*e].name.local) == "code")
        .unwrap();
    assert_eq!(s[code_el].type_id, s.type_id(Some(NS), "Code").unwrap());
}

#[test]
fn redefining_a_builtin_is_rejected() {
    let main = part(
        r#"<xs:redefine schemaLocation="part.xsd">
             <xs:simpleType name="string">
               <xs:restriction base="xs:string"/>
             </xs:simpleType>
           </xs:redefine>"#,
    );
    // Target the XSD namespace so the name collides with a built-in.
    let main = main.replace(
        &format!("targetNamespace=\"{NS}\""),
        "targetNamespace=\"http://www.w3.org/2001/XMLSchema\"",
    );
    let d = SchemaSetBuilder::new()
        .resolver(MapResolver::default().with("part.xsd", &part("")))
        .text(main, "mem://main.xsd")
        .build()
        .expect_err("redefining a built-in must fail");
    assert!(
        d.errors().any(|e| e.code == DiagCode::DuplicateGlobal),
        "{d}"
    );
}

// ---------------------------------------------------------------------------
// override
// ---------------------------------------------------------------------------

/// `xs:override` replaces outright. Unlike `redefine`, its own references
/// mean the **new** components, so there is no self-reference to untangle.
#[test]
fn an_override_replaces_a_type_outright() {
    let s = build(
        r#"<xs:complexType name="Person">
             <xs:sequence>
               <xs:element name="name" type="xs:string"/>
               <xs:element name="fax" type="xs:string"/>
             </xs:sequence>
           </xs:complexType>
           <xs:element name="person" type="tns:Person"/>"#,
        r#"<xs:override schemaLocation="part.xsd">
             <xs:complexType name="Person">
               <xs:sequence>
                 <xs:element name="name" type="xs:string"/>
                 <xs:element name="email" type="xs:string"/>
               </xs:sequence>
             </xs:complexType>
           </xs:override>"#,
    );
    assert_eq!(children(&s, "Person"), ["name", "email"], "fax is gone");

    let e = s.element_id(Some(NS), "person").unwrap();
    let names: Vec<_> = s
        .possible_children(s[e].type_id)
        .into_iter()
        .map(|x| s.names().resolve(s[x].name.local).to_string())
        .collect();
    assert_eq!(names, ["name", "email"], "the element sees the override");
}

#[test]
fn an_override_replaces_a_group() {
    let s = build(
        r#"<xs:group name="G">
             <xs:sequence><xs:element name="old" type="xs:string"/></xs:sequence>
           </xs:group>"#,
        r#"<xs:override schemaLocation="part.xsd">
             <xs:group name="G">
               <xs:sequence><xs:element name="new" type="xs:string"/></xs:sequence>
             </xs:group>
           </xs:override>
           <xs:complexType name="Uses">
             <xs:sequence><xs:group ref="tns:G"/></xs:sequence>
           </xs:complexType>"#,
    );
    assert_eq!(children(&s, "Uses"), ["new"]);
}

/// `xs:override` may replace *any* top-level component, not just the four
/// `xs:redefine` allows. An element declaration is the common case, and
/// ignoring it left the overridden document's own version in force.
#[test]
fn an_override_replaces_an_element_declaration() {
    let s = build(
        r#"<xs:element name="doc">
             <xs:complexType>
               <xs:sequence><xs:element name="para" type="xs:string"/></xs:sequence>
             </xs:complexType>
           </xs:element>"#,
        r#"<xs:override schemaLocation="part.xsd">
             <xs:element name="doc" type="xs:date"/>
           </xs:override>"#,
    );
    let e = s.element_id(Some(NS), "doc").expect("doc is declared");
    let ty = s[e].type_id;
    assert!(s[ty].is_simple(), "the override made `doc` a simple type");
    // And the element-only content model it used to have is gone.
    assert!(s.possible_children(ty).is_empty());
}

#[test]
fn an_override_replaces_an_attribute_declaration() {
    let s = build(
        r#"<xs:attribute name="a" type="xs:string"/>"#,
        r#"<xs:override schemaLocation="part.xsd">
             <xs:attribute name="a" type="xs:int"/>
           </xs:override>"#,
    );
    let a = s.attribute_id(Some(NS), "a").expect("a is declared");
    let name = s[s[a].type_id].name().map(|n| s.display_name(n));
    assert_eq!(
        name.as_deref(),
        Some("{http://www.w3.org/2001/XMLSchema}int")
    );
}

/// `xs:redefine` is limited to types and the two kinds of group, so an
/// element there is still unrecognised rather than silently applied.
#[test]
fn a_redefine_does_not_take_an_element() {
    let (_, diags) = SchemaSetBuilder::new()
        .resolver(MapResolver::default().with(
            "part.xsd",
            &part(r#"<xs:element name="doc" type="xs:string"/>"#),
        ))
        .text(
            part(
                r#"<xs:redefine schemaLocation="part.xsd">
                     <xs:element name="doc" type="xs:date"/>
                   </xs:redefine>"#,
            ),
            "mem://main.xsd",
        )
        .build_with_warnings();
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("ignoring `xs:element`")),
        "expected the element to be refused inside a redefine:\n{diags}"
    );
}

/// Components the modification does not mention come through untouched.
#[test]
fn untouched_components_survive() {
    let s = build(
        r#"<xs:complexType name="Person">
             <xs:sequence><xs:element name="name" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="Company">
             <xs:sequence><xs:element name="title" type="xs:string"/></xs:sequence>
           </xs:complexType>"#,
        r#"<xs:override schemaLocation="part.xsd">
             <xs:complexType name="Person">
               <xs:sequence><xs:element name="email" type="xs:string"/></xs:sequence>
             </xs:complexType>
           </xs:override>"#,
    );
    assert_eq!(children(&s, "Person"), ["email"]);
    assert_eq!(children(&s, "Company"), ["title"], "untouched");
}

#[test]
fn a_missing_schema_location_is_reported() {
    let d = SchemaSetBuilder::new()
        .text(part(r#"<xs:redefine/>"#), "mem://main.xsd")
        .build()
        .expect_err("a redefine without a location must fail");
    assert!(
        d.errors().any(|e| e.code == DiagCode::MissingAttribute),
        "{d}"
    );
}

#[test]
fn redefinitions_no_longer_warn_as_unsupported() {
    let (_, d) = SchemaSetBuilder::new()
        .resolver(MapResolver::default().with(
            "part.xsd",
            &part(r#"<xs:simpleType name="C"><xs:restriction base="xs:string"/></xs:simpleType>"#),
        ))
        .text(
            part(
                r#"<xs:redefine schemaLocation="part.xsd">
                      <xs:simpleType name="C">
                        <xs:restriction base="tns:C"><xs:maxLength value="2"/></xs:restriction>
                      </xs:simpleType>
                    </xs:redefine>"#,
            ),
            "mem://main.xsd",
        )
        .build_with_warnings();
    assert!(
        !d.iter().any(|x| x.code == DiagCode::Unsupported),
        "redefine is implemented now:\n{d}"
    );
    let _ = Term::Element;
}

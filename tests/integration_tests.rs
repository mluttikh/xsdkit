//! End-to-end tests: real schema documents in, components out.

use fxhash::FxHashMap;
use xsdkit::datatypes::{Builtin, Variety, WhiteSpace};
use xsdkit::model::{Compositor, NamespaceConstraint, Term};
use xsdkit::*;

const NS: &str = "urn:example";

/// Serves schema documents from memory, so composition tests need no files.
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

fn build(xsd: &str) -> Schemas {
    SchemaSetBuilder::new()
        .text(xsd, "mem://main.xsd")
        .build()
        .unwrap_or_else(|d| panic!("expected a clean build, got:\n{d}"))
}

fn errors(xsd: &str) -> Diagnostics {
    SchemaSetBuilder::new()
        .text(xsd, "mem://main.xsd")
        .build()
        .expect_err("expected diagnostics")
}

/// Local names of the element particles directly under a complex type.
fn child_elements(s: &Schemas, ty: TypeId) -> Vec<String> {
    let Some(c) = s[ty].as_complex() else {
        return Vec::new();
    };
    let Some(p) = c.content.particle() else {
        return Vec::new();
    };
    s.child_particles(p)
        .into_iter()
        .filter_map(|cp| match s[cp].term {
            Term::Element(e) => Some(s.names().resolve(s[e].name.local).to_string()),
            _ => None,
        })
        .collect()
}

fn schema(body: &str) -> String {
    format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    )
}

// ---------------------------------------------------------------------------
// Built-ins
// ---------------------------------------------------------------------------

#[test]
fn builtins_are_real_components() {
    let s = build(&schema(""));
    let string = s
        .type_(Some("http://www.w3.org/2001/XMLSchema"), "string")
        .unwrap();
    assert_eq!(s.as_builtin(string), Some(Builtin::String));
    assert_eq!(s.builtin(Builtin::String), string);

    // A reference to xs:int resolves the same way a user type does.
    let int = s.builtin(Builtin::Int);
    assert!(s.derives_from(int, s.builtin(Builtin::Decimal)));
    assert!(s.derives_from(int, s.builtin(Builtin::AnyType)));
    assert_eq!(s.component_counts().types, 50);
}

// ---------------------------------------------------------------------------
// Declarations and types
// ---------------------------------------------------------------------------

#[test]
fn inline_complex_type_yields_child_particles() {
    let s = build(&schema(
        r#"<xs:element name="report">
             <xs:complexType>
               <xs:sequence>
                 <xs:element name="title" type="xs:string"/>
                 <xs:element name="count" type="xs:int"/>
               </xs:sequence>
             </xs:complexType>
           </xs:element>"#,
    ));
    let report = s.element(Some(NS), "report").expect("global element");
    let ty = s[report].type_id;
    assert_eq!(child_elements(&s, ty), ["title", "count"]);

    let c = s[ty].as_complex().unwrap();
    let p = c.content.particle().unwrap();
    match &s[p].term {
        Term::Group(g) => assert_eq!(g.compositor, Compositor::Sequence),
        other => panic!("expected a sequence, got {other:?}"),
    }
}

#[test]
fn named_types_resolve_across_declarations() {
    let s = build(&schema(
        r#"<xs:complexType name="Station">
             <xs:sequence><xs:element name="id" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:element name="station" type="tns:Station"/>"#,
    ));
    let e = s.element(Some(NS), "station").unwrap();
    let named = s.type_(Some(NS), "Station").unwrap();
    assert_eq!(s[e].type_id, named);
}

#[test]
fn an_element_without_a_type_is_any_type() {
    let s = build(&schema(r#"<xs:element name="loose"/>"#));
    let e = s.element(Some(NS), "loose").unwrap();
    assert_eq!(s[e].type_id, s.builtin(Builtin::AnyType));
}

#[test]
fn element_form_default_governs_local_qualification() {
    let qualified = build(&schema(
        r#"<xs:element name="a">
             <xs:complexType><xs:sequence>
               <xs:element name="inner" type="xs:string"/>
             </xs:sequence></xs:complexType>
           </xs:element>"#,
    ));
    let ty = qualified[qualified.element(Some(NS), "a").unwrap()].type_id;
    let p = qualified[ty]
        .as_complex()
        .unwrap()
        .content
        .particle()
        .unwrap();
    let inner = match qualified[qualified.child_particles(p)[0]].term {
        Term::Element(e) => e,
        _ => unreachable!(),
    };
    assert!(
        qualified[inner].name.ns.is_some(),
        "elementFormDefault=qualified"
    );

    let unqualified = SchemaSetBuilder::new()
        .text(
            format!(
                r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                              targetNamespace="{NS}">
                     <xs:element name="a"><xs:complexType><xs:sequence>
                       <xs:element name="inner" type="xs:string"/>
                     </xs:sequence></xs:complexType></xs:element>
                   </xs:schema>"#
            ),
            "mem://u.xsd",
        )
        .build()
        .unwrap();
    let ty = unqualified[unqualified.element(Some(NS), "a").unwrap()].type_id;
    let p = unqualified[ty]
        .as_complex()
        .unwrap()
        .content
        .particle()
        .unwrap();
    let inner = match unqualified[unqualified.child_particles(p)[0]].term {
        Term::Element(e) => e,
        _ => unreachable!(),
    };
    assert!(
        unqualified[inner].name.ns.is_none(),
        "local elements default to unqualified"
    );
}

// ---------------------------------------------------------------------------
// Occurrence — the primitive the future config generator needs
// ---------------------------------------------------------------------------

#[test]
fn occurrence_drives_repeatability_and_optionality() {
    let s = build(&schema(
        r#"<xs:element name="root">
             <xs:complexType><xs:sequence>
               <xs:element name="once"  type="xs:string"/>
               <xs:element name="many"  type="xs:string" maxOccurs="unbounded"/>
               <xs:element name="maybe" type="xs:string" minOccurs="0"/>
               <xs:element name="upto3" type="xs:string" maxOccurs="3"/>
             </xs:sequence></xs:complexType>
           </xs:element>"#,
    ));
    let ty = s[s.element(Some(NS), "root").unwrap()].type_id;
    let p = s[ty].as_complex().unwrap().content.particle().unwrap();
    let kids = s.child_particles(p);

    assert!(!s[kids[0]].is_repeating() && !s[kids[0]].is_optional());
    assert!(s[kids[1]].is_repeating());
    assert_eq!(s[kids[1]].max_occurs, MaxOccurs::Unbounded);
    assert!(s[kids[2]].is_optional() && !s[kids[2]].is_repeating());
    assert!(s[kids[3]].is_repeating());
    assert_eq!(s[kids[3]].max_occurs.as_u32(), Some(3));
}

#[test]
fn min_occurs_above_max_occurs_is_an_error() {
    let d = errors(&schema(
        r#"<xs:element name="root"><xs:complexType><xs:sequence>
             <xs:element name="x" type="xs:string" minOccurs="5" maxOccurs="2"/>
           </xs:sequence></xs:complexType></xs:element>"#,
    ));
    assert!(
        d.errors().any(|e| e.code == DiagCode::InvalidOccurrence),
        "{d}"
    );
}

// ---------------------------------------------------------------------------
// Attributes and attribute groups
// ---------------------------------------------------------------------------

#[test]
fn attribute_uses_carry_kind_and_value_constraint() {
    let s = build(&schema(
        r#"<xs:complexType name="Measure">
             <xs:simpleContent>
               <xs:extension base="xs:double">
                 <xs:attribute name="uom" type="xs:string" use="required" fixed="m"/>
                 <xs:attribute name="note" type="xs:string"/>
               </xs:extension>
             </xs:simpleContent>
           </xs:complexType>"#,
    ));
    let ty = s.type_(Some(NS), "Measure").unwrap();
    let uses = s.attribute_uses(ty);
    assert_eq!(uses.len(), 2);

    let uom = &uses[0];
    assert!(uom.is_required());
    let decl = &s[uom.attribute];
    assert_eq!(s.names().resolve(decl.name.local), "uom");
    // A schema-`fixed` unit is exactly the case the units layer can compile
    // into a constant scale/offset later.
    assert_eq!(decl.value_constraint.as_ref().unwrap().value(), "m");
    assert!(decl.value_constraint.as_ref().unwrap().is_fixed());

    assert_eq!(uses[1].kind, AttributeUseKind::Optional);
}

#[test]
fn attribute_groups_flatten_transitively() {
    let s = build(&schema(
        r#"<xs:attributeGroup name="Ident">
             <xs:attribute name="id" type="xs:string" use="required"/>
           </xs:attributeGroup>
           <xs:attributeGroup name="Common">
             <xs:attributeGroup ref="tns:Ident"/>
             <xs:attribute name="lang" type="xs:string"/>
           </xs:attributeGroup>
           <xs:complexType name="Doc">
             <xs:attributeGroup ref="tns:Common"/>
             <xs:attribute name="ver" type="xs:string"/>
           </xs:complexType>"#,
    ));
    let ty = s.type_(Some(NS), "Doc").unwrap();
    let mut names: Vec<_> = s
        .attribute_uses(ty)
        .iter()
        .map(|u| s.names().resolve(s[u.attribute].name.local).to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["id", "lang", "ver"],
        "nested groups must flatten through"
    );
}

#[test]
fn a_self_referencing_attribute_group_is_reported() {
    let d = errors(&schema(
        r#"<xs:attributeGroup name="Loop">
             <xs:attributeGroup ref="tns:Loop"/>
           </xs:attributeGroup>"#,
    ));
    assert!(
        d.errors().any(|e| e.code == DiagCode::CircularDefinition),
        "{d}"
    );
}

// ---------------------------------------------------------------------------
// Simple types
// ---------------------------------------------------------------------------

#[test]
fn facets_compose_across_restriction_steps() {
    let s = build(&schema(
        r#"<xs:simpleType name="Code">
             <xs:restriction base="xs:string">
               <xs:pattern value="[A-Z]+"/>
               <xs:pattern value="[0-9]+"/>
               <xs:maxLength value="4"/>
             </xs:restriction>
           </xs:simpleType>"#,
    ));
    let t = s.type_(Some(NS), "Code").unwrap();
    let st = s[t].as_simple().unwrap();
    assert_eq!(st.variety, Variety::Atomic);
    assert_eq!(st.facets.max_length, Some(4));
    // Two patterns at one step are alternatives, so they share a group.
    assert_eq!(
        st.facets.patterns,
        vec![vec!["[A-Z]+".to_string(), "[0-9]+".to_string()]]
    );
    assert_eq!(st.primitive, Some(Builtin::String));
}

#[test]
fn enumerations_and_whitespace_survive() {
    let s = build(&schema(
        r#"<xs:simpleType name="Status">
             <xs:restriction base="xs:token">
               <xs:enumeration value="open"/>
               <xs:enumeration value="closed"/>
             </xs:restriction>
           </xs:simpleType>"#,
    ));
    let st = s[s.type_(Some(NS), "Status").unwrap()].as_simple().unwrap();
    assert_eq!(
        st.facets.enumeration.as_ref().unwrap(),
        &vec!["open".to_string(), "closed".to_string()]
    );
    // xs:token fixes whiteSpace at collapse, which is what makes it a `trim`.
    assert_eq!(Builtin::Token.white_space(), WhiteSpace::Collapse);
}

#[test]
fn list_and_union_varieties_resolve_their_members() {
    let s = build(&schema(
        r#"<xs:simpleType name="Ints">
             <xs:list itemType="xs:int"/>
           </xs:simpleType>
           <xs:simpleType name="IntOrWord">
             <xs:union memberTypes="xs:int xs:string"/>
           </xs:simpleType>"#,
    ));
    let ints = s[s.type_(Some(NS), "Ints").unwrap()].as_simple().unwrap();
    assert_eq!(ints.variety, Variety::List);
    assert_eq!(ints.item_type, Some(s.builtin(Builtin::Int)));

    let u = s[s.type_(Some(NS), "IntOrWord").unwrap()]
        .as_simple()
        .unwrap();
    assert_eq!(u.variety, Variety::Union);
    // Order is load-bearing: members are tried in declaration order.
    assert_eq!(
        u.member_types,
        vec![s.builtin(Builtin::Int), s.builtin(Builtin::String)]
    );
}

#[test]
fn conflicting_varieties_are_reported() {
    let d = errors(&schema(
        r#"<xs:simpleType name="Bad">
             <xs:restriction base="xs:string"/>
             <xs:list itemType="xs:int"/>
           </xs:simpleType>"#,
    ));
    assert!(
        d.errors()
            .any(|e| e.code == DiagCode::ConflictingSimpleTypeVariety),
        "{d}"
    );
}

// ---------------------------------------------------------------------------
// Derivation and substitution groups
// ---------------------------------------------------------------------------

#[test]
fn complex_extension_records_its_base_and_method() {
    let s = build(&schema(
        r#"<xs:complexType name="Base">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="Derived">
             <xs:complexContent>
               <xs:extension base="tns:Base">
                 <xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence>
               </xs:extension>
             </xs:complexContent>
           </xs:complexType>"#,
    ));
    let base = s.type_(Some(NS), "Base").unwrap();
    let derived = s.type_(Some(NS), "Derived").unwrap();
    let c = s[derived].as_complex().unwrap();
    assert_eq!(c.base, base);
    assert_eq!(c.derivation, DerivationMethod::Extension);
    assert!(s.derives_from(derived, base));
    // Only the extension's own particles live here; the base's are reached
    // through the base chain, not copied.
    assert_eq!(child_elements(&s, derived), ["b"]);
}

#[test]
fn substitution_closure_is_transitive_and_skips_abstract_heads() {
    let s = build(&schema(
        r#"<xs:element name="feature" type="xs:string" abstract="true"/>
           <xs:element name="point"   type="xs:string" substitutionGroup="tns:feature"/>
           <xs:element name="curve"   type="xs:string" substitutionGroup="tns:feature"/>
           <xs:element name="arc"     type="xs:string" substitutionGroup="tns:curve"/>"#,
    ));
    let feature = s.element(Some(NS), "feature").unwrap();
    let mut members: Vec<_> = s
        .substitution_closure(feature)
        .into_iter()
        .map(|e| s.names().resolve(s[e].name.local).to_string())
        .collect();
    members.sort();
    // `arc` substitutes for `curve`, which substitutes for `feature`.
    assert_eq!(members, ["arc", "curve", "point"]);
    // The head is abstract, so it cannot itself appear in an instance.
    assert!(!members.contains(&"feature".to_string()));

    let curve = s.element(Some(NS), "curve").unwrap();
    let mut sub: Vec<_> = s
        .substitution_closure(curve)
        .into_iter()
        .map(|e| s.names().resolve(s[e].name.local).to_string())
        .collect();
    sub.sort();
    assert_eq!(sub, ["arc", "curve"], "a concrete head includes itself");
}

#[test]
fn circular_derivation_is_reported() {
    let d = errors(&schema(
        r#"<xs:complexType name="A">
             <xs:complexContent><xs:extension base="tns:B"/></xs:complexContent>
           </xs:complexType>
           <xs:complexType name="B">
             <xs:complexContent><xs:extension base="tns:A"/></xs:complexContent>
           </xs:complexType>"#,
    ));
    assert!(
        d.errors().any(|e| e.code == DiagCode::CircularDefinition),
        "{d}"
    );
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

#[test]
fn include_merges_into_the_same_namespace() {
    let part = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{NS}">
             <xs:complexType name="Shared">
               <xs:sequence><xs:element name="x" type="xs:string"/></xs:sequence>
             </xs:complexType>
           </xs:schema>"#
    );
    let main = schema(
        r#"<xs:include schemaLocation="part.xsd"/>
                         <xs:element name="root" type="tns:Shared"/>"#,
    );
    let s = SchemaSetBuilder::new()
        .resolver(MapResolver::default().with("part.xsd", &part))
        .text(&main, "mem://main.xsd")
        .build()
        .unwrap();
    let root = s.element(Some(NS), "root").unwrap();
    assert_eq!(s[root].type_id, s.type_(Some(NS), "Shared").unwrap());
}

/// A chameleon include has no `targetNamespace` of its own and is absorbed
/// into the includer's — so the same file yields different components per
/// includer, and the URI alone is not a cache key.
#[test]
fn chameleon_include_absorbs_the_includers_namespace() {
    let part = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
                    <xs:complexType name="Chameleon">
                      <xs:sequence><xs:element name="x" type="xs:string"/></xs:sequence>
                    </xs:complexType>
                  </xs:schema>"#;
    let main = schema(
        r#"<xs:include schemaLocation="part.xsd"/>
                         <xs:element name="root" type="tns:Chameleon"/>"#,
    );
    let s = SchemaSetBuilder::new()
        .resolver(MapResolver::default().with("part.xsd", part))
        .text(&main, "mem://main.xsd")
        .build()
        .unwrap();

    // It answers to the includer's namespace, not to no-namespace.
    assert!(s.type_(Some(NS), "Chameleon").is_some());
    assert!(s.type_(None, "Chameleon").is_none());
    assert!(s.documents().iter().any(|d| d.chameleon));
}

#[test]
fn import_brings_in_another_namespace() {
    let other = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                               targetNamespace="urn:other">
                     <xs:simpleType name="Id">
                       <xs:restriction base="xs:string"/>
                     </xs:simpleType>
                   </xs:schema>"#;
    let main = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:o="urn:other" targetNamespace="{NS}">
             <xs:import namespace="urn:other" schemaLocation="other.xsd"/>
             <xs:element name="root" type="o:Id"/>
           </xs:schema>"#
    );
    let s = SchemaSetBuilder::new()
        .resolver(MapResolver::default().with("other.xsd", other))
        .text(&main, "mem://main.xsd")
        .build()
        .unwrap();
    let root = s.element(Some(NS), "root").unwrap();
    assert_eq!(s[root].type_id, s.type_(Some("urn:other"), "Id").unwrap());
}

#[test]
fn circular_includes_terminate() {
    let a = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{NS}">
             <xs:include schemaLocation="b.xsd"/>
             <xs:element name="a" type="xs:string"/>
           </xs:schema>"#
    );
    let b = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{NS}">
             <xs:include schemaLocation="a.xsd"/>
             <xs:element name="b" type="xs:string"/>
           </xs:schema>"#
    );
    let s = SchemaSetBuilder::new()
        .resolver(MapResolver::default().with("a.xsd", &a).with("b.xsd", &b))
        .text(&a, "a.xsd")
        .build()
        .unwrap();
    assert!(s.element(Some(NS), "a").is_some());
    assert!(s.element(Some(NS), "b").is_some());
}

#[test]
fn a_missing_schema_location_is_an_error_but_lax_downgrades_it() {
    let main = schema(r#"<xs:include schemaLocation="missing.xsd"/>"#);
    let d = SchemaSetBuilder::new()
        .resolver(MapResolver::default())
        .text(&main, "mem://main.xsd")
        .build()
        .expect_err("strict mode must fail");
    assert!(
        d.errors()
            .any(|e| e.code == DiagCode::UnresolvedSchemaLocation),
        "{d}"
    );

    let (_, d) = SchemaSetBuilder::new()
        .resolver(MapResolver::default())
        .conformance(Conformance::Lax)
        .text(&main, "mem://main.xsd")
        .build_with_warnings();
    assert!(!d.has_errors(), "lax mode should warn, not fail:\n{d}");
    assert!(!d.is_empty(), "lax mode should still say something");
}

// `xs:redefine` and `xs:override` have their own suite now that they are
// implemented rather than warned about — see `tests/redefine.rs`.

// ---------------------------------------------------------------------------
// Identity constraints
// ---------------------------------------------------------------------------

#[test]
fn keyref_resolves_to_the_key_it_refers_to() {
    let s = build(&schema(
        r#"<xs:element name="root">
             <xs:complexType><xs:sequence>
               <xs:element name="station" maxOccurs="unbounded">
                 <xs:complexType><xs:attribute name="id" type="xs:string"/></xs:complexType>
               </xs:element>
               <xs:element name="reading" maxOccurs="unbounded">
                 <xs:complexType><xs:attribute name="sid" type="xs:string"/></xs:complexType>
               </xs:element>
             </xs:sequence></xs:complexType>
             <xs:key name="StationKey">
               <xs:selector xpath="station"/><xs:field xpath="@id"/>
             </xs:key>
             <xs:keyref name="ReadingRef" refer="tns:StationKey">
               <xs:selector xpath="reading"/><xs:field xpath="@sid"/>
             </xs:keyref>
           </xs:element>"#,
    ));
    let root = s.element(Some(NS), "root").unwrap();
    let idcs = &s[root].identity_constraints;
    assert_eq!(idcs.len(), 2);

    let key = idcs.iter().find(|i| s[**i].kind == IdcKind::Key).unwrap();
    let keyref = idcs
        .iter()
        .find(|i| s[**i].kind == IdcKind::KeyRef)
        .unwrap();
    assert_eq!(s[*key].selector, "station");
    assert_eq!(s[*key].fields, vec!["@id".to_string()]);
    // A keyref is a declared foreign key — free relational structure.
    assert_eq!(s[*keyref].refer, Some(*key));
}

// ---------------------------------------------------------------------------
// Annotations — the seam the units layer will hang on
// ---------------------------------------------------------------------------

#[test]
fn appinfo_is_kept_verbatim() {
    let s = build(&schema(
        r#"<xs:element name="pressure" type="xs:double">
             <xs:annotation>
               <xs:documentation>Ambient pressure.</xs:documentation>
               <xs:appinfo source="urn:units"><u:unit xmlns:u="urn:u">hPa</u:unit></xs:appinfo>
             </xs:annotation>
           </xs:element>"#,
    ));
    let e = s.element(Some(NS), "pressure").unwrap();
    let ann = s
        .get_annotation(s[e].annotation.expect("annotation"))
        .unwrap();
    assert_eq!(ann.doc(), "Ambient pressure.");
    assert_eq!(ann.appinfo.len(), 1);
    assert_eq!(ann.appinfo[0].source.as_deref(), Some("urn:units"));
    // The raw XML has to survive: a unit in appinfo cannot be recovered from
    // a summary.
    assert!(ann.appinfo[0].xml.contains("hPa"), "{}", ann.appinfo[0].xml);
    assert!(
        ann.appinfo[0].xml.contains("{urn:u}unit"),
        "{}",
        ann.appinfo[0].xml
    );
}

// ---------------------------------------------------------------------------
// Wildcards
// ---------------------------------------------------------------------------

#[test]
fn wildcards_record_their_namespace_constraint() {
    let s = build(&schema(
        r###"<xs:complexType name="Open">
             <xs:sequence>
               <xs:any namespace="##other" processContents="lax"/>
             </xs:sequence>
             <xs:anyAttribute namespace="##any"/>
           </xs:complexType>"###,
    ));
    let ty = s.type_(Some(NS), "Open").unwrap();
    let c = s[ty].as_complex().unwrap();
    assert!(matches!(
        c.attribute_wildcard.as_ref().unwrap().namespace,
        NamespaceConstraint::Any
    ));

    let p = c.content.particle().unwrap();
    let kid = s.child_particles(p)[0];
    match &s[kid].term {
        // ##other excludes the target namespace, and admits everything else.
        Term::Wildcard(w) => {
            let tns = s.documents().iter().find_map(|d| d.target_namespace);
            assert!(tns.is_some());
            assert!(
                !w.namespace.admits(tns),
                "##other must exclude the target namespace"
            );
            assert!(
                w.namespace.admits(None),
                "##other admits no-namespace names"
            );
        }
        other => panic!("expected a wildcard, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Failure modes
// ---------------------------------------------------------------------------

#[test]
fn unresolved_references_are_reported_with_a_span() {
    let d = errors(&schema(r#"<xs:element name="root" type="tns:Missing"/>"#));
    let e = d
        .errors()
        .find(|e| e.code == DiagCode::UnresolvedReference)
        .unwrap_or_else(|| panic!("{d}"));
    assert!(e.message.contains("Missing"), "{}", e.message);
    assert!(!e.spans.is_empty(), "a diagnostic must point somewhere");
    assert!(e.help.is_some());
}

#[test]
fn every_error_is_reported_not_just_the_first() {
    let d = errors(&schema(
        r#"<xs:element name="a" type="tns:NopeOne"/>
           <xs:element name="b" type="tns:NopeTwo"/>
           <xs:element name="c" type="tns:NopeThree"/>"#,
    ));
    assert_eq!(
        d.errors()
            .filter(|e| e.code == DiagCode::UnresolvedReference)
            .count(),
        3,
        "schema authors need the whole list:\n{d}"
    );
}

#[test]
fn duplicate_globals_collide_only_within_one_symbol_space() {
    // Same name, different symbol spaces — legal.
    let s = build(&schema(
        r#"<xs:element name="Foo" type="xs:string"/>
           <xs:complexType name="Foo"><xs:sequence/></xs:complexType>"#,
    ));
    assert!(s.element(Some(NS), "Foo").is_some());
    assert!(s.type_(Some(NS), "Foo").is_some());

    // Same name, same symbol space — an error.
    let d = errors(&schema(
        r#"<xs:element name="Dup" type="xs:string"/>
           <xs:element name="Dup" type="xs:int"/>"#,
    ));
    assert!(
        d.errors().any(|e| e.code == DiagCode::DuplicateGlobal),
        "{d}"
    );
}

#[test]
fn malformed_xml_is_a_diagnostic_not_a_panic() {
    let d = SchemaSetBuilder::new()
        .text("<xs:schema><unclosed>", "mem://bad.xsd")
        .build()
        .expect_err("malformed input must not build");
    assert!(d.errors().any(|e| e.code == DiagCode::MalformedXml), "{d}");
}

#[test]
fn a_non_schema_root_is_rejected() {
    let d = SchemaSetBuilder::new()
        .text("<html><body/></html>", "mem://page.html")
        .build()
        .expect_err("a non-schema document must not build");
    assert!(
        d.errors().any(|e| e.code == DiagCode::NotASchemaDocument),
        "{d}"
    );
}

#[test]
fn an_undeclared_prefix_is_reported() {
    let d = errors(&schema(r#"<xs:element name="root" type="nope:Thing"/>"#));
    assert!(
        d.errors()
            .any(|e| e.code == DiagCode::InvalidAttributeValue),
        "{d}"
    );
}

#[test]
fn the_default_resolver_refuses_the_network() {
    let d = SchemaSetBuilder::new()
        .text(
            schema(r#"<xs:include schemaLocation="https://example.com/a.xsd"/>"#),
            "mem://main.xsd",
        )
        .build()
        .expect_err("network fetches are opt-in");
    let e = d
        .errors()
        .find(|e| e.code == DiagCode::UnresolvedSchemaLocation)
        .unwrap();
    assert!(e.message.contains("network"), "{}", e.message);
}

// ---------------------------------------------------------------------------
// Document encodings
// ---------------------------------------------------------------------------

fn latin1(s: &str) -> Vec<u8> {
    encoding_rs::WINDOWS_1252.encode(s).0.into_owned()
}

fn utf16le(s: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for u in s.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// The original repro: a schema that is legal, complete, and not UTF-8.
#[test]
fn a_latin1_schema_loads() {
    let xsd = format!(
        r#"<?xml version="1.0" encoding="ISO-8859-1"?>
           <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{NS}">
             <xs:element name="messgroesse" type="xs:double">
               <xs:annotation><xs:documentation>Größe in Metern</xs:documentation></xs:annotation>
             </xs:element>
           </xs:schema>"#
    );
    let s = SchemaSetBuilder::new()
        .bytes(latin1(&xsd), "mem://latin1.xsd")
        .build()
        .unwrap_or_else(|d| panic!("a Latin-1 schema must load:\n{d}"));

    let e = s.element(Some(NS), "messgroesse").expect("element");
    let ann = s
        .get_annotation(s[e].annotation.expect("annotation"))
        .unwrap();
    assert_eq!(
        ann.doc(),
        "Größe in Metern",
        "non-ASCII text must survive decoding"
    );
}

#[test]
fn a_utf16_schema_loads() {
    let xsd = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
           <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{NS}">
             <xs:element name="temperatur" type="xs:double"/>
           </xs:schema>"#
    );
    let s = SchemaSetBuilder::new()
        .bytes(utf16le(&xsd), "mem://utf16.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"));
    assert!(s.element(Some(NS), "temperatur").is_some());
}

#[test]
fn a_utf8_bom_does_not_break_the_parse() {
    let mut b = vec![0xEF, 0xBB, 0xBF];
    b.extend_from_slice(schema(r#"<xs:element name="a" type="xs:string"/>"#).as_bytes());
    let s = SchemaSetBuilder::new()
        .bytes(b, "mem://bom.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"));
    assert!(s.element(Some(NS), "a").is_some());
}

/// The failure this fix exists for: an encoding problem used to surface as a
/// missing file, with help about search paths.
#[test]
fn an_encoding_failure_blames_the_encoding() {
    let xsd = format!(
        r#"<?xml version="1.0" encoding="Klingon-1"?>
           <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{NS}"/>"#
    );
    let d = SchemaSetBuilder::new()
        .bytes(xsd.into_bytes(), "mem://bad.xsd")
        .build()
        .expect_err("an unknown encoding must not build");
    assert!(
        d.errors().any(|e| e.code == DiagCode::UnsupportedEncoding),
        "{d}"
    );
    assert!(
        !d.errors()
            .any(|e| e.code == DiagCode::UnresolvedSchemaLocation),
        "the file was found and read; do not blame its location:\n{d}"
    );
}

#[test]
fn bytes_that_contradict_their_declaration_are_reported() {
    let mut b = br#"<?xml version="1.0" encoding="UTF-8"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><!-- "#.to_vec();
    b.push(0xE9); // valid Latin-1, invalid UTF-8
    b.extend_from_slice(b" --></xs:schema>");
    let d = SchemaSetBuilder::new()
        .bytes(b, "mem://mismatch.xsd")
        .build()
        .expect_err("mismatched bytes must not build");
    assert!(
        d.errors().any(|e| e.code == DiagCode::MalformedEncoding),
        "{d}"
    );
}

/// Encoding detection has to work through composition too, not just at the
/// entry point — an included document has its own declaration.
#[test]
fn an_included_document_is_decoded_on_its_own_terms() {
    struct Bytes(FxHashMap<String, Vec<u8>>);
    impl Resolver for Bytes {
        fn resolve(
            &self,
            location: &str,
            _base: Option<&str>,
        ) -> Result<(String, Vec<u8>), String> {
            self.0
                .get(location)
                .map(|b| (location.to_string(), b.clone()))
                .ok_or_else(|| format!("not in map: {location}"))
        }
    }

    let part = format!(
        r#"<?xml version="1.0" encoding="ISO-8859-1"?>
           <xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="{NS}">
             <xs:simpleType name="Größe"><xs:restriction base="xs:string"/></xs:simpleType>
           </xs:schema>"#
    );
    let mut map = FxHashMap::default();
    map.insert("part.xsd".to_string(), latin1(&part));

    let main = schema(r#"<xs:include schemaLocation="part.xsd"/>"#);
    let s = SchemaSetBuilder::new()
        .resolver(Bytes(map))
        .text(main, "mem://main.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"));

    assert!(
        s.type_(Some(NS), "Größe").is_some(),
        "the included document's own encoding declaration must be honoured"
    );
}

// ---------------------------------------------------------------------------
// Inherited attribute uses
// ---------------------------------------------------------------------------

/// Both derivation methods inherit attributes — unlike content models, where
/// extension appends and restriction replaces.
///
/// Regression: a vacuous extension reported no attributes at all. That is not
/// a corner case; GML's whole measure family is vacuous extensions of
/// `gml:MeasureType`, so every one of its measure types lost its `uom`.
#[test]
fn attribute_uses_are_inherited_through_derivation() {
    let s = build(&schema(
        r#"<xs:complexType name="MeasureType">
             <xs:simpleContent><xs:extension base="xs:double">
               <xs:attribute name="uom" type="xs:string" use="required"/>
             </xs:extension></xs:simpleContent>
           </xs:complexType>
           <xs:complexType name="LengthType">
             <xs:simpleContent><xs:extension base="tns:MeasureType"/></xs:simpleContent>
           </xs:complexType>
           <xs:complexType name="PressureType">
             <xs:simpleContent><xs:extension base="tns:MeasureType">
               <xs:attribute name="datum" type="xs:string"/>
             </xs:extension></xs:simpleContent>
           </xs:complexType>"#,
    ));
    let names = |n: &str| -> Vec<String> {
        s.attribute_uses(s.type_(Some(NS), n).unwrap())
            .iter()
            .map(|u| s.names().resolve(s[u.attribute].name.local).to_string())
            .collect()
    };
    assert_eq!(names("MeasureType"), ["uom"]);
    assert_eq!(
        names("LengthType"),
        ["uom"],
        "a vacuous extension still has uom"
    );
    assert_eq!(
        names("PressureType"),
        ["uom", "datum"],
        "base first, then own"
    );
}

/// A restriction's own use replaces the inherited one, which is how a schema
/// narrows an attribute — or pins it to a constant.
#[test]
fn a_restriction_narrows_an_inherited_attribute() {
    let s = build(&schema(
        r#"<xs:complexType name="MeasureType">
             <xs:simpleContent><xs:extension base="xs:double">
               <xs:attribute name="uom" type="xs:string" use="required"/>
             </xs:extension></xs:simpleContent>
           </xs:complexType>
           <xs:complexType name="Metres">
             <xs:simpleContent><xs:restriction base="tns:MeasureType">
               <xs:attribute name="uom" type="xs:string" fixed="m"/>
             </xs:restriction></xs:simpleContent>
           </xs:complexType>
           <xs:complexType name="NoUnit">
             <xs:simpleContent><xs:restriction base="tns:MeasureType">
               <xs:attribute name="uom" use="prohibited"/>
             </xs:restriction></xs:simpleContent>
           </xs:complexType>"#,
    ));
    let uses = s.attribute_uses(s.type_(Some(NS), "Metres").unwrap());
    assert_eq!(uses.len(), 1, "not duplicated with the inherited one");
    // A schema-declared constant unit: known without seeing any document.
    assert_eq!(
        s[uses[0].attribute]
            .value_constraint
            .as_ref()
            .map(|v| v.value()),
        Some("m")
    );

    let prohibited = s.attribute_uses(s.type_(Some(NS), "NoUnit").unwrap());
    assert_eq!(prohibited.len(), 1);
    assert_eq!(prohibited[0].kind, AttributeUseKind::Prohibited);
}

/// Inheritance runs through a chain, not just one step.
#[test]
fn inherited_attributes_accumulate_down_a_chain() {
    let s = build(&schema(
        r#"<xs:complexType name="A"><xs:attribute name="a" type="xs:string"/></xs:complexType>
           <xs:complexType name="B"><xs:complexContent><xs:extension base="tns:A">
             <xs:attribute name="b" type="xs:string"/>
           </xs:extension></xs:complexContent></xs:complexType>
           <xs:complexType name="C"><xs:complexContent><xs:extension base="tns:B">
             <xs:attribute name="c" type="xs:string"/>
           </xs:extension></xs:complexContent></xs:complexType>"#,
    ));
    let names: Vec<_> = s
        .attribute_uses(s.type_(Some(NS), "C").unwrap())
        .iter()
        .map(|u| s.names().resolve(s[u.attribute].name.local).to_string())
        .collect();
    assert_eq!(names, ["a", "b", "c"]);
}

// ---------------------------------------------------------------------------
// Schema version
// ---------------------------------------------------------------------------

/// `xs:schema/@version` is a bare `xs:token` with no processing role, so it is
/// reported verbatim rather than parsed.
#[test]
fn the_schema_version_attribute_is_reported_verbatim() {
    let s = SchemaSetBuilder::new()
        .text(
            format!(
                r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                              targetNamespace="{NS}" version="3.2.2">
                     <xs:element name="a" type="xs:string"/>
                   </xs:schema>"#
            ),
            "mem://main.xsd",
        )
        .build()
        .unwrap();
    assert_eq!(s.documents()[0].version.as_deref(), Some("3.2.2"));
}

#[test]
fn an_absent_version_is_none_not_empty() {
    let s = build(&schema(r#"<xs:element name="a" type="xs:string"/>"#));
    assert_eq!(s.documents()[0].version, None);
}

/// The version identifying a *vocabulary* usually lives in the target
/// namespace; `@version` carries the patch level underneath it. Both are
/// reachable, and they routinely disagree — GML's namespace says 3.2 while its
/// documents say 3.2.2.
#[test]
fn the_namespace_and_the_version_attribute_are_separate_facts() {
    let s = SchemaSetBuilder::new()
        .text(
            r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                          targetNamespace="http://example.com/gml/3.2" version="3.2.2">
                 <xs:element name="a" type="xs:string"/>
               </xs:schema>"#,
            "mem://main.xsd",
        )
        .build()
        .unwrap();
    let d = &s.documents()[0];
    let ns = s.names().resolve_ns(d.target_namespace.unwrap());
    assert!(
        ns.ends_with("/3.2"),
        "the namespace carries the minor version"
    );
    assert_eq!(
        d.version.as_deref(),
        Some("3.2.2"),
        "the attribute carries the patch"
    );
}

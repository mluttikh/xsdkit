//! The navigable view over a compiled schema.
//!
//! These are the questions the id API could already answer; what is being
//! tested is that following the schema reads like following a schema, and
//! that the two layers cannot drift apart.

use xsdkit::*;

const NS: &str = "urn:example";

fn build(body: &str) -> Schemas {
    let xsd = format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    );
    SchemaSetBuilder::new()
        .text(xsd, "mem://main.xsd")
        .build()
        .unwrap_or_else(|d| panic!("{d}"))
}

/// A report with a bit of everything to walk over.
fn report() -> Schemas {
    build(
        r#"<xs:simpleType name="Unit">
             <xs:restriction base="xs:string">
               <xs:enumeration value="m"/><xs:enumeration value="ft"/>
             </xs:restriction>
           </xs:simpleType>
           <xs:simpleType name="Units"><xs:list itemType="tns:Unit"/></xs:simpleType>
           <xs:simpleType name="Depth">
             <xs:union memberTypes="xs:double tns:Unit"/>
           </xs:simpleType>
           <xs:element name="shape" type="xs:string" abstract="true"/>
           <xs:element name="circle" type="xs:string" substitutionGroup="tns:shape"/>
           <xs:element name="square" type="xs:string" substitutionGroup="tns:shape"/>
           <xs:complexType name="Report">
             <xs:sequence>
               <xs:element name="title" type="xs:string"/>
               <xs:element name="depth" type="tns:Depth" minOccurs="0"/>
               <xs:element name="units" type="tns:Units" minOccurs="0"/>
               <xs:element ref="tns:shape" minOccurs="0" maxOccurs="unbounded"/>
             </xs:sequence>
             <xs:attribute name="api" type="xs:string" use="required"/>
             <xs:attribute name="uom" type="tns:Unit" default="m"/>
           </xs:complexType>
           <xs:element name="report" type="tns:Report"/>"#,
    )
}

/// A reference is a borrow and an id, which is what makes handing them out
/// freely reasonable. If this grows, following a schema starts costing.
#[test]
fn a_reference_is_two_words() {
    assert_eq!(
        size_of::<ElementRef<'_>>(),
        2 * size_of::<usize>(),
        "a reference should be a pointer and an id"
    );
    assert_eq!(size_of::<TypeRef<'_>>(), 2 * size_of::<usize>());
    // And `Copy`, so passing one around never moves it.
    fn assert_copy<T: Copy>() {}
    assert_copy::<ElementRef<'_>>();
    assert_copy::<TypeRef<'_>>();
    assert_copy::<ChildRef<'_>>();
}

#[test]
fn a_name_lookup_gives_a_reference_and_the_id_beside_it() {
    let s = report();
    let e = s.element(Some(NS), "report").expect("report");
    assert_eq!(Some(e.id()), s.element_id(Some(NS), "report"));
    assert_eq!(s.get(e.id()), e, "get and the lookup agree");
    assert_eq!(e.local_name(), "report");
    assert_eq!(e.namespace(), Some(NS));
    assert_eq!(e.display_name(), format!("{{{NS}}}report"));
    assert!(e.is_global() && !e.is_abstract() && !e.is_nillable());
    assert!(s.element(Some(NS), "nope").is_none());
    // A namespace the schema never interned cannot name anything.
    assert!(s.element(Some("urn:unheard-of"), "report").is_none());
}

#[test]
fn following_a_schema_needs_no_ids() {
    let s = report();
    let report = s.element(Some(NS), "report").unwrap();

    let mut kids: Vec<_> = report
        .children()
        .map(|c| (c.local_name(), c.repeats(), c.optional()))
        .collect();
    kids.sort();
    assert_eq!(
        kids,
        [
            ("circle", true, true),
            ("depth", false, true),
            ("square", true, true),
            ("title", false, false),
            ("units", false, true),
        ]
    );

    // Down a level and back to a type by name.
    let depth = report.child("depth").expect("a depth child");
    assert_eq!(depth.type_of().local_name(), Some("Depth"));
    assert_eq!(depth.type_of().variety(), Some(datatypes::Variety::Union));
    assert!(report.child("nope").is_none());
}

#[test]
fn attributes_carry_the_use_not_just_the_declaration() {
    let s = report();
    let report = s.element(Some(NS), "report").unwrap();
    let mut attrs: Vec<_> = report
        .attributes()
        .map(|a| (a.local_name(), a.is_required(), a.default()))
        .collect();
    attrs.sort();
    assert_eq!(attrs, [("api", true, None), ("uom", false, Some("m"))]);
    assert_eq!(
        report
            .type_of()
            .attribute("uom")
            .unwrap()
            .type_of()
            .local_name(),
        Some("Unit")
    );
}

#[test]
fn simple_types_report_how_they_are_built() {
    let s = report();
    let unit = s.type_(Some(NS), "Unit").unwrap();
    assert_eq!(unit.variety(), Some(datatypes::Variety::Atomic));
    assert_eq!(
        unit.facets().and_then(|f| f.enumeration.clone()),
        Some(vec!["m".to_string(), "ft".to_string()])
    );

    let units = s.type_(Some(NS), "Units").unwrap();
    assert_eq!(units.variety(), Some(datatypes::Variety::List));
    assert_eq!(units.item_type().unwrap().local_name(), Some("Unit"));

    let depth = s.type_(Some(NS), "Depth").unwrap();
    let members: Vec<_> = depth.member_types().map(|m| m.local_name()).collect();
    assert_eq!(members, [Some("double"), Some("Unit")]);

    // A complex type answers none of those.
    let report = s.type_(Some(NS), "Report").unwrap();
    assert!(report.is_complex() && report.variety().is_none());
}

/// `xs:anyType` is its own base, which is a fixed point rather than a parent
/// worth returning — otherwise walking up the chain never ends.
#[test]
fn the_base_chain_terminates() {
    let s = report();
    let mut t = s.type_(Some(NS), "Unit").unwrap();
    let mut names = vec![t.display_name()];
    while let Some(base) = t.base() {
        names.push(base.display_name());
        t = base;
        assert!(names.len() < 10, "base chain did not terminate: {names:?}");
    }
    assert_eq!(names.first().unwrap(), &format!("{{{NS}}}Unit"));
    assert!(
        names.last().unwrap().ends_with("}anyType"),
        "the chain should reach anyType, got {names:?}"
    );
}

#[test]
fn substitutes_gives_the_closure_without_the_abstract_head() {
    let s = report();
    let shape = s.element(Some(NS), "shape").unwrap();
    assert!(shape.is_abstract());
    let mut members: Vec<_> = shape.substitutes().map(|e| e.local_name()).collect();
    members.sort();
    assert_eq!(members, ["circle", "square"]);
}

/// An anonymous type has no name to report, and must not be made to invent
/// one.
#[test]
fn an_inline_type_has_no_name() {
    let s = build(
        r#"<xs:element name="e">
             <xs:complexType><xs:sequence>
               <xs:element name="x" type="xs:string"/>
             </xs:sequence></xs:complexType>
           </xs:element>"#,
    );
    let t = s.element(Some(NS), "e").unwrap().type_of();
    assert_eq!(t.name(), None);
    assert_eq!(t.local_name(), None);
    assert_eq!(t.display_name(), "(anonymous)");
    assert_eq!(t.child("x").unwrap().local_name(), "x");
}

/// An id means nothing without the schema it indexes, so equality has to
/// take the schema into account.
#[test]
fn references_from_different_schemas_are_never_equal() {
    let a = report();
    let b = report();
    let ea = a.element(Some(NS), "report").unwrap();
    let eb = b.element(Some(NS), "report").unwrap();
    assert_eq!(ea.id(), eb.id(), "the same schema text gives the same ids");
    assert_ne!(ea, eb, "but they are declarations in different schemas");
    assert_eq!(ea, a.element(Some(NS), "report").unwrap());
}

/// The reference layer must stay a view of the id layer, not a second
/// implementation of it.
#[test]
fn the_two_layers_agree_everywhere() {
    let s = report();
    let mut checked = 0;
    for (id, decl) in s.iter_elements() {
        let e = s.get(id);
        assert_eq!(e.name(), decl.name);
        assert_eq!(e.type_of().id(), decl.type_id);
        assert_eq!(e.local_name(), s.names().resolve(decl.name.local));
        assert_eq!(e.display_name(), s.display_name(decl.name));
        assert_eq!(
            e.substitutes().map(|x| x.id()).collect::<Vec<_>>(),
            s.substitution_closure(id)
        );
        checked += 1;
    }
    assert!(checked > 4, "only {checked} elements were compared");

    for (id, def) in s.iter_types() {
        let t = s.get(id);
        assert_eq!(t.name(), def.name());
        assert_eq!(t.is_simple(), def.is_simple());
        assert_eq!(
            t.children().map(|c| c.id()).collect::<Vec<_>>(),
            s.possible_children(id)
        );
        for c in t.children() {
            assert_eq!(c.repeats(), s.child_repeats(id, c.id()));
            assert_eq!(c.optional(), s.child_is_optional(id, c.id()));
        }
    }
}

/// `Debug` on a reference must print the component, not the whole schema
/// behind it — a schema formats to megabytes.
#[test]
fn debug_prints_the_component() {
    let s = report();
    let e = s.element(Some(NS), "report").unwrap();
    assert_eq!(format!("{e:?}"), format!("ElementRef({{{NS}}}report)"));
    let shape = e.child("shape");
    assert!(shape.is_none(), "the abstract head does not appear itself");
    let circle = e.child("circle").unwrap();
    assert_eq!(
        format!("{circle:?}"),
        format!("ChildRef({{{NS}}}circle+?)"),
        "repeating and optional show as + and ?"
    );
    assert_eq!(
        format!("{:?}", e.type_of()),
        format!("TypeRef({{{NS}}}Report)")
    );
}

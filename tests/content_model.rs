//! Content-model compilation: automata, matching, UPA, and the queries the
//! config generator will be built on.

use xsdkit::*;

const NS: &str = "urn:example";

fn schema(body: &str) -> String {
    format!(
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"
                      xmlns:tns="{NS}" targetNamespace="{NS}"
                      elementFormDefault="qualified">{body}</xs:schema>"#
    )
}

fn build(body: &str) -> Schemas {
    SchemaSetBuilder::new()
        .text(schema(body), "mem://main.xsd")
        .build()
        .unwrap_or_else(|d| panic!("expected a clean build, got:\n{d}"))
}

fn build_lax(body: &str) -> (Schemas, Diagnostics) {
    SchemaSetBuilder::new()
        .text(schema(body), "mem://main.xsd")
        .conformance(Conformance::Lax)
        .build_with_warnings()
}

fn diagnostics(body: &str) -> Diagnostics {
    SchemaSetBuilder::new()
        .text(schema(body), "mem://main.xsd")
        .build_with_warnings()
        .1
}

/// A complex type wrapping `content`, plus enough element declarations to
/// reference by name.
fn ty(content: &str) -> String {
    format!(r#"<xs:complexType name="T">{content}</xs:complexType>"#)
}

fn type_t(s: &Schemas) -> TypeId {
    s.type_(Some(NS), "T").expect("type T")
}

/// Runs a space-separated sequence of local names through the matcher.
fn accepts(s: &Schemas, t: TypeId, seq: &str) -> bool {
    let mut m = s
        .match_content(t)
        .expect("a complex type has a content model");
    for name in seq.split_whitespace() {
        let Some(q) = s.qname(Some(NS), name) else {
            return false;
        };
        if !m.step(q) {
            return false;
        }
    }
    m.accepts_end()
}

fn upa_count(d: &Diagnostics) -> usize {
    d.iter()
        .filter(|x| x.code == DiagCode::AmbiguousContentModel)
        .count()
}

// ---------------------------------------------------------------------------
// Construction and matching
// ---------------------------------------------------------------------------

#[test]
fn a_sequence_matches_in_order() {
    let s = build(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string"/>
        <xs:element name="b" type="xs:string"/>
    </xs:sequence>"#));
    let t = type_t(&s);
    assert!(accepts(&s, t, "a b"));
    assert!(!accepts(&s, t, ""));
    assert!(!accepts(&s, t, "a"));
    assert!(
        !accepts(&s, t, "b a"),
        "order is load-bearing in a sequence"
    );
    assert!(!accepts(&s, t, "a b a"));
}

#[test]
fn a_choice_matches_exactly_one_branch() {
    let s = build(&ty(r#"<xs:choice>
        <xs:element name="a" type="xs:string"/>
        <xs:element name="b" type="xs:string"/>
    </xs:choice>"#));
    let t = type_t(&s);
    assert!(accepts(&s, t, "a"));
    assert!(accepts(&s, t, "b"));
    assert!(!accepts(&s, t, "a b"));
    assert!(!accepts(&s, t, ""));
}

#[test]
fn optional_particles_may_be_skipped() {
    let s = build(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="0"/>
        <xs:element name="b" type="xs:string"/>
    </xs:sequence>"#));
    let t = type_t(&s);
    assert!(accepts(&s, t, "b"));
    assert!(accepts(&s, t, "a b"));
    assert!(!accepts(&s, t, "a"));
}

#[test]
fn unbounded_repetition_loops() {
    let s = build(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" maxOccurs="unbounded"/>
    </xs:sequence>"#));
    let t = type_t(&s);
    assert!(!accepts(&s, t, ""), "minOccurs defaults to 1");
    assert!(accepts(&s, t, "a"));
    assert!(accepts(&s, t, "a a a a a a a a"));
}

#[test]
fn zero_or_more_accepts_empty() {
    let s = build(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="0" maxOccurs="unbounded"/>
    </xs:sequence>"#));
    let t = type_t(&s);
    assert!(accepts(&s, t, ""));
    assert!(accepts(&s, t, "a a"));
}

/// Numeric ranges are unrolled, so the bounds are enforced exactly rather
/// than widened to `+`.
#[test]
fn numeric_ranges_enforce_both_bounds() {
    let s = build(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="2" maxOccurs="3"/>
    </xs:sequence>"#));
    let t = type_t(&s);
    assert!(!accepts(&s, t, "a"), "below minOccurs");
    assert!(accepts(&s, t, "a a"));
    assert!(accepts(&s, t, "a a a"));
    assert!(!accepts(&s, t, "a a a a"), "above maxOccurs");

    let model = s.content_model(t).unwrap();
    match model {
        ContentModel::Automaton(a) => {
            assert_eq!(a.positions().len(), 3, "one position per unrolled copy");
            assert!(!a.approximated());
        }
        other => panic!("expected an automaton, got {other:?}"),
    }
}

#[test]
fn nested_groups_compose() {
    let s = build(&ty(r#"<xs:choice>
        <xs:sequence>
            <xs:element name="a" type="xs:string"/>
            <xs:element name="b" type="xs:string"/>
        </xs:sequence>
        <xs:element name="c" type="xs:string"/>
    </xs:choice>"#));
    let t = type_t(&s);
    assert!(accepts(&s, t, "a b"));
    assert!(accepts(&s, t, "c"));
    assert!(!accepts(&s, t, "a"));
    assert!(!accepts(&s, t, "a c"));
}

#[test]
fn named_group_references_are_inlined() {
    let s = build(&format!(
        r#"<xs:group name="G">
             <xs:sequence>
               <xs:element name="a" type="xs:string"/>
               <xs:element name="b" type="xs:string"/>
             </xs:sequence>
           </xs:group>
           {}"#,
        ty(r#"<xs:sequence>
                <xs:group ref="tns:G" maxOccurs="unbounded"/>
              </xs:sequence>"#)
    ));
    let t = type_t(&s);
    assert!(accepts(&s, t, "a b"));
    assert!(accepts(&s, t, "a b a b a b"));
    assert!(!accepts(&s, t, "a b a"));
}

#[test]
fn xs_all_accepts_any_order_but_not_repeats() {
    let s = build(&ty(r#"<xs:all>
        <xs:element name="a" type="xs:string"/>
        <xs:element name="b" type="xs:string"/>
        <xs:element name="c" type="xs:string" minOccurs="0"/>
    </xs:all>"#));
    let t = type_t(&s);
    assert!(matches!(s.content_model(t), Some(ContentModel::All(_))));

    assert!(accepts(&s, t, "a b"));
    assert!(accepts(&s, t, "b a"));
    assert!(accepts(&s, t, "c b a"));
    assert!(!accepts(&s, t, "a"), "b is required");
    assert!(!accepts(&s, t, "a b a"), "each member matches at most once");
}

#[test]
fn wildcards_match_by_namespace() {
    let s = build(&ty(r###"<xs:sequence>
        <xs:element name="a" type="xs:string"/>
        <xs:any namespace="##other" processContents="lax"/>
    </xs:sequence>"###));
    let t = type_t(&s);
    let mut m = s.match_content(t).unwrap();
    assert!(m.step(s.qname(Some(NS), "a").unwrap()));
    // ##other excludes the target namespace, so a same-namespace name here
    // must not satisfy the wildcard.
    assert!(!m.step(s.qname(Some(NS), "a").unwrap()));
}

#[test]
fn an_empty_content_model_admits_nothing() {
    let s = build(&ty(r#"<xs:attribute name="x" type="xs:string"/>"#));
    let t = type_t(&s);
    assert!(matches!(s.content_model(t), Some(ContentModel::Empty)));
    assert!(accepts(&s, t, ""));
    assert!(!accepts(&s, t, "a"));
}

// ---------------------------------------------------------------------------
// Unique Particle Attribution
// ---------------------------------------------------------------------------

/// The textbook breach: after an optional `a`, another `a` could be either
/// particle.
#[test]
fn an_optional_particle_followed_by_the_same_name_is_ambiguous() {
    let d = diagnostics(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="0"/>
        <xs:element name="a" type="xs:string"/>
    </xs:sequence>"#));
    assert_eq!(upa_count(&d), 1, "{d}");
    let e = d
        .iter()
        .find(|x| x.code == DiagCode::AmbiguousContentModel)
        .unwrap();
    assert_eq!(e.severity, Severity::Error);
    assert_eq!(
        e.spans.len(),
        2,
        "both competing particles must be pointed at"
    );
}

#[test]
fn an_element_competing_with_a_wildcard_is_ambiguous_in_xsd_10() {
    let d = diagnostics(&ty(r###"<xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="0"/>
        <xs:any namespace="##any"/>
    </xs:sequence>"###));
    assert_eq!(upa_count(&d), 1, "{d}");
    let e = d
        .iter()
        .find(|x| x.code == DiagCode::AmbiguousContentModel)
        .unwrap();
    assert!(
        e.help.as_ref().unwrap().contains("1.1"),
        "the 1.1 rule is worth naming"
    );
}

#[test]
fn unambiguous_models_are_silent() {
    for content in [
        r#"<xs:sequence>
             <xs:element name="a" type="xs:string"/>
             <xs:element name="b" type="xs:string"/>
           </xs:sequence>"#,
        r#"<xs:choice>
             <xs:element name="a" type="xs:string"/>
             <xs:element name="b" type="xs:string"/>
           </xs:choice>"#,
        r#"<xs:sequence>
             <xs:element name="a" type="xs:string" maxOccurs="unbounded"/>
             <xs:element name="b" type="xs:string"/>
           </xs:sequence>"#,
    ] {
        let d = diagnostics(&ty(content));
        assert_eq!(upa_count(&d), 0, "should be unambiguous:\n{content}\n{d}");
    }
}

/// Unrolling avoids a false positive that collapsing bounds to `+` would
/// produce: `a{2,2}` followed by `a` is a plain three-element chain.
#[test]
fn a_fixed_count_followed_by_the_same_name_is_not_ambiguous() {
    let d = diagnostics(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="2" maxOccurs="2"/>
        <xs:element name="a" type="xs:string"/>
    </xs:sequence>"#));
    assert_eq!(upa_count(&d), 0, "a a a is deterministic:\n{d}");
}

/// A variable-count run followed by the same name genuinely is ambiguous.
#[test]
fn a_variable_count_followed_by_the_same_name_is_ambiguous() {
    let d = diagnostics(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="1" maxOccurs="2"/>
        <xs:element name="a" type="xs:string"/>
    </xs:sequence>"#));
    assert_eq!(upa_count(&d), 1, "{d}");
}

/// Substitution groups are part of the ambiguity question: a head and one of
/// its members admit overlapping name sets.
#[test]
fn substitution_group_members_can_collide_with_their_head() {
    let d = diagnostics(&format!(
        r#"<xs:element name="head" type="xs:string"/>
           <xs:element name="member" type="xs:string" substitutionGroup="tns:head"/>
           {}"#,
        ty(r#"<xs:choice>
                <xs:element ref="tns:head"/>
                <xs:element ref="tns:member"/>
              </xs:choice>"#)
    ));
    assert_eq!(upa_count(&d), 1, "`member` satisfies both branches:\n{d}");
}

#[test]
fn lax_mode_downgrades_upa_to_a_warning() {
    let (_, d) = build_lax(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="0"/>
        <xs:element name="a" type="xs:string"/>
    </xs:sequence>"#));
    assert_eq!(upa_count(&d), 1, "{d}");
    assert!(
        !d.has_errors(),
        "real schemas breach UPA; lax must still build:\n{d}"
    );
}

// ---------------------------------------------------------------------------
// The occurrence budget
// ---------------------------------------------------------------------------

#[test]
fn a_huge_max_occurs_is_widened_rather_than_unrolled() {
    let s = build(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" maxOccurs="100000"/>
    </xs:sequence>"#));
    let t = type_t(&s);
    let ContentModel::Automaton(a) = s.content_model(t).unwrap() else {
        panic!("expected an automaton")
    };
    assert!(
        a.positions().len() <= MAX_POSITIONS,
        "unrolling must stay bounded"
    );
    assert!(a.approximated(), "widening must be recorded");

    // Widening accepts a superset, never a subset.
    assert!(accepts(&s, t, "a"));
    assert!(accepts(&s, t, &"a ".repeat(50)));
}

#[test]
fn an_approximated_model_downgrades_upa_to_a_warning() {
    let d = diagnostics(&ty(r#"<xs:sequence>
        <xs:element name="a" type="xs:string" minOccurs="0" maxOccurs="100000"/>
        <xs:element name="a" type="xs:string"/>
    </xs:sequence>"#));
    let e = d.iter().find(|x| x.code == DiagCode::AmbiguousContentModel);
    if let Some(e) = e {
        assert_eq!(
            e.severity,
            Severity::Warning,
            "an approximate verdict must not be fatal"
        );
        assert!(e.message.contains("approximate"), "{}", e.message);
    }
}

// ---------------------------------------------------------------------------
// The queries the config generator needs
// ---------------------------------------------------------------------------

#[test]
fn possible_children_expands_substitution_groups() {
    let s = build(&format!(
        r#"<xs:element name="geometry" type="xs:string" abstract="true"/>
           <xs:element name="point" type="xs:string" substitutionGroup="tns:geometry"/>
           <xs:element name="curve" type="xs:string" substitutionGroup="tns:geometry"/>
           {}"#,
        ty(r#"<xs:sequence>
                <xs:element ref="tns:geometry" maxOccurs="unbounded"/>
              </xs:sequence>"#)
    ));
    let t = type_t(&s);
    let mut names: Vec<_> = s
        .possible_children(t)
        .into_iter()
        .map(|e| s.names().resolve(s[e].name.local).to_string())
        .collect();
    names.sort();
    // The abstract head cannot itself appear.
    assert_eq!(names, ["curve", "point"]);
}

#[test]
fn child_repeats_covers_the_element_and_its_ancestors() {
    let s = build(&ty(r#"<xs:sequence>
        <xs:element name="once" type="xs:string"/>
        <xs:element name="many" type="xs:string" maxOccurs="unbounded"/>
        <xs:sequence maxOccurs="unbounded">
            <xs:element name="grouped" type="xs:string"/>
        </xs:sequence>
    </xs:sequence>"#));
    let t = type_t(&s);
    let child = |n: &str| {
        s.possible_children(t)
            .into_iter()
            .find(|e| s.names().resolve(s[*e].name.local) == n)
            .unwrap_or_else(|| panic!("no child {n}"))
    };
    assert!(!s.child_repeats(t, child("once")));
    assert!(
        s.child_repeats(t, child("many")),
        "maxOccurs on the element"
    );
    assert!(
        s.child_repeats(t, child("grouped")),
        "a repeating ancestor group makes its children repeat"
    );
}

#[test]
fn child_is_optional_matches_the_schema_exactly() {
    let s = build(&ty(r#"<xs:sequence>
        <xs:element name="required" type="xs:string"/>
        <xs:element name="skippable" type="xs:string" minOccurs="0"/>
        <xs:choice>
            <xs:element name="left" type="xs:string"/>
            <xs:element name="right" type="xs:string"/>
        </xs:choice>
    </xs:sequence>"#));
    let t = type_t(&s);
    let child = |n: &str| {
        s.possible_children(t)
            .into_iter()
            .find(|e| s.names().resolve(s[*e].name.local) == n)
            .unwrap_or_else(|| panic!("no child {n}"))
    };
    assert!(!s.child_is_optional(t, child("required")));
    assert!(s.child_is_optional(t, child("skippable")), "minOccurs=0");
    // Either branch of a choice can be absent, so both are nullable columns.
    assert!(s.child_is_optional(t, child("left")));
    assert!(s.child_is_optional(t, child("right")));
}

#[test]
fn content_stats_summarise_the_whole_schema() {
    let s = build(&format!(
        r#"{}
           <xs:complexType name="U"><xs:all>
             <xs:element name="x" type="xs:string"/>
           </xs:all></xs:complexType>
           <xs:complexType name="V"><xs:attribute name="a" type="xs:string"/></xs:complexType>"#,
        ty(r#"<xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>"#)
    ));
    let stats = s.content_stats();
    assert!(stats.automata >= 1);
    assert_eq!(stats.all_groups, 1);
    assert!(stats.empty >= 1);
    assert_eq!(stats.approximated, 0);
    // `positions` counts automaton positions; xs:all members are not among
    // them, so type T's single element is the only one here.
    assert_eq!(stats.positions, 1);
}

// ---------------------------------------------------------------------------
// Derivation
// ---------------------------------------------------------------------------

/// Extension appends to the base's content model. Building from a type's own
/// particle alone would silently lose every inherited child.
#[test]
fn extension_inherits_the_bases_content() {
    let s = build(
        r#"<xs:complexType name="Base">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="T">
             <xs:complexContent>
               <xs:extension base="tns:Base">
                 <xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence>
               </xs:extension>
             </xs:complexContent>
           </xs:complexType>"#,
    );
    let t = type_t(&s);
    let names: Vec<_> = s
        .possible_children(t)
        .into_iter()
        .map(|e| s.names().resolve(s[e].name.local).to_string())
        .collect();
    assert_eq!(names, ["a", "b"], "base content comes first");

    assert!(accepts(&s, t, "a b"));
    assert!(
        !accepts(&s, t, "b"),
        "the inherited element is still required"
    );
    assert!(!accepts(&s, t, "b a"));
}

/// Extension is transitive.
#[test]
fn extension_chains_accumulate_in_base_order() {
    let s = build(
        r#"<xs:complexType name="A">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="B">
             <xs:complexContent><xs:extension base="tns:A">
               <xs:sequence><xs:element name="b" type="xs:string"/></xs:sequence>
             </xs:extension></xs:complexContent>
           </xs:complexType>
           <xs:complexType name="T">
             <xs:complexContent><xs:extension base="tns:B">
               <xs:sequence><xs:element name="c" type="xs:string"/></xs:sequence>
             </xs:extension></xs:complexContent>
           </xs:complexType>"#,
    );
    let t = type_t(&s);
    assert!(accepts(&s, t, "a b c"));
    assert!(!accepts(&s, t, "c b a"));
}

/// Restriction states the content model in full, so an ancestor's particles
/// are *not* part of it.
#[test]
fn restriction_replaces_the_bases_content() {
    let s = build(
        r#"<xs:complexType name="Base">
             <xs:sequence>
               <xs:element name="a" type="xs:string"/>
               <xs:element name="b" type="xs:string" minOccurs="0"/>
             </xs:sequence>
           </xs:complexType>
           <xs:complexType name="T">
             <xs:complexContent>
               <xs:restriction base="tns:Base">
                 <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
               </xs:restriction>
             </xs:complexContent>
           </xs:complexType>"#,
    );
    let t = type_t(&s);
    let names: Vec<_> = s
        .possible_children(t)
        .into_iter()
        .map(|e| s.names().resolve(s[e].name.local).to_string())
        .collect();
    assert_eq!(names, ["a"], "restriction does not inherit particles");
    assert!(accepts(&s, t, "a"));
    assert!(!accepts(&s, t, "a b"), "the restriction dropped b");
}

/// An extension that adds only attributes still exposes the base's children.
#[test]
fn an_attribute_only_extension_keeps_the_bases_children() {
    let s = build(
        r#"<xs:complexType name="Base">
             <xs:sequence><xs:element name="a" type="xs:string"/></xs:sequence>
           </xs:complexType>
           <xs:complexType name="T">
             <xs:complexContent>
               <xs:extension base="tns:Base">
                 <xs:attribute name="id" type="xs:string"/>
               </xs:extension>
             </xs:complexContent>
           </xs:complexType>"#,
    );
    let t = type_t(&s);
    assert_eq!(
        s.possible_children(t).len(),
        1,
        "an empty own-particle is not an empty model"
    );
    assert!(accepts(&s, t, "a"));
}

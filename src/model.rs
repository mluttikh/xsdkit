//! The schema component model — the product of this crate.
//!
//! XSD is three languages: schema *documents*, schema *components*, and
//! validation *semantics* over those components. This module is the middle
//! layer, which the specification defines all its semantics against. Every
//! consumer — typed reading, code generation, anything else — is built on
//! this and never reaches back into the document syntax.
//!
//! # Why arenas
//!
//! Schemas are cyclic graphs: a complex type owns a particle referencing an
//! element declared with that same type. `Rc<RefCell<_>>` would leak and put
//! borrow lifetimes in every signature. Instead every component lives in an
//! [`Arena`] and is named by a 4-byte `Copy` index, which also makes the
//! whole [`Schemas`] one `Send + Sync` value and keeps it serializable for a
//! future compiled-schema cache.

use crate::datatypes::{Builtin, FacetSet, Variety};
use crate::diagnostics::Span;
use crate::names::{Interner, Namespace, QName};
use fxhash::FxHashMap;
use std::ops::Index;

/// A typed, append-only store of one kind of component.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Arena<T> {
    items: Vec<T>,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }

    pub(crate) fn push(&mut self, item: T) -> u32 {
        let id = self.items.len() as u32;
        self.items.push(item);
        id
    }

    pub(crate) fn get_mut(&mut self, i: u32) -> &mut T {
        &mut self.items[i as usize]
    }

    pub(crate) fn get(&self, i: u32) -> &T {
        &self.items[i as usize]
    }
}

/// Declares an id newtype over an arena index.
macro_rules! component_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        pub struct $name(pub(crate) u32);

        impl $name {
            /// Stands in for a reference not yet resolved.
            ///
            /// Present only between loading and resolution; [`Schemas`] is
            /// never handed out with one still in place. Not every component
            /// kind is referenced by name, so some are never placeholders.
            #[allow(dead_code)]
            pub(crate) const PLACEHOLDER: Self = Self(u32::MAX);

            #[allow(dead_code)]
            pub(crate) fn is_placeholder(self) -> bool {
                self.0 == u32::MAX
            }

            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

component_id!(
    /// A simple or complex type definition.
    TypeId
);
component_id!(
    /// An element declaration, global or local.
    ElementId
);
component_id!(
    /// An attribute declaration, global or local.
    AttributeId
);
component_id!(
    /// A particle: an occurrence range around a term.
    ParticleId
);
component_id!(
    /// A named model group definition (`xs:group`).
    GroupId
);
component_id!(
    /// A named attribute group definition (`xs:attributeGroup`).
    AttrGroupId
);
component_id!(
    /// An identity constraint (`xs:unique`, `xs:key`, `xs:keyref`).
    IdcId
);
component_id!(
    /// A notation declaration.
    NotationId
);
component_id!(
    /// An annotation, holding documentation and `appinfo`.
    AnnotationId
);

/// Where a declaration is visible.
///
/// Local declarations are scoped to the complex type containing them, so
/// `{name, targetNamespace}` is **not** a key for them.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Scope {
    Global,
    Local(TypeId),
}

/// A `default` or `fixed` value on a declaration.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ValueConstraint {
    Default(String),
    Fixed(String),
}

impl ValueConstraint {
    pub fn value(&self) -> &str {
        match self {
            ValueConstraint::Default(v) | ValueConstraint::Fixed(v) => v,
        }
    }

    pub fn is_fixed(&self) -> bool {
        matches!(self, ValueConstraint::Fixed(_))
    }
}

/// The `block` / `final` / `blockDefault` / `finalDefault` sets.
///
/// Post-composition these are what .NET calls `BlockResolved` and
/// `FinalResolved`; here there is only the resolved form, because
/// [`Schemas`] never exists before composition.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DerivationSet {
    pub extension: bool,
    pub restriction: bool,
    pub substitution: bool,
    pub list: bool,
    pub union: bool,
}

impl DerivationSet {
    pub const ALL: Self = Self {
        extension: true,
        restriction: true,
        substitution: true,
        list: true,
        union: true,
    };

    pub fn is_empty(self) -> bool {
        self == Self::default()
    }

    /// Both sets at once. `xsi:type` is blocked by the element declaration's
    /// own `block` *and* by the declared type's, taken together.
    pub fn union(self, other: Self) -> Self {
        Self {
            extension: self.extension || other.extension,
            restriction: self.restriction || other.restriction,
            substitution: self.substitution || other.substitution,
            list: self.list || other.list,
            union: self.union || other.union,
        }
    }

    /// The keywords a `block` or `final` attribute may contain, across every
    /// context one of them appears in.
    ///
    /// Which subset is legal depends on where the attribute sits — `final` on
    /// a simple type takes `list` and `union`, `block` on an element takes
    /// `substitution` — but a token outside this set is wrong everywhere.
    pub const KEYWORDS: [&str; 5] = ["extension", "restriction", "substitution", "list", "union"];

    /// Parses a `block`/`final` attribute value: `#all`, or a space-separated
    /// list of keywords. Unknown keywords are ignored by the caller's rules.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s == "#all" {
            return Self::ALL;
        }
        let mut out = Self::default();
        for tok in s.split_whitespace() {
            match tok {
                "extension" => out.extension = true,
                "restriction" => out.restriction = true,
                "substitution" => out.substitution = true,
                "list" => out.list = true,
                "union" => out.union = true,
                _ => {}
            }
        }
        out
    }
}

/// How a complex type is derived from its base.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DerivationMethod {
    Extension,
    Restriction,
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ElementDecl {
    pub name: QName,
    pub type_id: TypeId,
    pub scope: Scope,
    pub nillable: bool,
    pub is_abstract: bool,
    /// Heads this element may substitute for. XSD 1.1 permits several.
    pub substitution_group: Vec<ElementId>,
    pub value_constraint: Option<ValueConstraint>,
    pub block: DerivationSet,
    pub final_: DerivationSet,
    pub identity_constraints: Vec<IdcId>,
    pub annotation: Option<AnnotationId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AttributeDecl {
    pub name: QName,
    pub type_id: TypeId,
    pub scope: Scope,
    pub value_constraint: Option<ValueConstraint>,
    pub annotation: Option<AnnotationId>,
    pub span: Span,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AttributeUseKind {
    Optional,
    Required,
    Prohibited,
}

/// An attribute declaration as used by one complex type.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AttributeUse {
    pub attribute: AttributeId,
    pub kind: AttributeUseKind,
    /// Overrides the declaration's own constraint when present.
    pub value_constraint: Option<ValueConstraint>,
}

impl AttributeUse {
    pub fn is_required(&self) -> bool {
        self.kind == AttributeUseKind::Required
    }
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TypeDefinition {
    Simple(SimpleType),
    Complex(ComplexType),
}

impl TypeDefinition {
    pub fn name(&self) -> Option<QName> {
        match self {
            TypeDefinition::Simple(t) => t.name,
            TypeDefinition::Complex(t) => t.name,
        }
    }

    pub fn is_simple(&self) -> bool {
        matches!(self, TypeDefinition::Simple(_))
    }

    pub fn as_simple(&self) -> Option<&SimpleType> {
        match self {
            TypeDefinition::Simple(t) => Some(t),
            TypeDefinition::Complex(_) => None,
        }
    }

    pub fn as_complex(&self) -> Option<&ComplexType> {
        match self {
            TypeDefinition::Complex(t) => Some(t),
            TypeDefinition::Simple(_) => None,
        }
    }

    pub fn base(&self) -> TypeId {
        match self {
            TypeDefinition::Simple(t) => t.base,
            TypeDefinition::Complex(t) => t.base,
        }
    }

    pub fn span(&self) -> &Span {
        match self {
            TypeDefinition::Simple(t) => &t.span,
            TypeDefinition::Complex(t) => &t.span,
        }
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SimpleType {
    /// Absent for an anonymous type declared inline.
    pub name: Option<QName>,
    pub base: TypeId,
    pub variety: Variety,
    /// Set for atomic varieties; absent for list and union.
    pub primitive: Option<Builtin>,
    /// The built-in this type *is*, when it is one.
    pub builtin: Option<Builtin>,
    /// The item type of a list variety.
    pub item_type: Option<TypeId>,
    /// The member types of a union variety, in declaration order — the order
    /// in which they are tried.
    pub member_types: Vec<TypeId>,
    pub facets: FacetSet,
    pub final_: DerivationSet,
    pub annotation: Option<AnnotationId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ComplexType {
    pub name: Option<QName>,
    pub base: TypeId,
    pub derivation: DerivationMethod,
    pub content: ContentType,
    pub attribute_uses: Vec<AttributeUse>,
    /// Attribute group references, kept unexpanded until composition.
    pub attribute_group_refs: Vec<AttrGroupId>,
    pub attribute_wildcard: Option<Wildcard>,
    /// XSD 1.1 open content, from `xs:openContent` here or
    /// `xs:defaultOpenContent` on the schema.
    pub open_content: Option<OpenContent>,
    pub is_abstract: bool,
    pub block: DerivationSet,
    pub final_: DerivationSet,
    pub annotation: Option<AnnotationId>,
    pub span: Span,
}

/// What a complex type may contain.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ContentType {
    /// No child elements and no character data.
    Empty,
    /// Character data validated against a simple type.
    Simple(TypeId),
    /// Child elements only.
    ElementOnly(ParticleId),
    /// Child elements interleaved with character data.
    Mixed(ParticleId),
}

impl ContentType {
    /// The content particle, if this type has one.
    ///
    /// `Mixed` without a particle is a legitimate state — character data and
    /// nothing else, which is what `xs:anyType` starts as — so a placeholder
    /// here means "no particle", not "unresolved".
    pub fn particle(self) -> Option<ParticleId> {
        match self {
            ContentType::ElementOnly(p) | ContentType::Mixed(p) if !p.is_placeholder() => Some(p),
            _ => None,
        }
    }

    pub fn is_mixed(self) -> bool {
        matches!(self, ContentType::Mixed(_))
    }
}

// ---------------------------------------------------------------------------
// Particles and model groups
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum MaxOccurs {
    Bounded(u32),
    Unbounded,
}

impl MaxOccurs {
    pub fn is_repeating(self) -> bool {
        match self {
            MaxOccurs::Unbounded => true,
            MaxOccurs::Bounded(n) => n > 1,
        }
    }

    pub fn as_u32(self) -> Option<u32> {
        match self {
            MaxOccurs::Bounded(n) => Some(n),
            MaxOccurs::Unbounded => None,
        }
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Particle {
    pub min_occurs: u32,
    pub max_occurs: MaxOccurs,
    pub term: Term,
    pub span: Span,
}

impl Particle {
    /// Whether this particle may match more than once.
    ///
    /// This is the primitive the future config generator's table/column split
    /// is built on.
    pub fn is_repeating(&self) -> bool {
        self.max_occurs.is_repeating()
    }

    /// Whether this particle may match zero times, making its content
    /// optional and hence nullable.
    pub fn is_optional(&self) -> bool {
        self.min_occurs == 0
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Term {
    Element(ElementId),
    Wildcard(Wildcard),
    /// An inline `xs:sequence`, `xs:choice` or `xs:all`.
    Group(ModelGroup),
    /// A reference to a named group definition. Kept distinct from `Group`
    /// so provenance survives into later phases.
    GroupRef(GroupId),
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Compositor {
    Sequence,
    Choice,
    All,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ModelGroup {
    pub compositor: Compositor,
    pub particles: Vec<ParticleId>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ModelGroupDef {
    pub name: QName,
    pub group: ModelGroup,
    pub annotation: Option<AnnotationId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AttributeGroupDef {
    pub name: QName,
    pub attribute_uses: Vec<AttributeUse>,
    pub attribute_group_refs: Vec<AttrGroupId>,
    pub attribute_wildcard: Option<Wildcard>,
    pub annotation: Option<AnnotationId>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Wildcards
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ProcessContents {
    Skip,
    Lax,
    Strict,
}

/// Which namespaces a wildcard admits.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NamespaceConstraint {
    /// `##any`.
    Any,
    /// `##other`, or XSD 1.1's `notNamespace` — anything but these.
    /// `None` inside the list means "no namespace".
    Not(Vec<Option<Namespace>>),
    /// An explicit list. `None` means "no namespace".
    Enumeration(Vec<Option<Namespace>>),
}

impl NamespaceConstraint {
    pub fn admits(&self, ns: Option<Namespace>) -> bool {
        match self {
            NamespaceConstraint::Any => true,
            NamespaceConstraint::Not(list) => !list.contains(&ns),
            NamespaceConstraint::Enumeration(list) => list.contains(&ns),
        }
    }

    /// The constraint admitting only what both admit.
    ///
    /// *Attribute Wildcard Intersection*. Written for the XSD 1.1 form, where
    /// two negations combine into one — 1.0 could only spell `##other` and
    /// called that case an error, but the answer here is the same shape.
    pub fn intersect(&self, other: &Self) -> Self {
        use NamespaceConstraint::*;
        match (self, other) {
            (Any, o) | (o, Any) => o.clone(),
            (Enumeration(a), Enumeration(b)) => {
                Enumeration(a.iter().filter(|n| b.contains(n)).cloned().collect())
            }
            // What the list names, less what the negation bars.
            (Not(n), Enumeration(e)) | (Enumeration(e), Not(n)) => {
                Enumeration(e.iter().filter(|x| !n.contains(x)).cloned().collect())
            }
            // Barred by either is barred by both.
            (Not(a), Not(b)) => {
                let mut out = a.clone();
                out.extend(b.iter().filter(|x| !a.contains(x)).cloned());
                Not(out)
            }
        }
    }

    /// The constraint admitting whatever either admits.
    ///
    /// *Attribute Wildcard Union*, which is how an extension combines its own
    /// wildcard with the one it inherits.
    pub fn union(&self, other: &Self) -> Self {
        use NamespaceConstraint::*;
        match (self, other) {
            (Any, _) | (_, Any) => Any,
            (Enumeration(a), Enumeration(b)) => {
                let mut out = a.clone();
                out.extend(b.iter().filter(|x| !a.contains(x)).cloned());
                Enumeration(out)
            }
            // A negation stops barring whatever the list admits.
            (Not(n), Enumeration(e)) | (Enumeration(e), Not(n)) => {
                Not(n.iter().filter(|x| !e.contains(x)).cloned().collect())
            }
            // Only what both bar stays barred.
            (Not(a), Not(b)) => Not(a.iter().filter(|x| b.contains(x)).cloned().collect()),
        }
    }

    /// Whether this constraint admits a namespace URI the schema may never
    /// have seen.
    ///
    /// A wildcard exists precisely to admit names the schema does not
    /// declare, so it cannot be answered with interned ids alone: a URI that
    /// was never interned is not `None` — `None` means *no* namespace — it is
    /// simply not any of the ones enumerated.
    pub fn admits_uri(&self, names: &Interner, uri: Option<&str>) -> bool {
        let resolved = match uri {
            None | Some("") => Some(None),
            Some(u) => names.lookup(u).map(|sym| Some(Namespace::from_symbol(sym))),
        };
        match (self, resolved) {
            (NamespaceConstraint::Any, _) => true,
            (_, Some(ns)) => self.admits(ns),
            // The URI is real but unknown to this schema, so it cannot be one
            // of the enumerated or excluded ones.
            (NamespaceConstraint::Not(_), None) => true,
            (NamespaceConstraint::Enumeration(_), None) => false,
        }
    }
}

/// Where an XSD 1.1 `xs:openContent` wildcard may appear.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum OpenContentMode {
    /// Between any two particles, and at either end.
    Interleave,
    /// Only after the declared content is complete.
    Suffix,
}

/// XSD 1.1 open content: a wildcard admitted alongside a declared content
/// model rather than as part of it.
///
/// Kept beside the model rather than compiled into the automaton, because
/// interleaved open content is the *shuffle* of the declared language with
/// the wildcard's — which a position automaton cannot express, but a matcher
/// can decide in one extra check.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct OpenContent {
    pub mode: OpenContentMode,
    pub wildcard: Wildcard,
}

#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Wildcard {
    pub namespace: NamespaceConstraint,
    pub process_contents: ProcessContents,
    /// XSD 1.1 `notQName`, excluding specific names from an otherwise
    /// admitted namespace.
    pub not_qname: Vec<QName>,
    /// XSD 1.1 `notQName="##defined"`: exclude every name that has a global
    /// element declaration, so the wildcard admits only what the schema does
    /// *not* describe.
    pub not_defined: bool,
    /// XSD 1.1 `notQName="##definedSibling"`: exclude every name written out
    /// in the content model this wildcard sits in.
    ///
    /// The point is to let a wildcard sit beside named particles without
    /// competing with them — which is also why it settles a Unique Particle
    /// Attribution question that would otherwise be ambiguous.
    pub not_defined_sibling: bool,
}

impl Wildcard {
    /// The wildcard admitting only what both admit.
    ///
    /// Attribute wildcards reaching one complex type from several
    /// `xs:attributeGroup` references combine this way: an attribute has to
    /// satisfy every wildcard that reached the type, not just one of them.
    /// `processContents` and the name exclusions come from `self`, which the
    /// specification makes the type's own wildcard wherever it has one.
    pub fn intersect(&self, other: &Wildcard) -> Wildcard {
        let mut not_qname = self.not_qname.clone();
        not_qname.extend(
            other
                .not_qname
                .iter()
                .filter(|q| !self.not_qname.contains(q)),
        );
        Wildcard {
            namespace: self.namespace.intersect(&other.namespace),
            process_contents: self.process_contents,
            not_qname,
            not_defined: self.not_defined || other.not_defined,
            not_defined_sibling: self.not_defined_sibling || other.not_defined_sibling,
        }
    }

    /// The wildcard admitting whatever either admits, which is how an
    /// extension combines its own with the base's.
    pub fn union(&self, other: &Wildcard) -> Wildcard {
        Wildcard {
            namespace: self.namespace.union(&other.namespace),
            process_contents: self.process_contents,
            not_qname: self
                .not_qname
                .iter()
                .filter(|q| other.not_qname.contains(q))
                .copied()
                .collect(),
            not_defined: self.not_defined && other.not_defined,
            not_defined_sibling: self.not_defined_sibling && other.not_defined_sibling,
        }
    }
}

// ---------------------------------------------------------------------------
// Identity constraints, notations, annotations
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IdcKind {
    Unique,
    Key,
    KeyRef,
}

/// An `xs:unique`, `xs:key` or `xs:keyref`.
///
/// A `keyref` is a declared foreign key between two element sets, which is
/// exactly the relational structure a future config generator needs for its
/// `links:` entries.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IdentityConstraint {
    pub name: QName,
    pub kind: IdcKind,
    /// The restricted XPath subset selecting the constrained node set.
    pub selector: String,
    /// The restricted XPath subsets selecting the key fields.
    pub fields: Vec<String>,
    /// The same two, parsed. Prefixes bind in the *schema* document, so this
    /// is settled at load time where those bindings are still in hand.
    pub(crate) selector_paths: crate::identity::Paths,
    pub(crate) field_paths: Vec<crate::identity::Paths>,
    /// The key a `keyref` refers to.
    pub refer: Option<IdcId>,
    pub annotation: Option<AnnotationId>,
    pub span: Span,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NotationDecl {
    pub name: QName,
    pub public_id: Option<String>,
    pub system_id: Option<String>,
    pub annotation: Option<AnnotationId>,
    pub span: Span,
}

/// Machine-readable annotation content, kept verbatim.
///
/// This is the seam a caller's own conventions hang on: whatever a schema
/// family encodes here needs the original XML, not a summary of it.
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppInfo {
    pub source: Option<String>,
    /// The `appinfo` element's children, re-serialized.
    pub xml: String,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Annotation {
    pub documentation: Vec<String>,
    pub appinfo: Vec<AppInfo>,
}

impl Annotation {
    pub fn is_empty(&self) -> bool {
        self.documentation.is_empty() && self.appinfo.is_empty()
    }

    /// The documentation entries joined into one string.
    pub fn doc(&self) -> String {
        self.documentation.join("\n\n")
    }
}

// ---------------------------------------------------------------------------
// Symbol tables
// ---------------------------------------------------------------------------

/// XSD's seven independent symbol spaces.
///
/// A name collides only within one space *and* one namespace, so `Foo` the
/// type and `Foo` the element are unrelated components.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum SymbolSpace {
    Type,
    Element,
    Attribute,
    ModelGroup,
    AttributeGroup,
    Notation,
    IdentityConstraint,
}

impl SymbolSpace {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolSpace::Type => "type",
            SymbolSpace::Element => "element",
            SymbolSpace::Attribute => "attribute",
            SymbolSpace::ModelGroup => "model group",
            SymbolSpace::AttributeGroup => "attribute group",
            SymbolSpace::Notation => "notation",
            SymbolSpace::IdentityConstraint => "identity constraint",
        }
    }
}

#[derive(Clone, Default, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SymbolTables {
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub types: FxHashMap<QName, TypeId>,
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub elements: FxHashMap<QName, ElementId>,
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub attributes: FxHashMap<QName, AttributeId>,
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub model_groups: FxHashMap<QName, GroupId>,
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub attribute_groups: FxHashMap<QName, AttrGroupId>,
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub notations: FxHashMap<QName, NotationId>,
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub identity_constraints: FxHashMap<QName, IdcId>,
}

/// Provenance for one loaded schema document.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SourceDocument {
    pub uri: String,
    pub target_namespace: Option<Namespace>,
    /// True when this document was absorbed into an includer's namespace by
    /// a chameleon include.
    pub chameleon: bool,
    /// The `version` attribute of `xs:schema`, verbatim.
    ///
    /// The specification declares this as a bare `xs:token` — no pattern, no
    /// enumeration, no default, and no processing role, unlike the
    /// `elementFormDefault` beside it. It means whatever its author meant, so
    /// it is reported rather than interpreted.
    ///
    /// In practice the version that identifies a *vocabulary* lives in the
    /// target namespace instead, and this attribute carries the patch level
    /// underneath it: GML's namespace is `.../gml/3.2` while its documents
    /// say `version="3.2.2"`.
    pub version: Option<String>,
}

// ---------------------------------------------------------------------------
// Schemas
// ---------------------------------------------------------------------------

/// A compiled set of schema components.
///
/// Produced only by [`crate::SchemaSetBuilder::build`], so it never exists in
/// an unresolved state — "did you call `Compile()`?", the .NET footgun, is
/// not representable here.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Schemas {
    pub(crate) types: Arena<TypeDefinition>,
    pub(crate) elements: Arena<ElementDecl>,
    pub(crate) attributes: Arena<AttributeDecl>,
    pub(crate) particles: Arena<Particle>,
    pub(crate) model_groups: Arena<ModelGroupDef>,
    pub(crate) attribute_groups: Arena<AttributeGroupDef>,
    pub(crate) identity_constraints: Arena<IdentityConstraint>,
    pub(crate) notations: Arena<NotationDecl>,
    pub(crate) annotations: Arena<Annotation>,

    pub(crate) names: Interner,
    pub(crate) globals: SymbolTables,
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub(crate) builtins: FxHashMap<Builtin, TypeId>,
    /// Element id -> every element that may substitute for it, transitively.
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub(crate) substitution_closure: FxHashMap<ElementId, Vec<ElementId>>,
    /// Compiled content models, indexed by `TypeId`. `None` for simple types.
    pub(crate) content_models: Vec<Option<crate::content::Content>>,
    pub(crate) documents: Vec<SourceDocument>,
    /// Which XSD the documents were read as. Not the `xs:schema/@version`
    /// attribute (that is [`SourceDocument::version`]) — this is the language
    /// the reader applied, and the two lexical spaces differ.
    pub(crate) xsd_version: crate::load::Version,
}

macro_rules! arena_index {
    ($id:ty, $item:ty, $field:ident, $getter:ident, $iter:ident) => {
        impl Index<$id> for Schemas {
            type Output = $item;
            fn index(&self, id: $id) -> &$item {
                debug_assert!(!id.is_placeholder(), "unresolved reference reached Schemas");
                self.$field.get(id.0)
            }
        }

        impl Schemas {
            #[doc = concat!("Looks a component up by id, returning `None` if out of range.")]
            pub fn $getter(&self, id: $id) -> Option<&$item> {
                if id.is_placeholder() || id.index() >= self.$field.len() {
                    None
                } else {
                    Some(self.$field.get(id.0))
                }
            }

            #[doc = concat!("Iterates every component of this kind with its id.")]
            pub fn $iter(&self) -> impl Iterator<Item = ($id, &$item)> {
                self.$field
                    .iter()
                    .enumerate()
                    .map(|(i, t)| (<$id>::from_index(i), t))
            }
        }

        impl $id {
            pub(crate) fn from_index(i: usize) -> Self {
                Self(i as u32)
            }
        }
    };
}

arena_index!(TypeId, TypeDefinition, types, get_type, iter_types);
arena_index!(ElementId, ElementDecl, elements, get_element, iter_elements);
arena_index!(
    AttributeId,
    AttributeDecl,
    attributes,
    get_attribute,
    iter_attributes
);
arena_index!(
    ParticleId,
    Particle,
    particles,
    get_particle,
    iter_particles
);
arena_index!(
    GroupId,
    ModelGroupDef,
    model_groups,
    get_model_group,
    iter_model_groups
);
arena_index!(
    AttrGroupId,
    AttributeGroupDef,
    attribute_groups,
    get_attribute_group,
    iter_attribute_groups
);
arena_index!(
    IdcId,
    IdentityConstraint,
    identity_constraints,
    get_identity_constraint,
    iter_identity_constraints
);
arena_index!(
    NotationId,
    NotationDecl,
    notations,
    get_notation,
    iter_notations
);
arena_index!(
    AnnotationId,
    Annotation,
    annotations,
    get_annotation,
    iter_annotations
);

impl Schemas {
    /// The string interner, for turning [`QName`]s back into text.
    pub fn names(&self) -> &Interner {
        &self.names
    }

    /// The global symbol tables, one map per symbol space.
    pub fn globals(&self) -> &SymbolTables {
        &self.globals
    }

    /// The documents this schema set was built from.
    pub fn documents(&self) -> &[SourceDocument] {
        &self.documents
    }

    /// Which XSD these documents were read as.
    ///
    /// Not the `xs:schema/@version` attribute — that is per-document and says
    /// what the *vocabulary* calls itself ([`SourceDocument::version`]). This
    /// is the language the reader applied, and it decides questions the
    /// components alone cannot: the year `0000` and `+INF` are XSD 1.1 forms
    /// that 1.0 rejects.
    pub fn xsd_version(&self) -> crate::load::Version {
        self.xsd_version
    }

    /// Builds a [`QName`] from a namespace URI and local name, for lookups.
    ///
    /// Returns `None` when either string was never interned, which means no
    /// component in this schema set can carry that name.
    pub fn qname(&self, ns: Option<&str>, local: &str) -> Option<QName> {
        let local = self.names.lookup(local)?;
        let ns = match ns {
            None | Some("") => None,
            Some(u) => Some(Namespace::from_symbol(self.names.lookup(u)?)),
        };
        Some(QName { ns, local })
    }

    /// Looks up a global element declaration, by id.
    ///
    /// [`Self::element`] returns the same declaration as a navigable
    /// reference, which is what most callers want; this is the form to reach
    /// for when the id is the thing being stored or compared.
    pub fn element_id(&self, ns: Option<&str>, local: &str) -> Option<ElementId> {
        self.globals.elements.get(&self.qname(ns, local)?).copied()
    }

    /// Looks up a global type definition, by id. See [`Self::element_id`].
    pub fn type_id(&self, ns: Option<&str>, local: &str) -> Option<TypeId> {
        self.globals.types.get(&self.qname(ns, local)?).copied()
    }

    /// Looks up a global attribute declaration, by id. See
    /// [`Self::element_id`].
    pub fn attribute_id(&self, ns: Option<&str>, local: &str) -> Option<AttributeId> {
        self.globals
            .attributes
            .get(&self.qname(ns, local)?)
            .copied()
    }

    /// The `TypeId` of a built-in datatype. Always present.
    pub fn builtin(&self, b: Builtin) -> TypeId {
        self.builtins[&b]
    }

    /// The built-in a type *is*, if it is one.
    pub fn as_builtin(&self, id: TypeId) -> Option<Builtin> {
        self[id].as_simple().and_then(|t| t.builtin)
    }

    /// Renders a name in James Clark notation, `{ns}local`.
    pub fn display_name(&self, q: QName) -> String {
        self.names.display(q)
    }

    /// Every element that may appear where `head` is permitted, `head`
    /// included when it is not abstract.
    ///
    /// Substitution is transitive, so this is the closure, not the direct
    /// members. Without it you cannot know which element names may appear at
    /// a position in a GML, UBL or WITSML document.
    pub fn substitution_closure(&self, head: ElementId) -> Vec<ElementId> {
        let mut out = Vec::new();
        if !self[head].is_abstract {
            out.push(head);
        }
        if let Some(members) = self.substitution_closure.get(&head) {
            out.extend(members.iter().copied());
        }
        out
    }

    /// The substitution-group members that may actually stand in for `head`.
    ///
    /// [`Self::substitution_closure`] answers which elements *are* in the
    /// group; this answers which the head permits to replace it. `block` on
    /// the head bars substitution outright, or bars the derivation method a
    /// member's type used to reach the head's — so a member is admitted only
    /// if its type reaches the head's without a barred step.
    pub fn substitutable_for(&self, head: ElementId) -> Vec<ElementId> {
        let head_decl = &self[head];
        // The element's `{disallowed substitutions}` *and* the head type's
        // `{prohibited substitutions}`: a type may bar being restricted or
        // extended into a substitute without the element saying anything.
        let blocked = head_decl.block.union(
            self[head_decl.type_id]
                .as_complex()
                .map(|c| c.block)
                .unwrap_or_default(),
        );
        if blocked.is_empty() {
            return self.substitution_closure(head);
        }
        let head_type = head_decl.type_id;
        // Only when a derivation method is actually barred is the chain worth
        // walking. Whether a member's type is related to the head's at all is
        // a *schema* validity question, answered elsewhere — asking it here
        // would throw out members of a group the schema already accepted.
        let by_derivation = blocked.extension || blocked.restriction;
        self.substitution_closure(head)
            .into_iter()
            .filter(|&m| {
                m == head
                    || (!blocked.substitution
                        && (!by_derivation
                            || self.derives_from_unblocked(self[m].type_id, head_type, blocked)))
            })
            .collect()
    }

    /// Walks a type's base chain from the type itself up to `xs:anyType`.
    pub fn base_chain(&self, mut id: TypeId) -> Vec<TypeId> {
        let mut out = vec![id];
        let mut guard = 0usize;
        while let Some(def) = self.get_type(id) {
            let base = def.base();
            if base == id || base.is_placeholder() {
                break;
            }
            out.push(base);
            id = base;
            guard += 1;
            if guard > self.types.len() {
                break; // a cycle survived resolution; stop rather than hang
            }
        }
        out
    }

    /// Whether `derived` is `base`, or is derived from it.
    pub fn derives_from(&self, derived: TypeId, base: TypeId) -> bool {
        self.base_chain(derived).contains(&base)
    }

    /// Whether `derived` reaches `base` without using a derivation method
    /// that `blocked` disallows.
    ///
    /// The set applies at *every* step of the chain, not only the last, which
    /// is what makes `block="extension"` on a base type stop an `xsi:type`
    /// naming a restriction of an extension of it. A simple type's step
    /// counts as a restriction: `block` never carries `list` or `union`, so
    /// the distinction cannot be observed here.
    pub fn derives_from_unblocked(
        &self,
        derived: TypeId,
        base: TypeId,
        blocked: DerivationSet,
    ) -> bool {
        let mut id = derived;
        let mut guard = 0usize;
        loop {
            if id == base {
                return true;
            }
            let Some(def) = self.get_type(id) else {
                return false;
            };
            let step_blocked = match def {
                TypeDefinition::Complex(c) => match c.derivation {
                    DerivationMethod::Extension => blocked.extension,
                    DerivationMethod::Restriction => blocked.restriction,
                },
                TypeDefinition::Simple(_) => blocked.restriction,
            };
            if step_blocked {
                return false;
            }
            let next = def.base();
            if next == id || next.is_placeholder() {
                return false;
            }
            id = next;
            guard += 1;
            if guard > self.types.len() {
                return false;
            }
        }
    }

    /// Every attribute use on a complex type, with inherited attribute groups
    /// already merged in.
    pub fn attribute_uses(&self, id: TypeId) -> &[AttributeUse] {
        match &self[id] {
            TypeDefinition::Complex(t) => &t.attribute_uses,
            TypeDefinition::Simple(_) => &[],
        }
    }

    /// The direct child particles of a particle whose term is a group.
    ///
    /// Follows a `GroupRef` through to the named definition's model group.
    pub fn child_particles(&self, id: ParticleId) -> Vec<ParticleId> {
        match &self[id].term {
            Term::Group(g) => g.particles.clone(),
            Term::GroupRef(gid) => self[*gid].group.particles.clone(),
            _ => Vec::new(),
        }
    }

    /// Counts of every arena, for diagnostics and tests.
    pub fn component_counts(&self) -> ComponentCounts {
        ComponentCounts {
            types: self.types.len(),
            elements: self.elements.len(),
            attributes: self.attributes.len(),
            particles: self.particles.len(),
            model_groups: self.model_groups.len(),
            attribute_groups: self.attribute_groups.len(),
            identity_constraints: self.identity_constraints.len(),
            notations: self.notations.len(),
            annotations: self.annotations.len(),
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct ComponentCounts {
    pub types: usize,
    pub elements: usize,
    pub attributes: usize,
    pub particles: usize,
    pub model_groups: usize,
    pub attribute_groups: usize,
    pub identity_constraints: usize,
    pub notations: usize,
    pub annotations: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivation_sets_parse() {
        assert_eq!(DerivationSet::parse("#all"), DerivationSet::ALL);
        let d = DerivationSet::parse("extension restriction");
        assert!(d.extension && d.restriction && !d.substitution);
        assert!(DerivationSet::parse("").is_empty());
        assert!(DerivationSet::parse("nonsense").is_empty());
    }

    #[test]
    fn max_occurs_repetition() {
        assert!(!MaxOccurs::Bounded(1).is_repeating());
        assert!(!MaxOccurs::Bounded(0).is_repeating());
        assert!(MaxOccurs::Bounded(2).is_repeating());
        assert!(MaxOccurs::Unbounded.is_repeating());
        assert_eq!(MaxOccurs::Unbounded.as_u32(), None);
    }

    #[test]
    fn namespace_constraints_admit_the_right_names() {
        let any = NamespaceConstraint::Any;
        assert!(any.admits(None));
        let not_none = NamespaceConstraint::Not(vec![None]);
        assert!(!not_none.admits(None));
        let enumerated = NamespaceConstraint::Enumeration(vec![None]);
        assert!(enumerated.admits(None));
    }

    #[test]
    fn placeholders_are_recognisable() {
        assert!(TypeId::PLACEHOLDER.is_placeholder());
        assert!(!TypeId::from_index(0).is_placeholder());
    }
}

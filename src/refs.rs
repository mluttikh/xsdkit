//! A navigable view over a compiled schema.
//!
//! [`Schemas`] stores components in arenas and addresses them by `Copy` id,
//! which is the right representation — a schema is a cyclic graph, and ids
//! keep it flat, cheap to copy and free of reference counting. It is not the
//! right thing to *ask questions with*. Answering "what may go inside a
//! report?" through ids reads:
//!
//! ```text
//! let t = schemas[report].type_id;
//! let base = schemas[t].as_complex().unwrap().base;
//! schemas.possible_children(base)
//! ```
//!
//! The Python bindings never exposed that, because nobody would use it. They
//! grew their own object layer instead, and it drifted. This module is that
//! layer, built once in Rust for both to project.
//!
//! A reference is two words — a borrow of the schema and an id — so it is
//! `Copy`, allocates nothing, and cannot outlive the schema it came from.
//! Every one of them keeps [`id`](ElementRef::id) and a raw-component
//! accessor, so dropping back down to the id API costs nothing either.
//!
//! ```no_run
//! # let schemas = xsdkit::SchemaSetBuilder::new().file("report.xsd").compile().into_result().unwrap();
//! let report = schemas.element(Some("urn:example"), "report").unwrap();
//! for child in report.children() {
//!     println!(
//!         "{}{}{}: {}",
//!         child.local_name(),
//!         if child.repeats() { "+" } else { "" },
//!         if child.optional() { "?" } else { "" },
//!         child.type_of().display_name(),
//!     );
//! }
//! ```

use crate::content::Child;
use crate::datatypes::{FacetSet, Variety};
use crate::model::{
    AttributeDecl, AttributeId, AttributeUse, ContentType, ElementDecl, ElementId, Schemas, Scope,
    TypeDefinition, TypeId, ValueConstraint,
};
use crate::names::QName;
use std::fmt;

/// A component id that resolves to a navigable reference.
///
/// This is what makes [`Schemas::get`] work for every kind of id without a
/// method per kind.
pub trait Component: Copy {
    /// The reference type this id resolves to.
    type Ref<'s>;
    fn at(self, schemas: &Schemas) -> Self::Ref<'_>;
}

/// Renders `{ns}local`, or the local name alone when there is no namespace.
fn clark(schemas: &Schemas, q: QName) -> String {
    schemas.display_name(q)
}

/// The `default` or `fixed` value of a constraint, when it is that kind.
fn default_of(c: Option<&ValueConstraint>) -> Option<&str> {
    match c {
        Some(ValueConstraint::Default(v)) => Some(v),
        _ => None,
    }
}

fn fixed_of(c: Option<&ValueConstraint>) -> Option<&str> {
    match c {
        Some(ValueConstraint::Fixed(v)) => Some(v),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

/// An element declaration, and the schema to read it against.
#[derive(Copy, Clone)]
pub struct ElementRef<'s> {
    schemas: &'s Schemas,
    id: ElementId,
}

impl Component for ElementId {
    type Ref<'s> = ElementRef<'s>;
    fn at(self, schemas: &Schemas) -> ElementRef<'_> {
        ElementRef { schemas, id: self }
    }
}

impl<'s> ElementRef<'s> {
    pub fn id(self) -> ElementId {
        self.id
    }

    /// The schema this reference reads against.
    pub fn schemas(self) -> &'s Schemas {
        self.schemas
    }

    /// The declaration itself, for anything this view does not cover.
    pub fn decl(self) -> &'s ElementDecl {
        &self.schemas[self.id]
    }

    pub fn name(self) -> QName {
        self.decl().name
    }

    pub fn local_name(self) -> &'s str {
        self.schemas.local_of(self.name())
    }

    /// The namespace URI, absent when the name is unqualified.
    pub fn namespace(self) -> Option<&'s str> {
        self.schemas.namespace_of(self.name())
    }

    /// The name in James Clark notation, `{ns}local`.
    pub fn display_name(self) -> String {
        clark(self.schemas, self.name())
    }

    pub fn type_of(self) -> TypeRef<'s> {
        TypeRef {
            schemas: self.schemas,
            id: self.decl().type_id,
        }
    }

    pub fn is_nillable(self) -> bool {
        self.decl().nillable
    }

    pub fn is_abstract(self) -> bool {
        self.decl().is_abstract
    }

    /// Whether this is a global declaration, addressable by name.
    ///
    /// A local declaration is scoped to the type that contains it, so two
    /// with the same name in different types are different components.
    pub fn is_global(self) -> bool {
        self.decl().scope == Scope::Global
    }

    pub fn default(self) -> Option<&'s str> {
        default_of(self.decl().value_constraint.as_ref())
    }

    pub fn fixed(self) -> Option<&'s str> {
        fixed_of(self.decl().value_constraint.as_ref())
    }

    /// The elements that may appear inside this one — its type's children.
    pub fn children(self) -> impl Iterator<Item = ChildRef<'s>> {
        self.type_of().children()
    }

    /// The child of that local name, if there is one.
    pub fn child(self, local: &str) -> Option<ChildRef<'s>> {
        self.type_of().child(local)
    }

    /// The attributes this element may carry, with how it may carry them.
    pub fn attributes(self) -> impl Iterator<Item = AttributeUseRef<'s>> {
        self.type_of().attributes()
    }

    /// Whether a sequence of child names may appear inside this element.
    ///
    /// The same as `element.type_of().accepts(..)`, without the hop.
    pub fn accepts(self, names: impl IntoIterator<Item = QName>) -> bool {
        self.type_of().accepts(names)
    }

    /// Every element that may stand in for this one, this one included when
    /// it is not abstract.
    ///
    /// The names that may actually appear where this element is permitted:
    /// transitive, and with `block` applied, so this agrees with what the
    /// compiled content model admits. [`Schemas::substitution_group`] is the
    /// membership question, which is a different one.
    pub fn substitutes(self) -> impl Iterator<Item = ElementRef<'s>> {
        let schemas = self.schemas;
        schemas
            .permitted_substitutes(self.id)
            .into_iter()
            .map(move |id| ElementRef { schemas, id })
    }

    /// Every element in this one's substitution group, whether or not `block`
    /// permits it to stand in. See [`Schemas::substitution_group`].
    pub fn substitution_group(self) -> impl Iterator<Item = ElementRef<'s>> {
        let schemas = self.schemas;
        schemas
            .substitution_group(self.id)
            .into_iter()
            .map(move |id| ElementRef { schemas, id })
    }
}

impl fmt::Debug for ElementRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ElementRef({})", self.display_name())
    }
}

impl PartialEq for ElementRef<'_> {
    /// Two references are the same declaration when they carry the same id
    /// *and* read against the same schema; an id from one schema means
    /// nothing in another.
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.schemas, other.schemas) && self.id == other.id
    }
}

impl Eq for ElementRef<'_> {}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A type definition, and the schema to read it against.
#[derive(Copy, Clone)]
pub struct TypeRef<'s> {
    schemas: &'s Schemas,
    id: TypeId,
}

impl Component for TypeId {
    type Ref<'s> = TypeRef<'s>;
    fn at(self, schemas: &Schemas) -> TypeRef<'_> {
        TypeRef { schemas, id: self }
    }
}

impl<'s> TypeRef<'s> {
    pub fn id(self) -> TypeId {
        self.id
    }

    pub fn schemas(self) -> &'s Schemas {
        self.schemas
    }

    /// The definition itself, for anything this view does not cover.
    pub fn definition(self) -> &'s TypeDefinition {
        &self.schemas[self.id]
    }

    /// The name, absent for a type declared inline.
    pub fn name(self) -> Option<QName> {
        self.definition().name()
    }

    pub fn local_name(self) -> Option<&'s str> {
        self.name().map(|q| self.schemas.local_of(q))
    }

    pub fn namespace(self) -> Option<&'s str> {
        self.name().and_then(|q| self.schemas.namespace_of(q))
    }

    /// The name in Clark notation, or `(anonymous)` for an inline type.
    pub fn display_name(self) -> String {
        match self.name() {
            Some(q) => clark(self.schemas, q),
            None => "(anonymous)".to_string(),
        }
    }

    pub fn is_simple(self) -> bool {
        self.definition().is_simple()
    }

    pub fn is_complex(self) -> bool {
        !self.is_simple()
    }

    /// The type this one is derived from.
    ///
    /// Absent at the top of the hierarchy, where `xs:anyType` is its own
    /// base — a fixed point rather than a parent worth returning.
    pub fn base(self) -> Option<TypeRef<'s>> {
        let base = match self.definition() {
            TypeDefinition::Simple(t) => t.base,
            TypeDefinition::Complex(t) => t.base,
        };
        (base != self.id && self.schemas.get_type(base).is_some()).then_some(TypeRef {
            schemas: self.schemas,
            id: base,
        })
    }

    /// What this type may contain. `Empty` for every simple type.
    pub fn content(self) -> ContentType {
        self.definition()
            .as_complex()
            .map_or(ContentType::Empty, |c| c.content)
    }

    /// Whether character data may be interleaved with the children.
    pub fn is_mixed(self) -> bool {
        matches!(self.content(), ContentType::Mixed(_))
    }

    /// Every element that may appear directly inside this type, with
    /// substitution groups expanded and its occurrence resolved.
    pub fn children(self) -> impl Iterator<Item = ChildRef<'s>> {
        let schemas = self.schemas;
        schemas
            .children(self.id)
            .into_iter()
            .map(move |child| ChildRef { schemas, child })
    }

    /// The child of that local name, if there is one.
    ///
    /// Local, not qualified, because a child is almost always in its parent's
    /// namespace; use [`Self::children`] and match on [`ChildRef::name`] when
    /// it is not.
    pub fn child(self, local: &str) -> Option<ChildRef<'s>> {
        self.children().find(|c| c.local_name() == local)
    }

    /// The attributes this type declares, inherited ones included.
    pub fn attributes(self) -> impl Iterator<Item = AttributeUseRef<'s>> {
        let schemas = self.schemas;
        let uses: &'s [AttributeUse] = self
            .definition()
            .as_complex()
            .map_or(&[], |c| &c.attribute_uses);
        uses.iter()
            .map(move |use_| AttributeUseRef { schemas, use_ })
    }

    /// The attribute of that local name, if there is one.
    pub fn attribute(self, local: &str) -> Option<AttributeUseRef<'s>> {
        self.attributes().find(|a| a.local_name() == local)
    }

    /// Whether a sequence of child names satisfies this type's content
    /// model.
    ///
    /// The one-shot form of [`Schemas::match_content`], for when the answer
    /// wanted is yes or no rather than where a sequence went wrong. A simple
    /// type accepts nothing, not even the empty sequence: it has no content
    /// model to satisfy.
    ///
    /// ```no_run
    /// # let schemas = xsdkit::SchemaSetBuilder::new().file("report.xsd").compile().into_result().unwrap();
    /// # let report = schemas.element(Some("urn:example"), "report").unwrap();
    /// let title = schemas.qname(Some("urn:example"), "title").unwrap();
    /// assert!(report.type_of().accepts([title]));
    /// ```
    pub fn accepts(self, names: impl IntoIterator<Item = QName>) -> bool {
        let Some(mut m) = self.schemas.match_content(self.id) else {
            return false;
        };
        names.into_iter().all(|q| m.step(q)) && m.accepts_end()
    }

    /// Atomic, list or union. Absent for a complex type.
    pub fn variety(self) -> Option<Variety> {
        self.definition().as_simple().map(|t| t.variety)
    }

    /// The facets in force, after composing the whole restriction chain.
    pub fn facets(self) -> Option<&'s FacetSet> {
        self.definition().as_simple().map(|t| &t.facets)
    }

    /// The item type of a list.
    pub fn item_type(self) -> Option<TypeRef<'s>> {
        let id = self.definition().as_simple()?.item_type?;
        Some(TypeRef {
            schemas: self.schemas,
            id,
        })
    }

    /// The member types of a union, in the order they are tried.
    pub fn member_types(self) -> impl Iterator<Item = TypeRef<'s>> {
        let schemas = self.schemas;
        let members: &'s [TypeId] = self
            .definition()
            .as_simple()
            .map_or(&[], |t| &t.member_types);
        members.iter().map(move |&id| TypeRef { schemas, id })
    }
}

impl fmt::Debug for TypeRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypeRef({})", self.display_name())
    }
}

impl PartialEq for TypeRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.schemas, other.schemas) && self.id == other.id
    }
}

impl Eq for TypeRef<'_> {}

// ---------------------------------------------------------------------------
// Children
// ---------------------------------------------------------------------------

/// An element as a child of one particular type.
///
/// The occurrence facts belong to the pair, not to the declaration: one
/// global element may be used by several types under different bounds.
#[derive(Copy, Clone)]
pub struct ChildRef<'s> {
    schemas: &'s Schemas,
    child: Child,
}

impl<'s> ChildRef<'s> {
    pub fn element(self) -> ElementRef<'s> {
        ElementRef {
            schemas: self.schemas,
            id: self.child.element,
        }
    }

    /// Whether it may appear more than once — the table-versus-column
    /// question.
    pub fn repeats(self) -> bool {
        self.child.repeats
    }

    /// Whether some valid content leaves it out.
    pub fn optional(self) -> bool {
        self.child.optional
    }

    pub fn id(self) -> ElementId {
        self.child.element
    }

    pub fn name(self) -> QName {
        self.element().name()
    }

    pub fn local_name(self) -> &'s str {
        self.element().local_name()
    }

    pub fn namespace(self) -> Option<&'s str> {
        self.element().namespace()
    }

    pub fn display_name(self) -> String {
        self.element().display_name()
    }

    pub fn type_of(self) -> TypeRef<'s> {
        self.element().type_of()
    }

    /// The elements that may appear inside this child.
    pub fn children(self) -> impl Iterator<Item = ChildRef<'s>> {
        self.element().children()
    }
}

impl fmt::Debug for ChildRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ChildRef({}{}{})",
            self.display_name(),
            if self.repeats() { "+" } else { "" },
            if self.optional() { "?" } else { "" },
        )
    }
}

// ---------------------------------------------------------------------------
// Attributes
// ---------------------------------------------------------------------------

/// An attribute declaration, and the schema to read it against.
#[derive(Copy, Clone)]
pub struct AttributeRef<'s> {
    schemas: &'s Schemas,
    id: AttributeId,
}

impl Component for AttributeId {
    type Ref<'s> = AttributeRef<'s>;
    fn at(self, schemas: &Schemas) -> AttributeRef<'_> {
        AttributeRef { schemas, id: self }
    }
}

impl<'s> AttributeRef<'s> {
    pub fn id(self) -> AttributeId {
        self.id
    }

    pub fn schemas(self) -> &'s Schemas {
        self.schemas
    }

    pub fn decl(self) -> &'s AttributeDecl {
        &self.schemas[self.id]
    }

    pub fn name(self) -> QName {
        self.decl().name
    }

    pub fn local_name(self) -> &'s str {
        self.schemas.local_of(self.name())
    }

    pub fn namespace(self) -> Option<&'s str> {
        self.schemas.namespace_of(self.name())
    }

    pub fn display_name(self) -> String {
        clark(self.schemas, self.name())
    }

    pub fn type_of(self) -> TypeRef<'s> {
        TypeRef {
            schemas: self.schemas,
            id: self.decl().type_id,
        }
    }

    pub fn is_global(self) -> bool {
        self.decl().scope == Scope::Global
    }

    pub fn default(self) -> Option<&'s str> {
        default_of(self.decl().value_constraint.as_ref())
    }

    pub fn fixed(self) -> Option<&'s str> {
        fixed_of(self.decl().value_constraint.as_ref())
    }
}

impl fmt::Debug for AttributeRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AttributeRef({})", self.display_name())
    }
}

impl PartialEq for AttributeRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.schemas, other.schemas) && self.id == other.id
    }
}

impl Eq for AttributeRef<'_> {}

/// An attribute as used by one particular type.
///
/// Whether it is required, and any `default` or `fixed` the use overrides,
/// belong to the use rather than to the declaration — the same reason
/// [`ChildRef`] exists.
#[derive(Copy, Clone)]
pub struct AttributeUseRef<'s> {
    schemas: &'s Schemas,
    use_: &'s AttributeUse,
}

impl<'s> AttributeUseRef<'s> {
    pub fn attribute(self) -> AttributeRef<'s> {
        AttributeRef {
            schemas: self.schemas,
            id: self.use_.attribute,
        }
    }

    /// The use itself, for anything this view does not cover.
    pub fn use_(self) -> &'s AttributeUse {
        self.use_
    }

    pub fn is_required(self) -> bool {
        self.use_.is_required()
    }

    pub fn is_prohibited(self) -> bool {
        self.use_.kind == crate::model::AttributeUseKind::Prohibited
    }

    pub fn id(self) -> AttributeId {
        self.use_.attribute
    }

    pub fn name(self) -> QName {
        self.attribute().name()
    }

    pub fn local_name(self) -> &'s str {
        self.attribute().local_name()
    }

    pub fn namespace(self) -> Option<&'s str> {
        self.attribute().namespace()
    }

    pub fn display_name(self) -> String {
        self.attribute().display_name()
    }

    pub fn type_of(self) -> TypeRef<'s> {
        self.attribute().type_of()
    }

    /// The `default` in force here: the use's own if it states one, otherwise
    /// the declaration's.
    pub fn default(self) -> Option<&'s str> {
        match self.use_.value_constraint.as_ref() {
            Some(c) => default_of(Some(c)),
            None => self.attribute().default(),
        }
    }

    /// The `fixed` in force here, on the same rule as [`Self::default`].
    pub fn fixed(self) -> Option<&'s str> {
        match self.use_.value_constraint.as_ref() {
            Some(c) => fixed_of(Some(c)),
            None => self.attribute().fixed(),
        }
    }
}

impl fmt::Debug for AttributeUseRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "AttributeUseRef(@{}{})",
            self.display_name(),
            if self.is_required() { "" } else { "?" },
        )
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

impl Schemas {
    /// Resolves any component id to a navigable reference.
    ///
    /// ```no_run
    /// # let schemas = xsdkit::SchemaSetBuilder::new().file("s.xsd").compile().into_result().unwrap();
    /// # let id = schemas.element_id(None, "e").unwrap();
    /// let element = schemas.get(id);
    /// ```
    pub fn get<C: Component>(&self, id: C) -> C::Ref<'_> {
        id.at(self)
    }

    /// Looks up a global element declaration.
    ///
    /// ```no_run
    /// # let schemas = xsdkit::SchemaSetBuilder::new().file("report.xsd").compile().into_result().unwrap();
    /// let report = schemas.element(Some("urn:example"), "report").unwrap();
    /// ```
    pub fn element(&self, ns: Option<&str>, local: &str) -> Option<ElementRef<'_>> {
        Some(self.get(self.element_id(ns, local)?))
    }

    /// Looks up a global type definition.
    pub fn type_(&self, ns: Option<&str>, local: &str) -> Option<TypeRef<'_>> {
        Some(self.get(self.type_id(ns, local)?))
    }

    /// Looks up a global attribute declaration.
    pub fn attribute(&self, ns: Option<&str>, local: &str) -> Option<AttributeRef<'_>> {
        Some(self.get(self.attribute_id(ns, local)?))
    }

    /// Every global element declaration.
    pub fn global_elements(&self) -> impl Iterator<Item = ElementRef<'_>> {
        self.globals()
            .elements
            .values()
            .map(move |&id| ElementRef { schemas: self, id })
    }

    /// Every global type definition, the built-ins included.
    pub fn global_types(&self) -> impl Iterator<Item = TypeRef<'_>> {
        self.globals()
            .types
            .values()
            .map(move |&id| TypeRef { schemas: self, id })
    }

    /// Every global attribute declaration.
    pub fn global_attributes(&self) -> impl Iterator<Item = AttributeRef<'_>> {
        self.globals()
            .attributes
            .values()
            .map(move |&id| AttributeRef { schemas: self, id })
    }
}

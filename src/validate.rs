//! Validating values against the schema's own simple types.
//!
//! [`crate::values`] handles the 50 built-ins. This module handles what a
//! schema actually declares: a type five restrictions deep, a list of a union
//! of two restrictions, and so on. Three things have to be resolved before a
//! value can be checked at all.
//!
//! **Facets compose up the chain.** A type stores only the facets *it*
//! declares; the set in force is the fold from `xs:anySimpleType` down.
//!
//! **The nearest *built-in* ancestor governs parsing, not the primitive.**
//! `xs:int`'s primitive is `xs:decimal`, so parsing against the primitive
//! yields a decimal and drops every integer bound — `xs:byte` would accept
//! 999. The same distinction fixes `whiteSpace`: a type restricting
//! `xs:token` collapses, while its primitive `xs:string` preserves.
//!
//! **A restriction inherits its base's variety.** Restricting a list type
//! yields a list, not an atomic value, so the variety is taken from the
//! nearest ancestor that declares one.
//!
//! **A union's member order is load-bearing.** Members are tried in
//! declaration order and the first that accepts the value wins, which decides
//! what type the value *has*, not merely whether it is valid.

use crate::datatypes::{Builtin, FacetSet, Variety, WhiteSpace};
use crate::model::{Schemas, TypeDefinition, TypeId};
use crate::regex::{PatternError, PatternStep, Patterns, StepError};
use crate::values::{self, FacetViolation, Value, ValueError};
use std::fmt;

/// Why a value is not valid against a type.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ValidationError {
    /// The lexical form is not in the type's value space at all.
    Lexical(ValueError),
    /// The value parsed, but a facet rejects it.
    Facet(FacetViolation),
    /// No member type of a union accepted the value.
    NoUnionMember { tried: usize },
    /// The type is complex, so it has no value space.
    NotSimple,
    /// A `QName` value needs the document's namespace bindings, which a
    /// standalone value check does not have.
    NeedsNamespaceContext,
    /// The type refers to itself, directly or through its item or member
    /// types, so checking a value against it would not terminate.
    ///
    /// Such a schema is rejected with `CircularDefinition`, but
    /// `Conformance::Lax` keeps the model, and a validator must not blow the
    /// stack on it.
    CircularType,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationError::Lexical(e) => write!(f, "{e}"),
            ValidationError::Facet(v) => write!(f, "{v}"),
            ValidationError::NoUnionMember { tried } => {
                write!(f, "no member type accepts this value ({tried} tried)")
            }
            ValidationError::NotSimple => f.write_str("a complex type has no value space"),
            ValidationError::CircularType => {
                f.write_str("the type refers to itself, so it admits no value")
            }
            ValidationError::NeedsNamespaceContext => {
                f.write_str("QName values must be resolved against the document's namespaces")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// One simple type, prepared for repeated value checks.
#[derive(Clone, Debug)]
struct Prepared {
    variety: Variety,
    /// The nearest built-in ancestor, which is what values are parsed
    /// against. **Not** the primitive: `xs:int`'s primitive is `xs:decimal`,
    /// and parsing an `xs:int` as a decimal loses its bounds.
    builtin: Option<Builtin>,
    /// The whiteSpace in force, from the nearest built-in ancestor unless a
    /// restriction overrides it.
    white_space: WhiteSpace,
    facets: FacetSet,
    patterns: Patterns,
    item: Option<TypeId>,
    members: Vec<TypeId>,
}

/// Why a pattern a schema declares is not enforced as it was written.
#[derive(Clone, Debug)]
pub(crate) enum PatternIssue {
    /// Not an XSD regular expression, so the schema is in error.
    Invalid(PatternError),
    /// A valid XSD pattern the `regex` crate will not compile. The step is
    /// not enforced.
    Unsupported(PatternError),
    /// A `\p{IsX}` naming a block this build does not know, which XSD 1.1
    /// says matches every character.
    UnknownBlock { pattern: String, block: String },
}

/// Every simple type in a schema, prepared for value checks: facets composed
/// down each chain, and patterns compiled.
///
/// Built once per [`Schemas`] and kept on it. Every [`ValueValidator`] used
/// to build its own, and every validation builds a validator, so validating a
/// one-element document compiled every pattern in the schema — 20 ms a call
/// against two thousand of them.
#[derive(Clone, Debug, Default)]
pub(crate) struct PreparedTypes {
    types: Vec<Option<Prepared>>,
    /// The steps that could not be compiled, for
    /// [`ValueValidator::pattern_errors`].
    pattern_errors: Vec<(TypeId, PatternError)>,
    /// Everything about a pattern worth a diagnostic, keyed by the type that
    /// declared it rather than by every type that inherits it, so one bad
    /// pattern is reported once.
    pub(crate) pattern_issues: Vec<(TypeId, PatternIssue)>,
}

impl PreparedTypes {
    pub(crate) fn build(schemas: &Schemas) -> Self {
        let n = schemas.component_counts().types;

        // Each restriction step compiled once, on the type that wrote it. A
        // type's patterns are its ancestors' steps and then its own, so
        // compiling the composed set for every type, as this used to, compiled
        // a base's pattern again for each type below it — and one step that
        // would not compile dropped the others with it.
        let mut own: Vec<Vec<PatternStep>> = vec![Vec::new(); n];
        let mut pattern_errors = Vec::new();
        let mut pattern_issues = Vec::new();
        for (id, def) in schemas.iter_types() {
            let Some(simple) = def.as_simple() else {
                continue;
            };
            for step in &simple.facets.patterns {
                match PatternStep::compile_noting(step) {
                    Ok((compiled, unknown)) => {
                        own[id.index()].push(compiled);
                        for block in unknown {
                            pattern_issues.push((
                                id,
                                PatternIssue::UnknownBlock {
                                    pattern: step.join("|"),
                                    block,
                                },
                            ));
                        }
                    }
                    Err(e) => {
                        let (issue, error) = match e {
                            StepError::Invalid(e) => (PatternIssue::Invalid(e.clone()), e),
                            StepError::Unsupported(e) => (PatternIssue::Unsupported(e.clone()), e),
                        };
                        pattern_errors.push((id, error));
                        pattern_issues.push((id, issue));
                    }
                }
            }
        }

        let types = (0..n)
            .map(|i| {
                let id = TypeId::from_index(i);
                schemas[id].as_simple()?;
                let facets = effective_facets(schemas, id);
                let builtin = nearest_builtin(schemas, id);
                let white_space = facets.effective_white_space(
                    builtin.map_or(WhiteSpace::Preserve, Builtin::white_space),
                );
                let (variety, item, members) = effective_variety(schemas, id);
                // Root first, as `effective_facets` composes them.
                let patterns = simple_chain(schemas, id)
                    .into_iter()
                    .rev()
                    .flat_map(|t| own[t.index()].iter().cloned())
                    .collect();
                Some(Prepared {
                    variety,
                    builtin,
                    white_space,
                    facets,
                    patterns,
                    item,
                    members,
                })
            })
            .collect();

        Self {
            types,
            pattern_errors,
            pattern_issues,
        }
    }
}

/// Validates values against a compiled schema's simple types.
///
/// Cheap to make: the facets and patterns it checks against were prepared
/// once, when the schema was compiled, and are shared by every validator.
#[derive(Debug)]
pub struct ValueValidator<'a> {
    schemas: &'a Schemas,
    prepared: &'a PreparedTypes,
}

impl<'a> ValueValidator<'a> {
    /// A validator over every simple type in the schema.
    pub fn new(schemas: &'a Schemas) -> Self {
        Self {
            schemas,
            prepared: schemas.prepared_types(),
        }
    }

    /// Patterns in the schema that could not be compiled, each with the type
    /// that declared it.
    ///
    /// Non-empty means some type is more permissive than it declares. The
    /// same failures are diagnostics when the schema is compiled, so this is
    /// for a caller holding a `Schemas` whose diagnostics are gone — one read
    /// back from a cache, say.
    pub fn pattern_errors(&self) -> &[(TypeId, PatternError)] {
        &self.prepared.pattern_errors
    }

    /// Validates a lexical form against a simple type, returning its value.
    pub fn validate(&self, ty: TypeId, lexical: &str) -> Result<Value, ValidationError> {
        self.validate_with(ty, lexical, &values::NoNamespaces)
    }

    /// [`Self::validate`], with the namespace bindings in scope where the
    /// value was written.
    ///
    /// `xs:QName` and `xs:NOTATION` are the only datatypes whose value is not
    /// a function of the lexical form alone: `a:x` names whatever `a` is bound
    /// to *there*. Without bindings an unprefixed name is still a value — it
    /// is in no namespace — and a prefixed one cannot be resolved.
    ///
    /// The XSD version is not a parameter here the way it is on
    /// [`crate::values::parse_with`]: the schema this validator was built
    /// from already settled which language applies.
    pub fn validate_with(
        &self,
        ty: TypeId,
        lexical: &str,
        ns: &dyn values::Namespaces,
    ) -> Result<Value, ValidationError> {
        self.validate_within(ty, lexical, 0, ns)
    }

    /// The recursion behind [`Self::validate`], carrying how deep it is.
    ///
    /// A list's items and a union's members are themselves types, so checking
    /// a value descends. A type that reaches itself that way is rejected when
    /// the schema is compiled — but `Conformance::Lax` keeps the model anyway,
    /// and a validator handed untrusted input must not blow the stack. The
    /// limit is far above any real schema: nesting lists and unions sixty-four
    /// deep is not something a person writes.
    fn validate_within(
        &self,
        ty: TypeId,
        lexical: &str,
        depth: u32,
        ns: &dyn values::Namespaces,
    ) -> Result<Value, ValidationError> {
        if depth > 64 {
            return Err(ValidationError::CircularType);
        }
        let Some(p) = self.prepared.types.get(ty.index()).and_then(Option::as_ref) else {
            return Err(ValidationError::NotSimple);
        };

        match p.variety {
            Variety::Atomic => self.atomic(p, lexical, ns),
            Variety::List => self.list(p, lexical, depth, ns),
            Variety::Union => self.union(p, lexical, depth, ns),
        }
    }

    fn atomic(
        &self,
        p: &Prepared,
        lexical: &str,
        ns: &dyn values::Namespaces,
    ) -> Result<Value, ValidationError> {
        let normalized = p.white_space.normalize(lexical);

        // Patterns constrain the lexical form, so they run on the normalised
        // string before it becomes a value.
        values::check_patterns(normalized.as_ref(), &p.patterns).map_err(ValidationError::Facet)?;

        let builtin = p.builtin.unwrap_or(Builtin::String);
        // Already normalised, and the built-in's own whiteSpace is idempotent
        // over its output, so re-applying it inside `parse` changes nothing.
        let value = values::parse_with(
            builtin,
            normalized.as_ref(),
            &values::ParseContext::new()
                .version(self.schemas.xsd_version)
                .namespaces(ns),
        )
        .map_err(ValidationError::Lexical)?;
        // Facet literals are parsed against the same built-in, so bounds and
        // enumerations compare like with like.
        values::check_facets(&value, &p.facets, builtin).map_err(ValidationError::Facet)?;
        Ok(value)
    }

    fn list(
        &self,
        p: &Prepared,
        lexical: &str,
        depth: u32,
        ns: &dyn values::Namespaces,
    ) -> Result<Value, ValidationError> {
        let normalized = p.white_space.normalize(lexical);
        values::check_patterns(normalized.as_ref(), &p.patterns).map_err(ValidationError::Facet)?;

        let Some(item) = p.item else {
            return Err(ValidationError::NotSimple);
        };
        let items = normalized
            .split_whitespace()
            .map(|tok| self.validate_within(item, tok, depth + 1, ns))
            .collect::<Result<Vec<_>, _>>()?;

        // An enumeration on a list names whole lists, so each literal has to
        // be parsed as one and compared item by item. `check_facets` cannot:
        // it has no item type, and comparing a list against a string rejects
        // every value.
        if let Some(allowed) = &p.facets.enumeration {
            // The literals came from the schema, so their prefixes resolve
            // there; only `items` above came from the document.
            let schema_ns = &p.facets.namespaces;
            let matched = allowed.iter().any(|lex| {
                lex.split_whitespace()
                    .map(|tok| self.validate_within(item, tok, depth + 1, schema_ns))
                    .collect::<Result<Vec<_>, _>>()
                    .is_ok_and(|want| want == items)
            });
            if !matched {
                return Err(ValidationError::Facet(FacetViolation {
                    facet: "enumeration",
                    message: format!(
                        "`{}` is not one of the {} permitted lists",
                        normalized,
                        allowed.len()
                    ),
                }));
            }
        }

        let value = Value::List(items);
        // A list's own length facets count items, which `facet_length` knows.
        values::check_facets(&value, &p.facets, Builtin::String).map_err(ValidationError::Facet)?;
        Ok(value)
    }

    fn union(
        &self,
        p: &Prepared,
        lexical: &str,
        depth: u32,
        ns: &dyn values::Namespaces,
    ) -> Result<Value, ValidationError> {
        // Declaration order decides which member the value belongs to, not
        // just whether it is valid at all.
        for &member in &p.members {
            if let Ok(v) = self.validate_within(member, lexical, depth + 1, ns) {
                // The union's own facets still apply on top of the member's.
                if !p.patterns.is_empty() {
                    let normalized = p.white_space.normalize(lexical);
                    values::check_patterns(normalized.as_ref(), &p.patterns)
                        .map_err(ValidationError::Facet)?;
                }
                if let Some(allowed) = &p.facets.enumeration {
                    let schema_ns = &p.facets.namespaces;
                    let ok = allowed.iter().any(|lex| {
                        p.members.iter().any(|m| {
                            self.validate_within(*m, lex, depth + 1, schema_ns)
                                .map(|x| x == v)
                                .unwrap_or(false)
                        })
                    });
                    if !ok {
                        return Err(ValidationError::Facet(FacetViolation {
                            facet: "enumeration",
                            message: format!("`{v}` is not one of the permitted values"),
                        }));
                    }
                }
                return Ok(v);
            }
        }
        Err(ValidationError::NoUnionMember {
            tried: p.members.len(),
        })
    }

    /// Which member type of a union accepted the value.
    ///
    /// Useful because a union's *actual* type is part of the PSVI, not merely
    /// whether the value was valid.
    pub fn union_member(&self, ty: TypeId, lexical: &str) -> Option<TypeId> {
        let p = self.prepared.types.get(ty.index())?.as_ref()?;
        if p.variety != Variety::Union {
            return None;
        }
        p.members
            .iter()
            .copied()
            .find(|m| self.validate(*m, lexical).is_ok())
    }

    /// The effective whiteSpace of a simple type.
    pub fn white_space(&self, ty: TypeId) -> Option<WhiteSpace> {
        Some(self.prepared.types.get(ty.index())?.as_ref()?.white_space)
    }

    /// The facets in force on a simple type, composed up its whole chain.
    pub fn effective_facets(&self, ty: TypeId) -> Option<&FacetSet> {
        Some(&self.prepared.types.get(ty.index())?.as_ref()?.facets)
    }

    pub fn schemas(&self) -> &'a Schemas {
        self.schemas
    }
}

/// Folds a simple type's whole restriction chain into one facet set.
///
/// A type stores only the facets it declares itself, so the set in force is
/// the composition from the root down — which is also what keeps patterns
/// ANDing across steps.
pub(crate) fn effective_facets(schemas: &Schemas, id: TypeId) -> FacetSet {
    let chain = simple_chain(schemas, id);
    let mut out = FacetSet::new();
    for ty in chain.into_iter().rev() {
        if let Some(s) = schemas[ty].as_simple() {
            out = compose(&out, &s.facets);
        }
    }
    out
}

/// Applies one step's declared facets onto the inherited set.
///
/// `FacetSet::restrict` takes a facet list; a stored `FacetSet` is already
/// composed, so its parts are merged directly — patterns appended as their
/// own ANDed steps, everything else overriding.
fn compose(base: &FacetSet, step: &FacetSet) -> FacetSet {
    let mut out = base.clone();
    for group in &step.patterns {
        out.patterns.push(group.clone());
    }
    if step.enumeration.is_some() {
        out.enumeration = step.enumeration.clone();
        // The bindings resolve *these* literals, so they travel with the
        // enumeration that replaces the inherited one rather than
        // accumulating down the chain.
        out.namespaces = step.namespaces.clone();
    }
    // Every `Option` field of `FacetSet` where a step simply replaces what it
    // inherited. **A new field belongs here**: the list is written out, so one
    // that is forgotten silently stops composing, and the facet then applies
    // only on the step that declared it.
    macro_rules! take {
        ($($f:ident),*) => { $( if step.$f.is_some() { out.$f = step.$f.clone(); } )* };
    }
    take!(
        length,
        min_length,
        max_length,
        white_space,
        total_digits,
        fraction_digits,
        explicit_timezone,
        min_scale,
        max_scale
    );
    // A bound of one kind displaces the other, as it does within a step.
    if step.min_inclusive.is_some() {
        out.min_inclusive = step.min_inclusive.clone();
        out.min_exclusive = None;
    }
    if step.min_exclusive.is_some() {
        out.min_exclusive = step.min_exclusive.clone();
        out.min_inclusive = None;
    }
    if step.max_inclusive.is_some() {
        out.max_inclusive = step.max_inclusive.clone();
        out.max_exclusive = None;
    }
    if step.max_exclusive.is_some() {
        out.max_exclusive = step.max_exclusive.clone();
        out.max_inclusive = None;
    }
    out.assertions.extend(step.assertions.iter().cloned());
    out
}

/// A simple type's base chain, itself first, stopping at the first
/// non-simple type.
pub(crate) fn simple_chain(schemas: &Schemas, id: TypeId) -> Vec<TypeId> {
    let mut out = Vec::new();
    let mut cur = id;
    let mut guard = 0usize;
    loop {
        if !matches!(schemas[cur], TypeDefinition::Simple(_)) {
            break;
        }
        out.push(cur);
        let base = schemas[cur].base();
        guard += 1;
        if base == cur || base.is_placeholder() || guard > schemas.component_counts().types {
            break;
        }
        cur = base;
    }
    out
}

/// A simple type's effective variety, with the list item or union members
/// that go with it.
///
/// A restriction inherits its base's variety, so restricting a list yields a
/// list. The variety is therefore the nearest one actually declared.
pub(crate) fn effective_variety(
    schemas: &Schemas,
    id: TypeId,
) -> (Variety, Option<TypeId>, Vec<TypeId>) {
    for ty in simple_chain(schemas, id) {
        let Some(s) = schemas[ty].as_simple() else {
            continue;
        };
        match s.variety {
            Variety::List => return (Variety::List, s.item_type, Vec::new()),
            Variety::Union if !s.member_types.is_empty() => {
                return (Variety::Union, None, s.member_types.clone());
            }
            _ => {}
        }
    }
    (Variety::Atomic, None, Vec::new())
}

/// The nearest ancestor that *is* a built-in.
///
/// This is what fixes `whiteSpace`, and it is not the same as the primitive:
/// a type restricting `xs:token` collapses, while its primitive `xs:string`
/// preserves.
pub(crate) fn nearest_builtin(schemas: &Schemas, id: TypeId) -> Option<Builtin> {
    simple_chain(schemas, id)
        .into_iter()
        .find_map(|t| schemas[t].as_simple().and_then(|s| s.builtin))
}

impl Schemas {
    /// A [`ValueValidator`] over this schema's simple types.
    ///
    /// Cheap: everything it checks against was prepared once and is kept on
    /// the schema.
    pub fn value_validator(&self) -> ValueValidator<'_> {
        ValueValidator::new(self)
    }

    /// Every simple type, prepared once.
    ///
    /// Compiling fills this in; a schema read back through serde has none,
    /// since a compiled regular expression cannot be serialized, and builds
    /// it the first time a value is checked.
    pub(crate) fn prepared_types(&self) -> &PreparedTypes {
        self.prepared_types
            .get_or_init(|| PreparedTypes::build(self))
    }
}

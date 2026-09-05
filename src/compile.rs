//! Turning loaded documents into a resolved component graph.
//!
//! Runs as ordered steps, each of which must finish before the next begins —
//! the discipline `xsdata`'s analyzer pipeline demonstrates, and the only
//! thing that keeps flattening logic legible:
//!
//! 1. `resolve_references` patches every placeholder id from the fixup list.
//! 2. `merge_attribute_groups` flattens `xs:attributeGroup` references.
//! 3. `merge_inherited_attributes` folds each type's base chain into its own
//!    attribute uses.
//! 4. `resolve_simple_content` points simple-content types at their base.
//! 5. `check_cycles` rejects derivation and group chains that loop.
//! 6. `build_substitution_closure` precomputes transitive substitution.
//! 6. [`crate::content`] compiles every content model and checks UPA. It runs
//!    against the finished `Schemas`, so it can expand substitution groups
//!    while building.
//! 7. [`crate::facets`] checks that each simple type's facets are facets that
//!    datatype admits, and that their values are legal.
//! 8. [`crate::declarations`] checks each declaration's `default` or `fixed`
//!    against the type it belongs to.
//! 9. [`crate::derivation`] checks that no type derives from a base whose
//!    `final` forbids it.

use crate::datatypes::Builtin;
use crate::diagnostics::{DiagCode, Diagnostic, Diagnostics, Severity, Span};
use crate::load::{AttrOwner, Conformance, Fixup, Loader};
use crate::model::*;
use crate::names::QName;
use fxhash::{FxHashMap, FxHashSet};

pub(crate) fn compile(mut loader: Loader<'_>, mode: Conformance) -> (Schemas, Diagnostics) {
    resolve_references(&mut loader, mode);
    merge_attribute_groups(&mut loader);
    merge_inherited_attributes(&mut loader);
    resolve_simple_content(&mut loader);
    check_cycles(&mut loader);
    check_structural_cycles(&mut loader);
    let substitution_closure = build_substitution_closure(&loader);
    let version = loader.version();

    let Loader {
        types,
        elements,
        attributes,
        particles,
        model_groups,
        attribute_groups,
        identity_constraints,
        notations,
        annotations,
        names,
        globals,
        builtins,
        diags,
        documents,
        ..
    } = loader;

    let mut schemas = Schemas {
        types,
        elements,
        attributes,
        particles,
        model_groups,
        attribute_groups,
        identity_constraints,
        notations,
        annotations,
        names,
        globals,
        builtins,
        substitution_closure,
        content_models: Vec::new(),
        documents,
        xsd_version: version,
    };

    // Step 6 needs the query API, so it runs on the assembled value.
    let (models, content_diags) = crate::content::build_all(&schemas, mode, version);
    schemas.content_models = models;
    let mut diags = diags;
    diags.extend(content_diags);
    // Steps 7 and 8 likewise: the facet rules need the base type resolved,
    // and the declaration rules need the composed facet set on top of that.
    diags.extend(crate::facets::check_all(&schemas));
    diags.extend(crate::declarations::check_all(&schemas));
    diags.extend(crate::derivation::check_all(&schemas));
    diags.extend(crate::restriction::check_all(&schemas));

    (schemas, diags)
}

// ---------------------------------------------------------------------------
// 1. Reference resolution
// ---------------------------------------------------------------------------

fn resolve_references(l: &mut Loader<'_>, mode: Conformance) {
    let fixups = std::mem::take(&mut l.fixups);
    let mut unresolved: Vec<(SymbolSpace, QName, Span)> = Vec::new();

    for f in fixups {
        match f {
            Fixup::ElementType {
                element,
                name,
                span,
            } => {
                match l.globals.types.get(&name).copied() {
                    Some(t) => l.elements.get_mut(element.0).type_id = t,
                    None => {
                        // Keep the graph traversable: an unresolved type
                        // becomes xs:anyType rather than a dangling id.
                        let any = l.builtins[&crate::datatypes::Builtin::AnyType];
                        l.elements.get_mut(element.0).type_id = any;
                        unresolved.push((SymbolSpace::Type, name, span));
                    }
                }
            }
            Fixup::ElementSubstGroup {
                element,
                name,
                span,
            } => match l.globals.elements.get(&name).copied() {
                Some(head) => l.elements.get_mut(element.0).substitution_group.push(head),
                None => unresolved.push((SymbolSpace::Element, name, span)),
            },
            Fixup::AttributeType {
                attribute,
                name,
                span,
            } => match l.globals.types.get(&name).copied() {
                Some(t) => l.attributes.get_mut(attribute.0).type_id = t,
                None => {
                    let any = l.builtins[&crate::datatypes::Builtin::AnySimpleType];
                    l.attributes.get_mut(attribute.0).type_id = any;
                    unresolved.push((SymbolSpace::Type, name, span));
                }
            },
            Fixup::SimpleBase { type_, name, span } => {
                match l.globals.types.get(&name).copied() {
                    Some(t) => {
                        // A simple type restricting a built-in inherits that
                        // built-in's primitive, which the facet engine needs.
                        let primitive = match l.types.get(t.0) {
                            TypeDefinition::Simple(b) => b.primitive.or(b.builtin),
                            TypeDefinition::Complex(_) => None,
                        };
                        if let TypeDefinition::Simple(s) = l.types.get_mut(type_.0) {
                            s.base = t;
                            if s.primitive.is_none() {
                                s.primitive = primitive;
                            }
                        }
                    }
                    None => unresolved.push((SymbolSpace::Type, name, span)),
                }
            }
            Fixup::SimpleItem { type_, name, span } => match l.globals.types.get(&name).copied() {
                Some(t) => {
                    if let TypeDefinition::Simple(s) = l.types.get_mut(type_.0) {
                        s.item_type = Some(t);
                    }
                }
                None => unresolved.push((SymbolSpace::Type, name, span)),
            },
            Fixup::SimpleMember {
                type_,
                index,
                name,
                span,
            } => match l.globals.types.get(&name).copied() {
                Some(t) => {
                    if let TypeDefinition::Simple(s) = l.types.get_mut(type_.0) {
                        if index < s.member_types.len() {
                            s.member_types[index] = t;
                        }
                    }
                }
                None => unresolved.push((SymbolSpace::Type, name, span)),
            },
            Fixup::ComplexBase { type_, name, span } => match l.globals.types.get(&name).copied() {
                Some(t) => {
                    if let TypeDefinition::Complex(c) = l.types.get_mut(type_.0) {
                        c.base = t;
                    }
                }
                None => unresolved.push((SymbolSpace::Type, name, span)),
            },
            Fixup::ParticleElementRef {
                particle,
                name,
                span,
            } => match l.globals.elements.get(&name).copied() {
                Some(e) => l.particles.get_mut(particle.0).term = Term::Element(e),
                None => unresolved.push((SymbolSpace::Element, name, span)),
            },
            Fixup::ParticleGroupRef {
                particle,
                name,
                span,
            } => match l.globals.model_groups.get(&name).copied() {
                Some(g) => l.particles.get_mut(particle.0).term = Term::GroupRef(g),
                None => unresolved.push((SymbolSpace::ModelGroup, name, span)),
            },
            Fixup::AttrUseRef {
                owner,
                index,
                name,
                span,
            } => match l.globals.attributes.get(&name).copied() {
                Some(a) => {
                    if let Some(uses) = attribute_uses_mut(l, owner) {
                        if index < uses.len() {
                            uses[index].attribute = a;
                        }
                    }
                }
                None => unresolved.push((SymbolSpace::Attribute, name, span)),
            },
            Fixup::AttrGroupRef {
                owner,
                index,
                name,
                span,
            } => match l.globals.attribute_groups.get(&name).copied() {
                Some(g) => {
                    if let Some(refs) = attribute_group_refs_mut(l, owner) {
                        if index < refs.len() {
                            refs[index] = g;
                        }
                    }
                }
                None => unresolved.push((SymbolSpace::AttributeGroup, name, span)),
            },
            Fixup::KeyRefRefer { idc, name, span } => {
                match l.globals.identity_constraints.get(&name).copied() {
                    Some(k) => l.identity_constraints.get_mut(idc.0).refer = Some(k),
                    None => unresolved.push((SymbolSpace::IdentityConstraint, name, span)),
                }
            }
            Fixup::ElementIdcRef {
                element,
                index,
                name,
                span,
            } => match l.globals.identity_constraints.get(&name).copied() {
                Some(k) => l.elements.get_mut(element.0).identity_constraints[index] = k,
                // The placeholder stays, and `prune_placeholders` drops it.
                None => unresolved.push((SymbolSpace::IdentityConstraint, name, span)),
            },
        }
    }

    for (space, name, span) in unresolved {
        let shown = l.names.display(name);
        let d = Diagnostic::error(
            DiagCode::UnresolvedReference,
            format!("no {} named `{shown}`", space.as_str()),
        )
        .at(span)
        .with_help("check the spelling, or add an xs:import for its namespace");
        l.diags.push(if mode == Conformance::Lax {
            Diagnostic {
                severity: Severity::Warning,
                ..d
            }
        } else {
            d
        });
    }

    // Drop uses and refs whose target never resolved, so no placeholder can
    // reach a `Schemas`.
    prune_placeholders(l);
}

fn attribute_uses_mut<'a>(
    l: &'a mut Loader<'_>,
    owner: AttrOwner,
) -> Option<&'a mut Vec<AttributeUse>> {
    match owner {
        AttrOwner::ComplexType(t) => match l.types.get_mut(t.0) {
            TypeDefinition::Complex(c) => Some(&mut c.attribute_uses),
            TypeDefinition::Simple(_) => None,
        },
        AttrOwner::AttributeGroup(g) => Some(&mut l.attribute_groups.get_mut(g.0).attribute_uses),
    }
}

fn attribute_group_refs_mut<'a>(
    l: &'a mut Loader<'_>,
    owner: AttrOwner,
) -> Option<&'a mut Vec<AttrGroupId>> {
    match owner {
        AttrOwner::ComplexType(t) => match l.types.get_mut(t.0) {
            TypeDefinition::Complex(c) => Some(&mut c.attribute_group_refs),
            TypeDefinition::Simple(_) => None,
        },
        AttrOwner::AttributeGroup(g) => {
            Some(&mut l.attribute_groups.get_mut(g.0).attribute_group_refs)
        }
    }
}

fn prune_placeholders(l: &mut Loader<'_>) {
    for i in 0..l.types.len() as u32 {
        if let TypeDefinition::Complex(c) = l.types.get_mut(i) {
            c.attribute_uses.retain(|u| !u.attribute.is_placeholder());
            c.attribute_group_refs.retain(|g| !g.is_placeholder());
        }
    }
    for i in 0..l.attribute_groups.len() as u32 {
        let g = l.attribute_groups.get_mut(i);
        g.attribute_uses.retain(|u| !u.attribute.is_placeholder());
        g.attribute_group_refs.retain(|r| !r.is_placeholder());
    }
    for i in 0..l.model_groups.len() as u32 {
        let particles = l.model_groups.get(i).group.particles.clone();
        let kept: Vec<_> = particles
            .into_iter()
            .filter(|p| !particle_is_dangling(l, *p))
            .collect();
        l.model_groups.get_mut(i).group.particles = kept;
    }
    for i in 0..l.particles.len() as u32 {
        if let Term::Group(g) = &l.particles.get(i).term {
            let particles = g.particles.clone();
            let kept: Vec<_> = particles
                .into_iter()
                .filter(|p| !particle_is_dangling(l, *p))
                .collect();
            if let Term::Group(g) = &mut l.particles.get_mut(i).term {
                g.particles = kept;
            }
        }
    }
    for i in 0..l.elements.len() as u32 {
        let e = l.elements.get_mut(i);
        if e.type_id.is_placeholder() {
            e.type_id = TypeId::from_index(0); // xs:anyType, installed first
        }
    }
    // A type reference that never resolved. Elements and attributes get their
    // own repair below; these are the four places a *type* can point at
    // another one, and none of them was patched — so `iter_types` handed a
    // caller a placeholder and anything that walked a derivation chain
    // panicked. Same shape of bug as the dangling particle below, and found
    // the same way.
    let any_type = l.builtins[&Builtin::AnyType];
    let any_simple_type = l.builtins[&Builtin::AnySimpleType];
    for i in 0..l.types.len() as u32 {
        match l.types.get_mut(i) {
            TypeDefinition::Simple(s) => {
                if s.base.is_placeholder() {
                    s.base = any_simple_type;
                }
                if s.item_type.is_some_and(|t| t.is_placeholder()) {
                    s.item_type = Some(any_simple_type);
                }
                for m in &mut s.member_types {
                    if m.is_placeholder() {
                        *m = any_simple_type;
                    }
                }
            }
            TypeDefinition::Complex(c) => {
                if c.base.is_placeholder() {
                    c.base = any_type;
                }
            }
        }
    }

    // An identity constraint reached by `ref` that never resolved. Unlike a
    // type there is no sensible substitute — the constraint simply is not
    // there — so the slot goes away rather than pointing at something wrong.
    for i in 0..l.elements.len() as u32 {
        l.elements
            .get_mut(i)
            .identity_constraints
            .retain(|c| !c.is_placeholder());
    }
    // Attributes need the same repair, and a different fallback: an attribute
    // may only carry a simple type, so xs:anyType would be a lie. Reached by a
    // `type` that is not even a well-formed QName — nothing binds, and the
    // declaration keeps its placeholder into `Schemas`. Found by fuzzing.
    let any_simple = l.builtins[&Builtin::AnySimpleType];
    for i in 0..l.attributes.len() as u32 {
        let a = l.attributes.get_mut(i);
        if a.type_id.is_placeholder() {
            a.type_id = any_simple;
        }
    }

    // A type whose *entire* content is a dangling reference. The loops above
    // prune dangling particles out of their containers, but a content
    // particle has no container to be pruned from — it hangs off
    // `ComplexType::content` directly, so it survived and reached `Schemas`
    // as a placeholder. Found by the W3C suite, on a schema whose chameleon
    // include could not be resolved.
    for i in 0..l.types.len() as u32 {
        let drop = match l.types.get(i) {
            TypeDefinition::Complex(c) => match c.content.particle() {
                Some(p) => particle_is_dangling(l, p),
                None => false,
            },
            TypeDefinition::Simple(_) => false,
        };
        if drop {
            if let TypeDefinition::Complex(c) = l.types.get_mut(i) {
                c.content = ContentType::Empty;
            }
        }
    }

    // Everything above unlinks dangling particles from whatever *contained*
    // them, but the particles themselves stay in the arena, still holding the
    // placeholder id. Nothing reachable points at them — and yet
    // `Schemas::iter_particles` walks the arena, not the reachable graph, so a
    // caller enumerating components still meets one and hands it straight back
    // to `child_particles`, which indexes the placeholder and panics.
    //
    // So neutralize the terms too. The invariant `Schemas` is supposed to
    // carry is that no placeholder survives compilation *anywhere*, not just
    // where a traversal from the roots happens to look. An empty sequence is
    // the inert particle: unreachable as it already was, and harmless when
    // enumerated. Found by fuzzing the loader.
    for i in 0..l.particles.len() as u32 {
        let dangling = match &l.particles.get(i).term {
            Term::Element(e) => e.is_placeholder(),
            Term::GroupRef(g) => g.is_placeholder(),
            _ => false,
        };
        if dangling {
            l.particles.get_mut(i).term = Term::Group(ModelGroup {
                compositor: Compositor::Sequence,
                particles: Vec::new(),
            });
        }
    }
}

fn particle_is_dangling(l: &Loader<'_>, p: ParticleId) -> bool {
    match &l.particles.get(p.0).term {
        Term::Element(e) => e.is_placeholder(),
        Term::GroupRef(g) => g.is_placeholder(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// 2. Attribute group flattening
// ---------------------------------------------------------------------------

/// Expands `xs:attributeGroup` references into direct attribute uses.
///
/// Groups may reference groups, so this resolves each group's own references
/// first, memoising the result and guarding against cycles.
fn merge_attribute_groups(l: &mut Loader<'_>) {
    let n = l.attribute_groups.len();
    let mut expanded: Vec<Option<Vec<AttributeUse>>> = vec![None; n];
    let mut in_progress = FxHashSet::default();

    for i in 0..n {
        expand_group(
            l,
            AttrGroupId::from_index(i),
            &mut expanded,
            &mut in_progress,
        );
    }

    for i in 0..l.types.len() as u32 {
        let refs = match l.types.get(i) {
            TypeDefinition::Complex(c) => c.attribute_group_refs.clone(),
            TypeDefinition::Simple(_) => continue,
        };
        if refs.is_empty() {
            continue;
        }
        let mut extra = Vec::new();
        for g in refs {
            if let Some(uses) = &expanded[g.index()] {
                extra.extend(uses.iter().cloned());
            }
        }
        if let TypeDefinition::Complex(c) = l.types.get_mut(i) {
            for u in extra {
                if !c.attribute_uses.iter().any(|e| e.attribute == u.attribute) {
                    c.attribute_uses.push(u);
                }
            }
        }
    }
}

fn expand_group(
    l: &mut Loader<'_>,
    id: AttrGroupId,
    expanded: &mut Vec<Option<Vec<AttributeUse>>>,
    in_progress: &mut FxHashSet<AttrGroupId>,
) -> Vec<AttributeUse> {
    if let Some(done) = &expanded[id.index()] {
        return done.clone();
    }
    if !in_progress.insert(id) {
        let name = l.attribute_groups.get(id.0).name;
        let shown = l.names.display(name);
        let span = l.attribute_groups.get(id.0).span.clone();
        l.diags.push(
            Diagnostic::error(
                DiagCode::CircularDefinition,
                format!("attribute group `{shown}` references itself"),
            )
            .at(span),
        );
        return Vec::new();
    }

    let mut out = l.attribute_groups.get(id.0).attribute_uses.clone();
    let refs = l.attribute_groups.get(id.0).attribute_group_refs.clone();
    for r in refs {
        for u in expand_group(l, r, expanded, in_progress) {
            if !out.iter().any(|e| e.attribute == u.attribute) {
                out.push(u);
            }
        }
    }

    in_progress.remove(&id);
    l.attribute_groups.get_mut(id.0).attribute_uses = out.clone();
    expanded[id.index()] = Some(out.clone());
    out
}

// ---------------------------------------------------------------------------
// 3. Inherited attribute uses
// ---------------------------------------------------------------------------

/// Folds each complex type's base chain into its own attribute uses.
///
/// **Both** derivation methods inherit attributes, unlike content models
/// where extension appends and restriction replaces. Extension adds uses;
/// restriction may only narrow one already present, or remove it with
/// `use="prohibited"`.
///
/// Without this, a type that extends another without adding attributes has
/// none at all — and that is not a corner case. GML's whole measure family is
/// vacuous extensions of `gml:MeasureType`, so every one of its twenty
/// measure types would report no `uom`.
fn merge_inherited_attributes(l: &mut Loader<'_>) {
    let n = l.types.len();
    let mut done: Vec<Option<Vec<AttributeUse>>> = vec![None; n];
    let mut in_progress = FxHashSet::default();
    for i in 0..n {
        let uses = effective_attributes(l, TypeId::from_index(i), &mut done, &mut in_progress);
        if let TypeDefinition::Complex(c) = l.types.get_mut(i as u32) {
            c.attribute_uses = uses;
        }
    }
}

fn effective_attributes(
    l: &mut Loader<'_>,
    id: TypeId,
    done: &mut Vec<Option<Vec<AttributeUse>>>,
    in_progress: &mut FxHashSet<TypeId>,
) -> Vec<AttributeUse> {
    if let Some(cached) = &done[id.index()] {
        return cached.clone();
    }
    let TypeDefinition::Complex(c) = l.types.get(id.0) else {
        done[id.index()] = Some(Vec::new());
        return Vec::new();
    };
    let own = c.attribute_uses.clone();
    let base = c.base;

    // A derivation cycle is reported by `check_cycles`; here it must simply
    // not recurse forever.
    if base == id || base.is_placeholder() || !in_progress.insert(id) {
        done[id.index()] = Some(own.clone());
        return own;
    }

    let mut out = effective_attributes(l, base, done, in_progress);
    for u in own {
        let name = l.attributes.get(u.attribute.0).name;
        // An own use replaces the inherited one for the same attribute — that
        // is how a restriction narrows or prohibits.
        match out
            .iter()
            .position(|e| l.attributes.get(e.attribute.0).name == name)
        {
            Some(i) => out[i] = u,
            None => out.push(u),
        }
    }

    in_progress.remove(&id);
    done[id.index()] = Some(out.clone());
    out
}

// ---------------------------------------------------------------------------
// 4. Simple content
// ---------------------------------------------------------------------------

/// Points a `simpleContent` complex type at the simple type it validates
/// character data against, which is its base's.
fn resolve_simple_content(l: &mut Loader<'_>) {
    for i in 0..l.types.len() as u32 {
        let (needs, base) = match l.types.get(i) {
            TypeDefinition::Complex(c) => match c.content {
                ContentType::Simple(t) if t.is_placeholder() => (true, c.base),
                _ => (false, TypeId::PLACEHOLDER),
            },
            TypeDefinition::Simple(_) => continue,
        };
        if !needs {
            continue;
        }
        // The base is either a simple type (use it) or a complex type with
        // simple content (use that type's own simple content).
        let resolved = match l.types.get(base.0) {
            TypeDefinition::Simple(_) => base,
            TypeDefinition::Complex(bc) => match bc.content {
                ContentType::Simple(t) if !t.is_placeholder() => t,
                _ => base,
            },
        };
        if let TypeDefinition::Complex(c) = l.types.get_mut(i) {
            c.content = ContentType::Simple(resolved);
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Cycle detection
// ---------------------------------------------------------------------------

fn check_cycles(l: &mut Loader<'_>) {
    let mut reported = FxHashSet::default();
    for i in 0..l.types.len() as u32 {
        let start = TypeId(i);
        let mut slow = start;
        let mut fast = start;
        // Floyd's cycle detection over the derivation chain.
        while let (Some(f1), true) = (next_base(l, fast), true) {
            let Some(f2) = next_base(l, f1) else { break };
            fast = f2;
            slow = next_base(l, slow).unwrap_or(slow);
            if slow == fast {
                if reported.insert(slow) {
                    let name = l.types.get(slow.0).name();
                    let shown = name
                        .map(|n| l.names.display(n))
                        .unwrap_or_else(|| "<anonymous>".into());
                    let span = l.types.get(slow.0).span().clone();
                    l.diags.push(
                        Diagnostic::error(
                            DiagCode::CircularDefinition,
                            format!("type `{shown}` is derived from itself"),
                        )
                        .at(span),
                    );
                }
                break;
            }
        }
    }
}

/// Reports a simple type that reaches itself through its item or member types.
///
/// Separate from the derivation walk above, which follows `base` only. A list
/// of itself and a union containing itself are just as circular, and checking
/// a value against one would not terminate — the fuzzer found the schema that
/// does it before anyone wrote it on purpose.
fn check_structural_cycles(l: &mut Loader<'_>) {
    // Grey means "on the current path", black means "finished and clean".
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Grey,
        Black,
    }
    let n = l.types.len();
    let mut marks = vec![Mark::White; n];
    let mut found = Vec::new();

    // An explicit stack: a schema is untrusted input, so the recursion depth
    // would be too.
    for root in 0..n as u32 {
        if marks[root as usize] != Mark::White {
            continue;
        }
        let mut stack = vec![(root, 0usize)];
        marks[root as usize] = Mark::Grey;
        while let Some((id, next)) = stack.pop() {
            let edges = structural_edges(l, TypeId(id));
            match edges.get(next) {
                None => {
                    marks[id as usize] = Mark::Black;
                }
                Some(&child) => {
                    stack.push((id, next + 1));
                    let c = child.index();
                    if c >= n {
                        continue;
                    }
                    match marks[c] {
                        Mark::Grey => found.push(TypeId(child.0)),
                        Mark::White => {
                            marks[c] = Mark::Grey;
                            stack.push((child.0, 0));
                        }
                        Mark::Black => {}
                    }
                }
            }
        }
    }

    let mut reported = FxHashSet::default();
    for id in found {
        if !reported.insert(id) {
            continue;
        }
        let def = l.types.get(id.0);
        let shown = def
            .name()
            .map(|n| l.names.display(n))
            .unwrap_or_else(|| "<anonymous>".into());
        let span = def.span().clone();
        l.diags.push(
            Diagnostic::error(
                DiagCode::CircularDefinition,
                format!("simple type `{shown}` contains itself"),
            )
            .at(span)
            .with_help("a list cannot have itself as its item type, nor a union as a member"),
        );
    }
}

/// The types a simple type is built from: its item type and its members.
///
/// Not `base` — the derivation walk covers that, and following both here would
/// report one cycle twice.
fn structural_edges(l: &Loader<'_>, id: TypeId) -> Vec<TypeId> {
    let TypeDefinition::Simple(s) = l.types.get(id.0) else {
        return Vec::new();
    };
    s.item_type
        .into_iter()
        .chain(s.member_types.iter().copied())
        .filter(|t| !t.is_placeholder())
        .collect()
}

/// The next link in a derivation chain, or `None` at a self-referential root.
fn next_base(l: &Loader<'_>, id: TypeId) -> Option<TypeId> {
    if id.is_placeholder() || id.index() >= l.types.len() {
        return None;
    }
    let base = l.types.get(id.0).base();
    if base == id || base.is_placeholder() {
        None
    } else {
        Some(base)
    }
}

// ---------------------------------------------------------------------------
// 6. Substitution closure
// ---------------------------------------------------------------------------

/// Precomputes, for every head, the transitive set of elements that may
/// substitute for it.
///
/// Substitution is transitive: if `b` substitutes for `a` and `c` for `b`,
/// then `c` may appear wherever `a` is permitted. Without this you cannot
/// know which element names are legal at a position in GML, UBL or WITSML.
fn build_substitution_closure(l: &Loader<'_>) -> FxHashMap<ElementId, Vec<ElementId>> {
    let mut direct: FxHashMap<ElementId, Vec<ElementId>> = FxHashMap::default();
    for i in 0..l.elements.len() as u32 {
        let member = ElementId(i);
        for &head in &l.elements.get(i).substitution_group {
            direct.entry(head).or_default().push(member);
        }
    }

    let mut out: FxHashMap<ElementId, Vec<ElementId>> = FxHashMap::default();
    for &head in direct.keys() {
        let mut seen = FxHashSet::default();
        let mut stack = vec![head];
        let mut members = Vec::new();
        while let Some(cur) = stack.pop() {
            let Some(kids) = direct.get(&cur) else {
                continue;
            };
            for &k in kids {
                if seen.insert(k) {
                    // An abstract member is a head only; it cannot itself
                    // appear in an instance.
                    if !l.elements.get(k.0).is_abstract {
                        members.push(k);
                    }
                    stack.push(k);
                }
            }
        }
        out.insert(head, members);
    }
    out
}

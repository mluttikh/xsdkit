//! Where an `xs:all` group may appear, and what it may contain.
//!
//! *All Group Limited*, and one of the few constraints the two versions state
//! differently rather than one merely adding to the other.
//!
//! An `all` group is not a regular expression. Its members are matched in any
//! order, which is why [`crate::content`] gives it per-member counters instead
//! of an automaton — and why the specification confines it to positions where
//! that treatment is enough. Outside them, "any order" would have to compose
//! with sequencing, and it cannot.
//!
//! # 1.0
//!
//! An `all` may be a named group definition's group, or a complex type's whole
//! content model with `maxOccurs="1"`. Nothing else. And every member particle
//! must have `maxOccurs` of 0 or 1: one member, at most once.
//!
//! # 1.1
//!
//! Two changes, in opposite directions. The member bound is **gone** — `all`
//! members may repeat, which is the `xsd1_1-AllGroups-MaxOccurs` feature — and
//! an `all` may now also sit *inside* another `all`, with
//! `minOccurs = maxOccurs = 1`. In exchange, clause 2 acquires a new job: a
//! member whose term is a model group has to be an `all` group. That is what
//! makes a group reference inside an `all` legal at all, and it is the half
//! that needs the reference resolved — hence this runs over the assembled
//! [`Schemas`] rather than in the loader's document sweep.
//!
//! # What this does not do
//!
//! Each type's **own** content particle is walked, not its effective one. An
//! extension's effective model is "the base's particle, then its own", which
//! [`crate::content`] assembles as a sequence — walking that would report
//! every `all` inherited by an extension as an `all` inside a sequence.
//! Whether an `all` may extend a sequence at all is *Derivation Valid
//! (Extension)*, a different rule, and unimplemented.

use crate::diagnostics::{DiagCode, Diagnostic, Diagnostics};
use crate::load::Version;
use crate::model::{Compositor, MaxOccurs, ModelGroup, Particle, Schemas, Term};

/// Where a particle sits, which is what decides whether its term may be an
/// `all` group.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Position {
    /// The `{particle}` of a complex type's `{content type}` — clause 1.2.
    ContentModel,
    /// Inside an `all` — clause 1.3, and 1.1 only.
    InsideAll,
    /// Inside a sequence or a choice, where an `all` is never allowed.
    InsideOrdered,
}

pub(crate) fn check_all(schemas: &Schemas) -> Diagnostics {
    let mut diags = Diagnostics::new();
    let version = schemas.xsd_version();

    // Clause 1.1: a named group definition's own group may be an `all`, so the
    // group itself is not checked for position — only its contents.
    for (_, def) in schemas.iter_model_groups() {
        walk_group(schemas, &def.group, version, &mut diags);
    }

    for (_, def) in schemas.iter_types() {
        let Some(complex) = def.as_complex() else {
            continue;
        };
        let Some(pid) = complex.content.particle() else {
            continue;
        };
        walk_particle(
            schemas,
            &schemas[pid],
            Position::ContentModel,
            version,
            &mut diags,
        );
    }
    diags
}

fn walk_particle(
    schemas: &Schemas,
    particle: &Particle,
    position: Position,
    version: Version,
    diags: &mut Diagnostics,
) {
    let Term::Group(group) = &particle.term else {
        // An element, a wildcard, or a reference to a named group. A reference
        // is checked where it sits — as a *member*, below — and the group it
        // names is walked from the arena, once, however many places point at
        // it.
        return;
    };
    if group.compositor == Compositor::All {
        check_position(particle, position, version, diags);
    }
    walk_group(schemas, group, version, diags);
}

/// Clause 1: the positions an `all` may occupy.
fn check_position(
    particle: &Particle,
    position: Position,
    version: Version,
    diags: &mut Diagnostics,
) {
    let bad = |what: &str, help: &str, diags: &mut Diagnostics| {
        diags.push(
            Diagnostic::error(
                DiagCode::InvalidOccurrence,
                format!("an `xs:all` group {what}"),
            )
            .at(particle.span.clone())
            .with_help(help.to_string()),
        );
    };
    match position {
        // Clause 1.2. The bound is on the particle carrying the `all`, and
        // both versions require it: an `all` that may repeat would need the
        // ordering its members do not have.
        Position::ContentModel => {
            if particle.max_occurs != MaxOccurs::Bounded(1) {
                bad(
                    "may not have `maxOccurs` greater than 1",
                    "an `xs:all` matches its members in any order, which cannot be repeated",
                    diags,
                );
            }
        }
        // Clause 1.3, new in 1.1, and exactly once.
        Position::InsideAll if version == Version::Xsd11 => {
            if particle.min_occurs != 1 || particle.max_occurs != MaxOccurs::Bounded(1) {
                bad(
                    "nested in another `xs:all` must have `minOccurs` and `maxOccurs` of 1",
                    "XSD 1.1 allows the nesting, but only for exactly one occurrence",
                    diags,
                );
            }
        }
        Position::InsideAll => bad(
            "may not appear inside another `xs:all` in XSD 1.0",
            "nesting an `xs:all` needs XSD 1.1",
            diags,
        ),
        Position::InsideOrdered => bad(
            "may only be a complex type's whole content model, or a named group definition",
            "an `xs:all` cannot sit inside an `xs:sequence` or an `xs:choice`",
            diags,
        ),
    }
}

fn walk_group(schemas: &Schemas, group: &ModelGroup, version: Version, diags: &mut Diagnostics) {
    let inside = match group.compositor {
        Compositor::All => Position::InsideAll,
        Compositor::Sequence | Compositor::Choice => Position::InsideOrdered,
    };
    for &pid in &group.particles {
        let member = &schemas[pid];
        if group.compositor == Compositor::All {
            check_member(schemas, member, version, diags);
        }
        walk_particle(schemas, member, inside, version, diags);
    }
}

/// What a member of an `all` may be. The two versions disagree about this more
/// than about anything else in the rule.
fn check_member(schemas: &Schemas, member: &Particle, version: Version, diags: &mut Diagnostics) {
    if version == Version::Xsd10 {
        // 1.0 clause 2: at most once, each.
        if member.max_occurs != MaxOccurs::Bounded(0) && member.max_occurs != MaxOccurs::Bounded(1)
        {
            diags.push(
                Diagnostic::error(
                    DiagCode::InvalidOccurrence,
                    "a member of an `xs:all` may not have `maxOccurs` greater than 1".to_string(),
                )
                .at(member.span.clone())
                .with_help("XSD 1.1 lifted this; in 1.0 an `xs:all` member occurs at most once"),
            );
        }
        return;
    }

    // 1.1 clause 2: a member whose term is a model group has to be an `all`
    // group — and by clause 1.3 it occurs exactly once. An inline `xs:all` is
    // handled by `check_position`; what is left is a reference to a named
    // group, whose compositor is only knowable once it is resolved.
    let Term::GroupRef(gid) = member.term else {
        return;
    };
    let referenced = &schemas[gid];
    if referenced.group.compositor != Compositor::All {
        diags.push(
            Diagnostic::error(
                DiagCode::InvalidOccurrence,
                format!(
                    "`{}` is referenced inside an `xs:all`, but it is not an `xs:all` group",
                    schemas.display_name(referenced.name)
                ),
            )
            .at(member.span.clone())
            .at(crate::diagnostics::Span::labelled(
                &referenced.span.uri,
                referenced.span.line,
                "defined here",
            ))
            .with_help("only an `xs:all` group may be referenced from inside an `xs:all`"),
        );
        return;
    }
    if member.min_occurs != 1 || member.max_occurs != MaxOccurs::Bounded(1) {
        diags.push(
            Diagnostic::error(
                DiagCode::InvalidOccurrence,
                format!(
                    "`{}` is referenced inside an `xs:all`, so it must occur exactly once",
                    schemas.display_name(referenced.name)
                ),
            )
            .at(member.span.clone())
            .with_help("`minOccurs` and `maxOccurs` must both be 1 on the reference"),
        );
    }
}

//! Does a restriction's content model accept only what its base accepts?
//!
//! *Derivation Valid (Restriction, Complex)*, and the hardest rule in the
//! specification. A restriction states its content model in full rather than
//! by reference, so nothing structural forces it to be narrower than the base
//! — the schema simply asserts it, and the assertion has to be checked.
//!
//! **This one errs towards accepting.** Everywhere else in this crate an
//! uncertain answer is reported, because a missed validity check is cheaper
//! than a schema that will not load. Here the balance tips the other way: the
//! rule is recursive over particle trees, several of its cases are subtle, and
//! a false rejection turns a schema that works everywhere else into one this
//! crate refuses. So each case below either *knows* the restriction is invalid
//! or says nothing, and what is left unjudged is listed at [`unhandled`].

use crate::diagnostics::{DiagCode, Diagnostic, Diagnostics};
use crate::model::{
    Compositor, DerivationMethod, MaxOccurs, ModelGroup, Particle, ParticleId, Schemas, Term,
    Wildcard,
};
use crate::names::QName;

pub(crate) fn check_all(schemas: &Schemas) -> Diagnostics {
    let mut diags = Diagnostics::new();
    for (id, def) in schemas.iter_types() {
        let Some(c) = def.as_complex() else { continue };
        if c.derivation != DerivationMethod::Restriction || c.base == id || c.base.is_placeholder()
        {
            continue;
        }
        let Some(r) = c.content.particle() else {
            continue;
        };
        // Not the base's own particle. A base built by extension states only
        // the part it adds, and its content model is that appended to its own
        // base's — which XSD expresses by making an extension's content type a
        // fresh sequence of (base, extension). This rebuilds that sequence.
        let base_parts = crate::content::effective_particles(schemas, c.base);
        if base_parts.is_empty() {
            continue;
        }
        // A single particle *is* the content model; wrapping it in a sequence
        // of one would compare a group reference against a sequence containing
        // that reference, and the two sides would resolve to different depths.
        let wrapped;
        let b = if let [only] = base_parts[..] {
            &schemas[only]
        } else {
            wrapped = as_sequence(&base_parts, &schemas[r]);
            &wrapped
        };

        let Verdict::Bad(why) = particle_ok(schemas, &schemas[r], b) else {
            continue;
        };
        {
            let name = def
                .name()
                .map(|n| format!("`{}`", schemas.display_name(n)))
                .unwrap_or_else(|| "an anonymous type".into());
            diags.push(
                Diagnostic::error(
                    DiagCode::InvalidRestriction,
                    format!("{name} does not restrict its base: {why}"),
                )
                .at(schemas[r].span.clone())
                .with_help("a restriction may only accept a subset of what its base accepts"),
            );
        }
    }
    diags
}

/// The base's content model as one particle occurring exactly once.
fn as_sequence(parts: &[ParticleId], borrow_span_from: &Particle) -> Particle {
    Particle {
        min_occurs: 1,
        max_occurs: MaxOccurs::Bounded(1),
        term: Term::Group(ModelGroup {
            compositor: Compositor::Sequence,
            particles: parts.to_vec(),
        }),
        span: borrow_span_from.span.clone(),
    }
}

/// What a comparison concluded.
///
/// Three outcomes rather than two, and the third is the one that makes this
/// module safe. A pair it cannot judge is *not* a match: treating it as one
/// lets an ordered walk consume a base particle it never checked and then
/// report the leftovers as an error, which is exactly how the first version of
/// this rejected the W3C schema for schemas. `Unknown` propagates all the way
/// out and the restriction is accepted.
#[derive(Debug)]
enum Verdict {
    Ok,
    Bad(String),
    Unknown,
}

/// Runs `f`, short-circuiting the caller on anything but `Ok`.
macro_rules! settled {
    ($e:expr) => {
        match $e {
            Verdict::Ok => {}
            other => return other,
        }
    };
}

/// The cases this module deliberately does not judge.
///
/// A restriction whose shape lands here is accepted. Each is a place where
/// getting the rule subtly wrong would reject working schemas:
///
/// - a group restricting a wildcard (*NSRecurseCheckCardinality*), which has
///   to sum occurrences across a whole group;
/// - `choice` against `sequence`, or either against the other in the
///   direction this module does not cover (*MapAndSum*, *RecurseUnordered*),
///   where the mapping is neither positional nor by name;
/// - a model whose members are not all elements, once the base is an `xs:all`
///   — nested groups inside an `all` are XSD 1.1 and change what "the member
///   named x" means;
/// - a wildcard carrying `##defined` or `##definedSibling`, whose exclusions
///   depend on a content model rather than on the wildcard alone.
fn unhandled() -> Verdict {
    Verdict::Unknown
}

fn particle_ok(schemas: &Schemas, rp: &Particle, bp: &Particle) -> Verdict {
    match (&rp.term, &bp.term) {
        // Elt:Elt — NameAndTypeOK.
        (Term::Element(re), Term::Element(be)) => {
            let (rn, bn) = (schemas[*re].name, schemas[*be].name);
            if rn != bn {
                // XSD 1.1 lets a restriction name a member of the base
                // particle's substitution group, and several members may share
                // one base particle — so the occurrence bounds have to be
                // *summed* across them rather than compared one at a time.
                // That is `MapAndSum`, and guessing at it would reject working
                // schemas, so a substitutable name is declined rather than
                // refused.
                if substitutes_for(schemas, rn, *be) {
                    return unhandled();
                }
                return Verdict::Bad(format!(
                    "`{}` does not appear in the base",
                    schemas.display_name(rn)
                ));
            }
            settled!(range_ok(rp, bp, &format!("`{}`", schemas.display_name(rn))));
            // The declared type has to be the base declaration's or derived
            // from it, or a document valid against the restriction could carry
            // a value the base rejects.
            let (rt, bt) = (schemas[*re].type_id, schemas[*be].type_id);
            if !type_derivation_ok(schemas, rt, bt, 0) {
                return Verdict::Bad(format!(
                    "the type of `{}` is not derived from its type in the base",
                    schemas.display_name(rn)
                ));
            }
            Verdict::Ok
        }

        // Elt:Any — NSCompat. The wildcard has to admit the name.
        (Term::Element(re), Term::Wildcard(w)) => {
            let name = schemas[*re].name;
            if w.not_defined_sibling || w.not_defined {
                return unhandled();
            }
            if !w.namespace.admits(name.ns) || w.not_qname.contains(&name) {
                return Verdict::Bad(format!(
                    "`{}` is not admitted by the wildcard it replaces",
                    schemas.display_name(name)
                ));
            }
            range_ok(rp, bp, &format!("`{}`", schemas.display_name(name)))
        }

        // Any:Any — NSSubset.
        (Term::Wildcard(rw), Term::Wildcard(bw)) => {
            if !namespace_subset(rw, bw) {
                return Verdict::Bad("its wildcard admits namespaces the base's does not".into());
            }
            range_ok(rp, bp, "the wildcard")
        }

        // A wildcard admits everything the element did and more.
        (Term::Wildcard(_), Term::Element(_)) => {
            Verdict::Bad("a wildcard cannot restrict an element declaration".into())
        }

        _ => match (members(schemas, rp), members(schemas, bp)) {
            // Elt:All/Sequence — RecurseAsIfGroup, reading the lone particle
            // as a group of one.
            (None, Some((bc, bs))) => {
                let one = Particle {
                    min_occurs: 1,
                    max_occurs: MaxOccurs::Bounded(1),
                    term: Term::Group(ModelGroup {
                        compositor: bc,
                        particles: Vec::new(),
                    }),
                    span: rp.span.clone(),
                };
                group_ok(schemas, (&one, bc, &[rp]), (bp, bc, &bs))
            }
            (Some((rc, rs)), Some((bc, bs))) => group_ok(schemas, (rp, rc, &rs), (bp, bc, &bs)),
            _ => unhandled(),
        },
    }
}

/// A group against a group.
fn group_ok(
    schemas: &Schemas,
    (rp, rc, rs): (&Particle, Compositor, &[&Particle]),
    (bp, bc, bs): (&Particle, Compositor, &[&Particle]),
) -> Verdict {
    settled!(range_ok(rp, bp, "the group"));
    match (rc, bc) {
        // The base is unordered, so members correspond by name whatever the
        // restriction's own compositor is — which is what lets an `xs:sequence`
        // restrict an `xs:all` at all.
        (_, Compositor::All) => by_name(schemas, rs, bs),
        // Same compositor, ordered: walk both, letting the base skip what it
        // did not require.
        (Compositor::Sequence, Compositor::Sequence) => recurse(schemas, rs, bs),
        (Compositor::Choice, Compositor::Choice) => recurse_lax(schemas, rs, bs),
        _ => unhandled(),
    }
}

/// *Recurse* against an unordered base: every member the restriction keeps has
/// to name one in the base, and every one it drops has to have been optional.
fn by_name(schemas: &Schemas, rs: &[&Particle], bs: &[&Particle]) -> Verdict {
    let named = |p: &Particle| -> Option<QName> {
        match &p.term {
            Term::Element(e) => Some(schemas[*e].name),
            _ => None,
        }
    };
    // Nested groups inside an `xs:all` are XSD 1.1 and change what "the member
    // named x" means, so a model with one is left alone.
    if rs.iter().any(|p| named(p).is_none()) || bs.iter().any(|p| named(p).is_none()) {
        return unhandled();
    }

    let mut matched = vec![false; bs.len()];
    for rp in rs {
        let name = named(rp).expect("checked above");
        let Some(i) = bs.iter().position(|bp| named(bp) == Some(name)) else {
            // Not there by name — but it may substitute for one that is, and
            // that case belongs to `MapAndSum` rather than here.
            if substitutes_for_any(schemas, rp, bs) {
                return unhandled();
            }
            return Verdict::Bad(format!(
                "`{}` does not appear in the base",
                schemas.display_name(name)
            ));
        };
        matched[i] = true;
        settled!(particle_ok(schemas, rp, bs[i]));
    }
    for (i, bp) in bs.iter().enumerate() {
        if !matched[i] && !is_emptiable(schemas, bp) {
            return Verdict::Bad(format!(
                "`{}` is required by the base and missing here",
                schemas.display_name(named(bp).expect("checked above"))
            ));
        }
    }
    Verdict::Ok
}

/// *Recurse*, for a sequence against a sequence: the restriction's members map
/// onto the base's in order, and anything skipped has to have been optional —
/// dropping a required step changes what the type accepts.
fn recurse(schemas: &Schemas, rs: &[&Particle], bs: &[&Particle]) -> Verdict {
    let mut bi = 0usize;
    for rp in rs {
        loop {
            let Some(bp) = bs.get(bi) else {
                return Verdict::Bad("it has more particles than the base".into());
            };
            bi += 1;
            match particle_ok(schemas, rp, bp) {
                Verdict::Ok => break,
                // Not a match, and not a judgement either — the alignment from
                // here on is guesswork, so stop rather than guess.
                Verdict::Unknown => return Verdict::Unknown,
                // A base particle this one does not match may be passed over
                // only if the base did not require it.
                Verdict::Bad(_) if is_emptiable(schemas, bp) => {}
                Verdict::Bad(why) => return Verdict::Bad(why),
            }
        }
    }
    for bp in &bs[bi..] {
        if !is_emptiable(schemas, bp) {
            return Verdict::Bad(describe(schemas, bp));
        }
    }
    Verdict::Ok
}

/// *RecurseLax*, for a choice against a choice.
///
/// The same order-preserving walk, without the emptiability rule: narrowing a
/// choice means removing alternatives, so a base alternative the restriction
/// does not keep is the point rather than an error.
fn recurse_lax(schemas: &Schemas, rs: &[&Particle], bs: &[&Particle]) -> Verdict {
    let mut bi = 0usize;
    for rp in rs {
        loop {
            let Some(bp) = bs.get(bi) else {
                return Verdict::Bad("it offers an alternative the base does not".into());
            };
            bi += 1;
            match particle_ok(schemas, rp, bp) {
                Verdict::Ok => break,
                Verdict::Unknown => return Verdict::Unknown,
                Verdict::Bad(_) => {}
            }
        }
    }
    Verdict::Ok
}

/// Whether `derived` may stand where `base` is declared.
///
/// Ordinary derivation, plus the rule that is easy to miss: a member of a
/// union is validly derived from that union. `sub-chap` need not restrict
/// `chap` if `chap` is a union that lists it — the values it admits are a
/// subset by construction.
fn type_derivation_ok(
    schemas: &Schemas,
    derived: crate::model::TypeId,
    base: crate::model::TypeId,
    depth: u32,
) -> bool {
    if derived == base || schemas.derives_from(derived, base) {
        return true;
    }
    // A union of unions is legal, so this recurses; the guard is against a
    // malformed schema whose members cycle.
    if depth > 16 {
        return true;
    }
    schemas[base]
        .as_simple()
        .map(|s| {
            s.member_types
                .iter()
                .any(|m| type_derivation_ok(schemas, derived, *m, depth + 1))
        })
        .unwrap_or(false)
}

/// Whether an element named `name` may substitute for the declaration `head`.
///
/// By name rather than by id: the declaration doing the substituting need not
/// be the global one. A local `<xs:element name="A1"/>` stands in for the
/// global `A1` that joined the group, and comparing ids would miss it.
fn substitutes_for(schemas: &Schemas, name: QName, head: crate::model::ElementId) -> bool {
    schemas
        .substitution_group(head)
        .into_iter()
        .any(|m| schemas[m].name == name)
}

/// Whether `rp` is an element that may substitute for one of `bs`.
fn substitutes_for_any(schemas: &Schemas, rp: &Particle, bs: &[&Particle]) -> bool {
    let Term::Element(re) = &rp.term else {
        return false;
    };
    let name = schemas[*re].name;
    bs.iter().any(|bp| match &bp.term {
        Term::Element(be) => substitutes_for(schemas, name, *be),
        _ => false,
    })
}

fn describe(schemas: &Schemas, b: &Particle) -> String {
    match &b.term {
        Term::Element(e) => format!(
            "`{}` is required by the base and missing here",
            schemas.display_name(schemas[*e].name)
        ),
        _ => "a particle the base requires is missing here".into(),
    }
}

/// *Occurrence Range OK*: the restriction may not allow more, nor fewer.
fn range_ok(r: &Particle, b: &Particle, what: &str) -> Verdict {
    if r.min_occurs < b.min_occurs {
        return Verdict::Bad(format!(
            "`minOccurs` for {what} is {}, below the base's {}",
            r.min_occurs, b.min_occurs
        ));
    }
    let ok = match (r.max_occurs, b.max_occurs) {
        (_, MaxOccurs::Unbounded) => true,
        (MaxOccurs::Unbounded, _) => false,
        (MaxOccurs::Bounded(x), MaxOccurs::Bounded(y)) => x <= y,
    };
    if !ok {
        return Verdict::Bad(format!(
            "`maxOccurs` for {what} is {}, above the base's {}",
            show_max(r.max_occurs),
            show_max(b.max_occurs)
        ));
    }
    Verdict::Ok
}

fn show_max(m: MaxOccurs) -> String {
    match m {
        MaxOccurs::Unbounded => "unbounded".into(),
        MaxOccurs::Bounded(n) => n.to_string(),
    }
}

/// A particle's members, if it is a group.
///
/// Members that are themselves an inline group of the same compositor
/// occurring exactly once are spliced in. `(a, (b, c))` and `(a, b, c)` are
/// the same sequence, and real schemas produce the nested shape constantly —
/// an extension's content model *is* a sequence of the base's and its own, so
/// every type in a derivation chain adds a layer. Comparing member counts
/// without flattening rejects the W3C schema for schemas.
fn members<'a>(schemas: &'a Schemas, p: &'a Particle) -> Option<(Compositor, Vec<&'a Particle>)> {
    let (c, ids) = group_of(schemas, p)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let child = &schemas[*id];
        match group_of(schemas, child) {
            // Only inline groups: a `xs:group ref` is a component in its own
            // right, and the base's reference to it should meet the
            // restriction's rather than dissolve into the surrounding model.
            Some((cc, inner))
                if cc == c
                    && matches!(child.term, Term::Group(_))
                    && child.min_occurs == 1
                    && child.max_occurs == MaxOccurs::Bounded(1) =>
            {
                for inner_id in inner {
                    out.push(&schemas[*inner_id]);
                }
            }
            _ => out.push(child),
        }
    }
    Some((c, out))
}

/// The compositor and member ids of a particle that is a group.
fn group_of<'a>(
    schemas: &'a Schemas,
    p: &'a Particle,
) -> Option<(Compositor, &'a Vec<ParticleId>)> {
    match &p.term {
        Term::Group(g) => Some((g.compositor, &g.particles)),
        Term::GroupRef(gid) if !gid.is_placeholder() => {
            let g = &schemas[*gid].group;
            Some((g.compositor, &g.particles))
        }
        _ => None,
    }
}

/// Whether a particle can match nothing at all.
fn is_emptiable(schemas: &Schemas, p: &Particle) -> bool {
    if p.min_occurs == 0 {
        return true;
    }
    match members(schemas, p) {
        Some((Compositor::Choice, ps)) => ps.iter().any(|c| is_emptiable(schemas, c)),
        Some((_, ps)) => ps.iter().all(|c| is_emptiable(schemas, c)),
        None => false,
    }
}

/// Whether every namespace `r` admits, `b` admits too.
fn namespace_subset(r: &Wildcard, b: &Wildcard) -> bool {
    use crate::model::NamespaceConstraint::*;
    match (&r.namespace, &b.namespace) {
        (_, Any) => true,
        (Any, _) => false,
        (Enumeration(x), Enumeration(y)) => x.iter().all(|n| y.contains(n)),
        // Everything outside `y` is admitted, so `x` must avoid all of it.
        (Enumeration(x), Not(y)) => x.iter().all(|n| !y.contains(n)),
        // A longer exclusion list is a smaller set.
        (Not(x), Not(y)) => y.iter().all(|n| x.contains(n)),
        // "Anything but these" is never inside a finite list.
        (Not(_), Enumeration(_)) => false,
    }
}

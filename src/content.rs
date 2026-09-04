//! Content models compiled to position automata.
//!
//! Every complex type's particle tree is compiled once into a Glushkov
//! (position) automaton. Three things fall out of that single structure:
//!
//! - **Unique Particle Attribution.** A content model is 1-unambiguous
//!   exactly when its position automaton is deterministic, so the UPA check
//!   is "does any state have two out-transitions with overlapping labels".
//! - **Which elements may appear here** — the transition labels, expanded
//!   through substitution groups.
//! - **Whether an element may repeat** — whether its position can reach
//!   itself.
//!
//! The last two are the primitives the future `xml2arrow` config generator's
//! table/column split is built on.
//!
//! # Derivation
//!
//! A type derived by **extension** has the effective content model
//! "base's particle, then its own" — so building from a type's own particle
//! alone would lose every inherited child. A type derived by **restriction**
//! states its content model in full and replaces the base's. Both are handled
//! by `effective_particles`, which walks the base chain and stops at the
//! first restriction step.
//!
//! # Occurrence ranges
//!
//! Numeric ranges are **unrolled**: `a{2,4}` becomes three positions, the
//! third optional. That keeps the automaton an ordinary Glushkov automaton
//! with no counter machinery, which is a large simplification.
//!
//! `maxOccurs="5000"` is legal and appears in the wild, so unrolling is
//! capped two ways: [`MAX_UNROLL`] copies of any one particle, and
//! [`MAX_POSITIONS`] positions per model. Past either cap the range is
//! widened to unbounded and the model is marked
//! [`ContentAutomaton::approximated`].
//!
//! The copy cap is the tighter of the two, and it is what keeps construction
//! near-linear: unrolling `a{1,n}` leaves every optional copy in the `last`
//! set, so concatenating the next one costs `O(n)` edges and the whole
//! unrolling costs `O(n^2)`. Sixty-four copies is past anything a real schema
//! spells out and cheap to build; the position cap then guards models that
//! are merely enormous rather than repetitive.
//! Widening only ever *adds* reachable positions, so an approximated model
//! accepts a superset of the true language and its UPA verdict may be a
//! false positive — never a false negative. Callers downgrade UPA findings
//! on approximated models to warnings for exactly that reason.

use crate::diagnostics::{DiagCode, Diagnostic, Diagnostics, Severity, Span};
use crate::model::*;
use crate::names::QName;
use fxhash::FxHashSet;

/// Cap on positions per content model, bounding total model size.
///
/// This one is generous: a flat sequence of hundreds of distinct elements is
/// perfectly ordinary and must not be truncated.
pub const MAX_POSITIONS: usize = 4096;

/// Cap on unrolled copies of a single particle.
///
/// Deliberately far tighter than [`MAX_POSITIONS`], because unrolling is
/// quadratic in the copy count while a flat model is linear in its size.
pub const MAX_UNROLL: u32 = 64;

/// Index of a position within one [`ContentAutomaton`].
pub type PositionId = u32;

/// What a position matches.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Label {
    /// An element particle. Admits the element itself plus every member of
    /// its substitution group, so the admitted set is usually larger than
    /// one name.
    Element(ElementId),
    /// A wildcard particle; the constraint lives on the particle's term.
    Wildcard,
}

/// One occurrence of an element or wildcard in a content model.
#[derive(Clone, Debug)]
pub struct Position {
    /// The particle this position came from. Several positions share one
    /// particle when a numeric range was unrolled.
    pub particle: ParticleId,
    pub label: Label,
    /// Every name this position admits, precomputed for element labels.
    /// Empty for wildcards, whose admission is a predicate.
    pub admits: Vec<QName>,
}

/// A Glushkov position automaton over a content model.
#[derive(Clone, Debug)]
pub struct ContentAutomaton {
    positions: Vec<Position>,
    first: Vec<PositionId>,
    follow: Vec<Vec<PositionId>>,
    last: Vec<PositionId>,
    nullable: bool,
    approximated: bool,
}

impl ContentAutomaton {
    pub fn positions(&self) -> &[Position] {
        &self.positions
    }

    pub fn position(&self, p: PositionId) -> &Position {
        &self.positions[p as usize]
    }

    /// Positions a match may start at.
    pub fn first(&self) -> &[PositionId] {
        &self.first
    }

    /// Positions reachable immediately after `p`.
    pub fn follow(&self, p: PositionId) -> &[PositionId] {
        &self.follow[p as usize]
    }

    /// Positions a match may end at.
    pub fn last(&self) -> &[PositionId] {
        &self.last
    }

    /// Whether empty content is accepted.
    pub fn is_nullable(&self) -> bool {
        self.nullable
    }

    /// Whether unrolling hit [`MAX_POSITIONS`] and a range was widened to
    /// unbounded. An approximated model accepts a superset of its true
    /// language.
    pub fn approximated(&self) -> bool {
        self.approximated
    }

    /// Whether `p` can reach itself, i.e. whether its element may repeat.
    pub fn repeats(&self, p: PositionId) -> bool {
        let mut seen = FxHashSet::default();
        let mut stack: Vec<PositionId> = self.follow(p).to_vec();
        while let Some(q) = stack.pop() {
            if q == p {
                return true;
            }
            if seen.insert(q) {
                stack.extend_from_slice(self.follow(q));
            }
        }
        false
    }
}

/// An `xs:all` group: every member may appear in any order, each within its
/// own occurrence bounds.
///
/// Modelled separately rather than as an automaton — the interleaving of `n`
/// members is `n!` paths as a regex, and a per-member counter is both
/// smaller and exactly what the specification describes.
#[derive(Clone, Debug)]
pub struct AllGroup {
    pub members: Vec<AllMember>,
}

#[derive(Clone, Debug)]
pub struct AllMember {
    pub particle: ParticleId,
    pub label: Label,
    pub admits: Vec<QName>,
    pub min_occurs: u32,
    pub max_occurs: MaxOccurs,
}

impl AllGroup {
    /// Whether empty content satisfies every member's lower bound.
    pub fn is_nullable(&self) -> bool {
        self.members.iter().all(|m| m.min_occurs == 0)
    }
}

/// The compiled form of a complex type's content.
#[derive(Clone, Debug)]
pub enum ContentModel {
    /// No child elements are permitted.
    Empty,
    Automaton(ContentAutomaton),
    All(AllGroup),
}

impl ContentModel {
    pub fn is_nullable(&self) -> bool {
        match self {
            ContentModel::Empty => true,
            ContentModel::Automaton(a) => a.is_nullable(),
            ContentModel::All(a) => a.is_nullable(),
        }
    }

    /// Every element declaration this model may admit directly, in first
    /// appearance order, with substitution groups expanded and duplicates
    /// removed.
    pub fn admitted_elements(&self, schemas: &Schemas) -> Vec<ElementId> {
        let mut out = Vec::new();
        let mut seen = FxHashSet::default();
        let mut push = |label: &Label| {
            if let Label::Element(e) = label {
                for m in schemas.substitution_closure(*e) {
                    if seen.insert(m) {
                        out.push(m);
                    }
                }
            }
        };
        match self {
            ContentModel::Empty => {}
            ContentModel::Automaton(a) => a.positions.iter().for_each(|p| push(&p.label)),
            ContentModel::All(a) => a.members.iter().for_each(|m| push(&m.label)),
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

/// A partially built model fragment, in Glushkov terms.
struct Frag {
    first: Vec<PositionId>,
    last: Vec<PositionId>,
    nullable: bool,
}

impl Frag {
    /// The fragment matching only the empty sequence.
    fn empty() -> Self {
        Self {
            first: Vec::new(),
            last: Vec::new(),
            nullable: true,
        }
    }
}

struct Builder<'a> {
    schemas: &'a Schemas,
    positions: Vec<Position>,
    follow: Vec<Vec<PositionId>>,
    approximated: bool,
    /// Guards a group definition that reaches itself. XSD forbids this, but
    /// a malformed schema must not hang the compiler.
    visiting: Vec<GroupId>,
}

impl<'a> Builder<'a> {
    fn new(schemas: &'a Schemas) -> Self {
        Self {
            schemas,
            positions: Vec::new(),
            follow: Vec::new(),
            approximated: false,
            visiting: Vec::new(),
        }
    }

    fn add_position(&mut self, particle: ParticleId, label: Label) -> PositionId {
        let admits = match &label {
            Label::Element(e) => self
                .schemas
                .substitution_closure(*e)
                .into_iter()
                .map(|m| self.schemas[m].name)
                .collect(),
            Label::Wildcard => Vec::new(),
        };
        let id = self.positions.len() as PositionId;
        self.positions.push(Position {
            particle,
            label,
            admits,
        });
        self.follow.push(Vec::new());
        id
    }

    fn link(&mut self, from: &[PositionId], to: &[PositionId]) {
        for &f in from {
            let row = &mut self.follow[f as usize];
            for &t in to {
                if !row.contains(&t) {
                    row.push(t);
                }
            }
        }
    }

    /// `A` followed by `B`. `b_optional` treats `B` as `B?`.
    fn concat(&mut self, a: Frag, b: Frag, b_optional: bool) -> Frag {
        self.link(&a.last, &b.first);
        let b_nullable = b.nullable || b_optional;

        let mut first = a.first.clone();
        if a.nullable {
            extend_unique(&mut first, &b.first);
        }
        let mut last = b.last.clone();
        if b_nullable {
            extend_unique(&mut last, &a.last);
        }
        Frag {
            first,
            last,
            nullable: a.nullable && b_nullable,
        }
    }

    fn union(&mut self, frags: Vec<Frag>) -> Frag {
        let mut out = Frag {
            first: Vec::new(),
            last: Vec::new(),
            nullable: false,
        };
        for f in frags {
            extend_unique(&mut out.first, &f.first);
            extend_unique(&mut out.last, &f.last);
            out.nullable |= f.nullable;
        }
        out
    }

    /// Builds a particle, unrolling its occurrence range.
    fn particle(&mut self, pid: ParticleId) -> Frag {
        let p = &self.schemas[pid];
        let min = p.min_occurs;
        let max = p.max_occurs;

        if max == MaxOccurs::Bounded(0) {
            return Frag::empty();
        }

        // First copy, which also tells us what a copy costs.
        let before = self.positions.len();
        let mut acc = self.term(pid);
        let per_copy = self.positions.len() - before;

        let unbounded = max == MaxOccurs::Unbounded;
        let mut copies = match max {
            MaxOccurs::Unbounded => min.max(1),
            MaxOccurs::Bounded(n) => n,
        };

        // Cap the unrolling, widening to unbounded past either cap.
        let mut widened = unbounded;
        if copies > MAX_UNROLL {
            copies = MAX_UNROLL;
            widened = true;
            self.approximated = true;
        }
        if per_copy > 0 && copies > 1 {
            let room = MAX_POSITIONS.saturating_sub(self.positions.len()) / per_copy + 1;
            if copies as usize > room {
                copies = (room as u32).max(1);
                widened = true;
                self.approximated = true;
            }
        }

        for i in 1..copies {
            let next = self.term(pid);
            acc = self.concat(acc, next, i >= min);
        }

        if widened {
            // Loop the tail back to the start, turning it into `T+`. When
            // several copies were built this links the whole model's last set
            // to its first, which over-approximates `T{n,}` — it accepts a
            // superset, never a subset.
            let (first, last) = (acc.first.clone(), acc.last.clone());
            self.link(&last, &first);
        }

        if min == 0 {
            acc.nullable = true;
        }
        acc
    }

    /// Builds a particle's term, ignoring its occurrence range.
    fn term(&mut self, pid: ParticleId) -> Frag {
        if self.positions.len() >= MAX_POSITIONS {
            self.approximated = true;
            return Frag::empty();
        }
        match &self.schemas[pid].term {
            Term::Element(e) => {
                let p = self.add_position(pid, Label::Element(*e));
                Frag {
                    first: vec![p],
                    last: vec![p],
                    nullable: false,
                }
            }
            Term::Wildcard(_) => {
                let p = self.add_position(pid, Label::Wildcard);
                Frag {
                    first: vec![p],
                    last: vec![p],
                    nullable: false,
                }
            }
            Term::Group(g) => {
                let (compositor, particles) = (g.compositor, g.particles.clone());
                self.model_group(compositor, &particles)
            }
            Term::GroupRef(gid) => {
                let gid = *gid;
                if self.visiting.contains(&gid) {
                    // A self-referential group; already diagnosed elsewhere.
                    return Frag::empty();
                }
                self.visiting.push(gid);
                let def = &self.schemas[gid];
                let (compositor, particles) = (def.group.compositor, def.group.particles.clone());
                let f = self.model_group(compositor, &particles);
                self.visiting.pop();
                f
            }
        }
    }

    fn model_group(&mut self, compositor: Compositor, particles: &[ParticleId]) -> Frag {
        match compositor {
            Compositor::Sequence => {
                let mut acc = Frag::empty();
                for &p in particles {
                    let f = self.particle(p);
                    acc = self.concat(acc, f, false);
                }
                acc
            }
            Compositor::Choice => {
                if particles.is_empty() {
                    // An empty choice matches nothing at all, not the empty
                    // sequence.
                    return Frag {
                        first: Vec::new(),
                        last: Vec::new(),
                        nullable: false,
                    };
                }
                let frags = particles.iter().map(|&p| self.particle(p)).collect();
                self.union(frags)
            }
            Compositor::All => {
                // A nested `xs:all` is only legal in XSD 1.1. Over-approximate
                // it as a repeated choice so the model stays usable, and mark
                // the result approximate.
                self.approximated = true;
                let frags: Vec<Frag> = particles.iter().map(|&p| self.particle(p)).collect();
                let f = self.union(frags);
                let (first, last) = (f.first.clone(), f.last.clone());
                self.link(&last, &first);
                f
            }
        }
    }
}

fn extend_unique(dst: &mut Vec<PositionId>, src: &[PositionId]) {
    for &s in src {
        if !dst.contains(&s) {
            dst.push(s);
        }
    }
}

/// Compiles the content model of every complex type.
///
/// Runs against a fully resolved [`Schemas`], so it can expand substitution
/// groups while building.
pub(crate) fn build_all(
    schemas: &Schemas,
    mode: crate::load::Conformance,
) -> (Vec<Option<ContentModel>>, Diagnostics) {
    let mut models = vec![None; schemas.component_counts().types];
    let mut diags = Diagnostics::new();

    for (id, def) in schemas.iter_types() {
        let TypeDefinition::Complex(_) = def else {
            continue;
        };
        let particles = effective_particles(schemas, id);
        models[id.index()] = Some(if particles.is_empty() {
            ContentModel::Empty
        } else {
            build_one(schemas, &particles)
        });
    }

    for (id, def) in schemas.iter_types() {
        let Some(model) = &models[id.index()] else {
            continue;
        };
        if let ContentModel::Automaton(a) = model {
            check_upa(schemas, def, a, mode, &mut diags);
        }
    }

    (models, diags)
}

/// The particles making up a type's effective content model, outermost base
/// first.
///
/// Extension appends to the base's content, so the chain is walked upwards
/// and reversed. Restriction states the model in full, so the walk stops
/// there — an ancestor's particles are not part of a restricting type's
/// content.
fn effective_particles(schemas: &Schemas, id: TypeId) -> Vec<ParticleId> {
    let mut chain = Vec::new();
    let mut cur = id;
    let mut guard = 0usize;
    loop {
        chain.push(cur);
        let Some(c) = schemas[cur].as_complex() else {
            break;
        };
        if c.derivation == DerivationMethod::Restriction {
            break;
        }
        let base = c.base;
        guard += 1;
        if base == cur || base.is_placeholder() || guard > schemas.component_counts().types {
            break;
        }
        cur = base;
    }
    chain.reverse();
    chain
        .into_iter()
        .filter_map(|t| schemas[t].as_complex()?.content.particle())
        .collect()
}

fn build_one(schemas: &Schemas, particles: &[ParticleId]) -> ContentModel {
    // A lone top-level `xs:all` occurring at most once gets member counters
    // rather than an automaton. Extending an `xs:all` is not legal, so this
    // only applies when it is the whole model.
    if let [only] = particles {
        let p = &schemas[*only];
        if !p.is_repeating() {
            let group = match &p.term {
                Term::Group(g) => Some(g),
                Term::GroupRef(gid) => Some(&schemas[*gid].group),
                _ => None,
            };
            if let Some(g) = group {
                if g.compositor == Compositor::All {
                    return ContentModel::All(build_all_group(schemas, &g.particles));
                }
            }
        }
    }

    let mut b = Builder::new(schemas);
    let mut frag = Frag::empty();
    for &p in particles {
        let f = b.particle(p);
        frag = b.concat(frag, f, false);
    }
    ContentModel::Automaton(ContentAutomaton {
        positions: b.positions,
        first: frag.first,
        last: frag.last,
        nullable: frag.nullable,
        follow: b.follow,
        approximated: b.approximated,
    })
}

fn build_all_group(schemas: &Schemas, particles: &[ParticleId]) -> AllGroup {
    let mut members = Vec::new();
    for &pid in particles {
        let p = &schemas[pid];
        let label = match &p.term {
            Term::Element(e) => Label::Element(*e),
            Term::Wildcard(_) => Label::Wildcard,
            // Only element and wildcard particles are legal inside xs:all.
            _ => continue,
        };
        let admits = match &label {
            Label::Element(e) => schemas
                .substitution_closure(*e)
                .into_iter()
                .map(|m| schemas[m].name)
                .collect(),
            Label::Wildcard => Vec::new(),
        };
        members.push(AllMember {
            particle: pid,
            label,
            admits,
            min_occurs: p.min_occurs,
            max_occurs: p.max_occurs,
        });
    }
    AllGroup { members }
}

// ---------------------------------------------------------------------------
// Unique Particle Attribution
// ---------------------------------------------------------------------------

/// Reports states with two out-transitions that could both match one element.
///
/// This is the whole of UPA: a content model is 1-unambiguous exactly when
/// its position automaton is deterministic.
fn check_upa(
    schemas: &Schemas,
    def: &TypeDefinition,
    a: &ContentAutomaton,
    mode: crate::load::Conformance,
    diags: &mut Diagnostics,
) {
    let mut reported: FxHashSet<(ParticleId, ParticleId)> = FxHashSet::default();

    let mut check_state = |targets: &[PositionId], diags: &mut Diagnostics| {
        for i in 0..targets.len() {
            for j in (i + 1)..targets.len() {
                let (p, q) = (a.position(targets[i]), a.position(targets[j]));
                if p.particle == q.particle {
                    // Two unrolled copies of one particle are a chain, not a
                    // choice; they can never be confused for each other.
                    continue;
                }
                let Some(overlap) = overlap(schemas, p, q) else {
                    continue;
                };
                let key = if p.particle < q.particle {
                    (p.particle, q.particle)
                } else {
                    (q.particle, p.particle)
                };
                if !reported.insert(key) {
                    continue;
                }
                diags.push(upa_diagnostic(
                    schemas,
                    def,
                    p,
                    q,
                    overlap,
                    a.approximated(),
                    mode,
                ));
            }
        }
    };

    check_state(a.first(), diags);
    for p in 0..a.positions().len() as PositionId {
        let targets = a.follow(p).to_vec();
        check_state(&targets, diags);
    }
}

/// What the two positions have in common, if anything.
enum Overlap {
    /// Both are element particles admitting this name.
    Name(QName),
    /// An element particle competes with a wildcard.
    ElementAndWildcard(QName),
    /// Two wildcards admit an overlapping namespace set.
    Wildcards,
}

fn overlap(schemas: &Schemas, p: &Position, q: &Position) -> Option<Overlap> {
    match (&p.label, &q.label) {
        (Label::Element(_), Label::Element(_)) => {
            let rhs: FxHashSet<QName> = q.admits.iter().copied().collect();
            p.admits
                .iter()
                .find(|n| rhs.contains(n))
                .map(|n| Overlap::Name(*n))
        }
        (Label::Element(_), Label::Wildcard) => {
            wildcard_of(schemas, q).and_then(|w| first_admitted(w, &p.admits))
        }
        (Label::Wildcard, Label::Element(_)) => {
            wildcard_of(schemas, p).and_then(|w| first_admitted(w, &q.admits))
        }
        (Label::Wildcard, Label::Wildcard) => {
            let (Some(a), Some(b)) = (wildcard_of(schemas, p), wildcard_of(schemas, q)) else {
                return None;
            };
            wildcards_overlap(a, b).then_some(Overlap::Wildcards)
        }
    }
}

fn wildcard_of<'a>(schemas: &'a Schemas, p: &Position) -> Option<&'a Wildcard> {
    match &schemas[p.particle].term {
        Term::Wildcard(w) => Some(w),
        _ => None,
    }
}

fn first_admitted(w: &Wildcard, names: &[QName]) -> Option<Overlap> {
    names
        .iter()
        .find(|n| w.namespace.admits(n.ns) && !w.not_qname.contains(n))
        .map(|n| Overlap::ElementAndWildcard(*n))
}

/// Whether two wildcards can both match some name.
///
/// Exact for the cases that arise in practice; errs towards reporting an
/// overlap, since a missed UPA violation is worse than a spurious warning
/// the schema author can silence with `Conformance::Lax`.
fn wildcards_overlap(a: &Wildcard, b: &Wildcard) -> bool {
    use NamespaceConstraint::*;
    match (&a.namespace, &b.namespace) {
        (Any, _) | (_, Any) => true,
        (Enumeration(x), Enumeration(y)) => x.iter().any(|n| y.contains(n)),
        (Enumeration(x), Not(y)) | (Not(y), Enumeration(x)) => x.iter().any(|n| !y.contains(n)),
        // Two `##other`s both admit anything outside their exclusion lists,
        // and those lists are finite.
        (Not(_), Not(_)) => true,
    }
}

fn upa_diagnostic(
    schemas: &Schemas,
    def: &TypeDefinition,
    p: &Position,
    q: &Position,
    overlap: Overlap,
    approximated: bool,
    mode: crate::load::Conformance,
) -> Diagnostic {
    let owner = def
        .name()
        .map(|n| format!("`{}`", schemas.display_name(n)))
        .unwrap_or_else(|| "an anonymous type".to_string());

    let (what, help) = match overlap {
        Overlap::Name(n) => (
            format!("`{}` could match either particle", schemas.display_name(n)),
            "make the choice explicit, or merge the two particles".to_string(),
        ),
        Overlap::ElementAndWildcard(n) => (
            format!(
                "`{}` could match either the element particle or the wildcard",
                schemas.display_name(n)
            ),
            "XSD 1.1 resolves this in favour of the element particle; XSD 1.0 rejects it"
                .to_string(),
        ),
        Overlap::Wildcards => (
            "two wildcards admit overlapping namespaces".to_string(),
            "narrow one wildcard's `namespace` constraint".to_string(),
        ),
    };

    let mut d = Diagnostic::error(
        DiagCode::AmbiguousContentModel,
        format!("the content model of {owner} is ambiguous: {what}"),
    )
    .at(Span::labelled(
        schemas[p.particle].span.uri.clone(),
        schemas[p.particle].span.line,
        "one candidate",
    ))
    .at(Span::labelled(
        schemas[q.particle].span.uri.clone(),
        schemas[q.particle].span.line,
        "the other",
    ))
    .with_help(help);

    if approximated {
        // The model was widened to stay within the position budget, so this
        // could be an artefact of that widening rather than a real breach.
        d.severity = Severity::Warning;
        d.message
            .push_str(" (occurrence ranges were widened; verdict is approximate)");
    } else if mode == crate::load::Conformance::Lax {
        // UPA breaches ship in real schemas often enough that rejecting them
        // outright would make lax mode useless.
        d.severity = Severity::Warning;
    }
    d
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// Walks a content model over a sequence of child element names.
///
/// Simulates the automaton as an NFA rather than determinising it, so a model
/// that breaches UPA still matches — which is what `Conformance::Lax` needs.
#[derive(Debug)]
pub struct ContentMatcher<'a> {
    schemas: &'a Schemas,
    model: &'a ContentModel,
    /// Active positions, for an automaton model.
    active: Vec<PositionId>,
    /// Per-member counts, for an `xs:all` model.
    counts: Vec<u32>,
    started: bool,
    failed: bool,
}

impl<'a> ContentMatcher<'a> {
    pub fn new(schemas: &'a Schemas, model: &'a ContentModel) -> Self {
        let counts = match model {
            ContentModel::All(g) => vec![0; g.members.len()],
            _ => Vec::new(),
        };
        Self {
            schemas,
            model,
            active: Vec::new(),
            counts,
            started: false,
            failed: false,
        }
    }

    /// Consumes one child element. Returns `false` once the sequence has
    /// become invalid; further calls keep returning `false`.
    pub fn step(&mut self, name: QName) -> bool {
        if self.failed {
            return false;
        }
        let ok = match self.model {
            ContentModel::Empty => false,
            ContentModel::Automaton(a) => self.step_automaton(a, name),
            ContentModel::All(g) => Self::step_all(self.schemas, g, &mut self.counts, name),
        };
        if !ok {
            self.failed = true;
        }
        ok
    }

    fn step_automaton(&mut self, a: &ContentAutomaton, name: QName) -> bool {
        let candidates: Vec<PositionId> = if self.started {
            self.active
                .iter()
                .flat_map(|&p| a.follow(p).iter().copied())
                .collect()
        } else {
            a.first().to_vec()
        };
        let mut next = Vec::new();
        for c in candidates {
            if admits(self.schemas, a.position(c), name) && !next.contains(&c) {
                next.push(c);
            }
        }
        self.started = true;
        self.active = next;
        !self.active.is_empty()
    }

    fn step_all(schemas: &Schemas, g: &AllGroup, counts: &mut [u32], name: QName) -> bool {
        for (i, m) in g.members.iter().enumerate() {
            let matches = match &m.label {
                Label::Element(_) => m.admits.contains(&name),
                Label::Wildcard => match &schemas[m.particle].term {
                    Term::Wildcard(w) => {
                        w.namespace.admits(name.ns) && !w.not_qname.contains(&name)
                    }
                    _ => false,
                },
            };
            if !matches {
                continue;
            }
            let room = match m.max_occurs {
                MaxOccurs::Unbounded => true,
                MaxOccurs::Bounded(n) => counts[i] < n,
            };
            if room {
                counts[i] += 1;
                return true;
            }
        }
        false
    }

    /// Whether the content seen so far is a complete, valid match.
    pub fn accepts_end(&self) -> bool {
        if self.failed {
            return false;
        }
        match self.model {
            ContentModel::Empty => !self.started,
            ContentModel::Automaton(a) => {
                if !self.started {
                    return a.is_nullable();
                }
                self.active.iter().any(|p| a.last().contains(p))
            }
            ContentModel::All(g) => g
                .members
                .iter()
                .enumerate()
                .all(|(i, m)| self.counts[i] >= m.min_occurs),
        }
    }
}

fn admits(schemas: &Schemas, p: &Position, name: QName) -> bool {
    match &p.label {
        Label::Element(_) => p.admits.contains(&name),
        Label::Wildcard => match &schemas[p.particle].term {
            Term::Wildcard(w) => w.namespace.admits(name.ns) && !w.not_qname.contains(&name),
            _ => false,
        },
    }
}

// ---------------------------------------------------------------------------
// Queries built on the automaton
// ---------------------------------------------------------------------------

impl Schemas {
    /// The compiled content model of a complex type.
    ///
    /// `None` for simple types.
    pub fn content_model(&self, id: TypeId) -> Option<&ContentModel> {
        self.content_models.get(id.index())?.as_ref()
    }

    /// Starts matching a sequence of children against a type's content model.
    pub fn match_content(&self, id: TypeId) -> Option<ContentMatcher<'_>> {
        self.content_model(id).map(|m| ContentMatcher::new(self, m))
    }

    /// Every element that may appear directly inside this type, with
    /// substitution groups expanded.
    pub fn possible_children(&self, id: TypeId) -> Vec<ElementId> {
        self.content_model(id)
            .map(|m| m.admitted_elements(self))
            .unwrap_or_default()
    }

    /// Whether `child` may appear more than once inside `parent`.
    ///
    /// True when a position admitting it can reach itself, which covers both
    /// `maxOccurs > 1` on the element and a repeating ancestor group. This is
    /// the table-versus-column question a config generator asks.
    pub fn child_repeats(&self, parent: TypeId, child: ElementId) -> bool {
        let Some(model) = self.content_model(parent) else {
            return false;
        };
        let name = self[child].name;
        match model {
            ContentModel::Empty => false,
            ContentModel::All(g) => g
                .members
                .iter()
                .any(|m| m.admits.contains(&name) && m.max_occurs.is_repeating()),
            ContentModel::Automaton(a) => a
                .positions()
                .iter()
                .enumerate()
                .filter(|(_, p)| p.admits.contains(&name))
                .any(|(i, p)| self[p.particle].is_repeating() || a.repeats(i as PositionId)),
        }
    }

    /// Whether `child` may be absent from `parent`, making a column derived
    /// from it nullable.
    pub fn child_is_optional(&self, parent: TypeId, child: ElementId) -> bool {
        let Some(model) = self.content_model(parent) else {
            return true;
        };
        let name = self[child].name;
        match model {
            ContentModel::Empty => true,
            ContentModel::All(g) => g
                .members
                .iter()
                .filter(|m| m.admits.contains(&name))
                .all(|m| m.min_occurs == 0),
            ContentModel::Automaton(a) => {
                // Optional unless every path to the end passes through it.
                let required: Vec<PositionId> = a
                    .positions()
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.admits.contains(&name))
                    .map(|(i, _)| i as PositionId)
                    .collect();
                if required.is_empty() {
                    return true;
                }
                // Optional exactly when some accepting path skips it.
                reaches_end_avoiding(a, &required)
            }
        }
    }
}

/// Whether the automaton has an accepting path that touches none of
/// `avoid` — i.e. whether the element those positions carry is skippable.
fn reaches_end_avoiding(a: &ContentAutomaton, avoid: &[PositionId]) -> bool {
    let avoid: FxHashSet<PositionId> = avoid.iter().copied().collect();
    if a.is_nullable() {
        return true;
    }
    let mut seen = FxHashSet::default();
    let mut stack: Vec<PositionId> = a
        .first()
        .iter()
        .copied()
        .filter(|p| !avoid.contains(p))
        .collect();
    while let Some(p) = stack.pop() {
        if !seen.insert(p) {
            continue;
        }
        if a.last().contains(&p) {
            return true;
        }
        stack.extend(a.follow(p).iter().copied().filter(|q| !avoid.contains(q)));
    }
    false
}

/// Content-model statistics, for diagnostics and tests.
#[derive(Copy, Clone, Default, PartialEq, Eq, Debug)]
pub struct ContentStats {
    pub models: usize,
    pub automata: usize,
    pub all_groups: usize,
    pub empty: usize,
    pub positions: usize,
    pub approximated: usize,
}

impl Schemas {
    pub fn content_stats(&self) -> ContentStats {
        let mut s = ContentStats::default();
        for m in self.content_models.iter().flatten() {
            s.models += 1;
            match m {
                ContentModel::Empty => s.empty += 1,
                ContentModel::All(_) => s.all_groups += 1,
                ContentModel::Automaton(a) => {
                    s.automata += 1;
                    s.positions += a.positions().len();
                    if a.approximated() {
                        s.approximated += 1;
                    }
                }
            }
        }
        s
    }
}

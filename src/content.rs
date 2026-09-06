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
//! The last two are the primitives a table-versus-column split is built on,
//! for anything mapping a schema onto a relational or columnar shape.
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
use fxhash::{FxHashMap, FxHashSet};

/// Cap on positions per content model, bounding total model size.
///
/// This one is generous: a flat sequence of hundreds of distinct elements is
/// perfectly ordinary and must not be truncated.
///
/// Not API: an artefact of compiling content models as Glushkov position
/// automata, hidden so the strategy stays changeable. [`ContentMatcher`] is
/// the supported way to ask what a content model accepts.
#[doc(hidden)]
pub const MAX_POSITIONS: usize = 4096;

/// Cap on unrolled copies of a single particle.
///
/// Deliberately far tighter than [`MAX_POSITIONS`], because unrolling is
/// quadratic in the copy count while a flat model is linear in its size.
///
/// Not API: an artefact of compiling content models as Glushkov position
/// automata, hidden so the strategy stays changeable. [`ContentMatcher`] is
/// the supported way to ask what a content model accepts.
#[doc(hidden)]
pub const MAX_UNROLL: u32 = 64;

/// Index of a position within one [`ContentAutomaton`].
///
/// Not API: an artefact of compiling content models as Glushkov position
/// automata, hidden so the strategy stays changeable. [`ContentMatcher`] is
/// the supported way to ask what a content model accepts.
#[doc(hidden)]
pub type PositionId = u32;

/// What a position matches.
///
/// Not API: an artefact of compiling content models as Glushkov position
/// automata, hidden so the strategy stays changeable. [`ContentMatcher`] is
/// the supported way to ask what a content model accepts.
#[doc(hidden)]
#[derive(Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Label {
    /// An element particle. Admits the element itself plus every member of
    /// its substitution group, so the admitted set is usually larger than
    /// one name.
    Element(ElementId),
    /// A wildcard particle; the constraint lives on the particle's term.
    Wildcard,
}

/// One occurrence of an element or wildcard in a content model.
///
/// Not API: an artefact of compiling content models as Glushkov position
/// automata, hidden so the strategy stays changeable. [`ContentMatcher`] is
/// the supported way to ask what a content model accepts.
#[doc(hidden)]
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Position {
    /// The particle this position came from. Several positions share one
    /// particle when a numeric range was unrolled.
    pub particle: ParticleId,
    pub label: Label,
    /// Every element declaration this position admits — the substitution
    /// closure, precomputed. Empty for wildcards, whose admission is a
    /// predicate rather than a set.
    ///
    /// Declarations rather than names because a validator needs the one that
    /// matched: a substituting element has its own type.
    pub admits: Vec<ElementId>,
}

/// A Glushkov position automaton over a content model.
///
/// Not API: an artefact of compiling content models as Glushkov position
/// automata, hidden so the strategy stays changeable. [`ContentMatcher`] is
/// the supported way to ask what a content model accepts.
#[doc(hidden)]
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AllGroup {
    pub members: Vec<AllMember>,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AllMember {
    pub particle: ParticleId,
    pub label: Label,
    pub admits: Vec<ElementId>,
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ContentModel {
    /// No child elements are permitted.
    Empty,
    Automaton(ContentAutomaton),
    All(AllGroup),
}

/// A content model together with any XSD 1.1 open content around it.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Content {
    pub model: ContentModel,
    /// Every element name written out in this content model, which is what
    /// `notQName="##definedSibling"` excludes.
    ///
    /// Resolved here rather than at load time because "the content model this
    /// wildcard sits in" is only settled once group references are expanded
    /// and an extension's base is folded in. Names, not declarations: a
    /// substitution group member is admitted through its head rather than
    /// named, so it is not a sibling.
    #[cfg_attr(feature = "serde", serde(with = "crate::names::set_as_seq"))]
    pub siblings: FxHashSet<QName>,
    /// The declaration each of those names carries.
    ///
    /// For the XSD 1.1 *dynamic* Element Declarations Consistent check: a
    /// wildcard may admit a name this model also declares, and if the two
    /// declarations disagree about the type then no document can satisfy the
    /// model. 1.0 rejected such a schema outright; 1.1 accepts it and reports
    /// the clash only when a document actually walks into it, which is why
    /// this has to be answerable at validation time.
    #[cfg_attr(feature = "serde", serde(with = "crate::names::map_as_seq"))]
    pub sibling_decls: FxHashMap<QName, ElementId>,
    /// Kept beside the model rather than compiled into it: interleaved open
    /// content is the *shuffle* of the declared language with the wildcard's,
    /// which a position automaton cannot express — but a matcher decides it
    /// in one extra check.
    pub open: Option<OpenContent>,
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
    pub fn admitted_elements(&self) -> Vec<ElementId> {
        let mut out = Vec::new();
        let mut seen = FxHashSet::default();
        let mut push = |admits: &[ElementId]| {
            for m in admits {
                if seen.insert(*m) {
                    out.push(*m);
                }
            }
        };
        match self {
            ContentModel::Empty => {}
            ContentModel::Automaton(a) => a.positions.iter().for_each(|p| push(&p.admits)),
            ContentModel::All(a) => a.members.iter().for_each(|m| push(&m.admits)),
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
            Label::Element(e) => self.schemas.permitted_substitutes(*e),
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
    version: crate::load::Version,
) -> (Vec<Option<Content>>, Diagnostics) {
    let mut models: Vec<Option<Content>> = vec![None; schemas.component_counts().types];
    let mut diags = Diagnostics::new();

    for (id, def) in schemas.iter_types() {
        let TypeDefinition::Complex(_) = def else {
            continue;
        };
        let particles = effective_particles(schemas, id);
        let model = if particles.is_empty() {
            ContentModel::Empty
        } else {
            build_one(schemas, &particles)
        };
        let open = schemas[id]
            .as_complex()
            .and_then(|c| c.open_content.clone());
        models[id.index()] = Some(Content {
            model,
            open,
            siblings: named_elements(schemas, &particles),
            sibling_decls: named_declarations(schemas, &particles),
        });
    }

    for (id, def) in schemas.iter_types() {
        let Some(content) = &models[id.index()] else {
            continue;
        };
        if let ContentModel::Automaton(a) = &content.model {
            check_upa(
                schemas,
                def,
                a,
                &content.siblings,
                mode,
                version,
                &mut diags,
            );
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
pub(crate) fn effective_particles(schemas: &Schemas, id: TypeId) -> Vec<ParticleId> {
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
            Label::Element(e) => schemas.permitted_substitutes(*e),
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
    siblings: &FxHashSet<QName>,
    mode: crate::load::Conformance,
    version: crate::load::Version,
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
                let Some(overlap) = overlap(schemas, p, q, siblings) else {
                    continue;
                };
                // XSD 1.1 resolves an element competing with a wildcard in
                // favour of the element, so it is no longer ambiguous.
                if version == crate::load::Version::Xsd11
                    && matches!(overlap, Overlap::ElementAndWildcard(_))
                {
                    continue;
                }
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

fn overlap(
    schemas: &Schemas,
    p: &Position,
    q: &Position,
    siblings: &FxHashSet<QName>,
) -> Option<Overlap> {
    match (&p.label, &q.label) {
        (Label::Element(_), Label::Element(_)) => {
            // By *name*, not by declaration identity. Two distinct local
            // declarations that share a name are precisely what makes a
            // content model ambiguous.
            let rhs: FxHashSet<QName> = q.admits.iter().map(|e| schemas[*e].name).collect();
            p.admits
                .iter()
                .map(|e| schemas[*e].name)
                .find(|n| rhs.contains(n))
                .map(Overlap::Name)
        }
        (Label::Element(_), Label::Wildcard) => {
            wildcard_of(schemas, q).and_then(|w| first_admitted(schemas, w, &p.admits, siblings))
        }
        (Label::Wildcard, Label::Element(_)) => {
            wildcard_of(schemas, p).and_then(|w| first_admitted(schemas, w, &q.admits, siblings))
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

/// Whether a wildcard admits `name`, given the names its content model writes
/// out.
///
/// The namespace decides first, then the three exclusions XSD 1.1 added.
/// `##defined` asks the schema and `##definedSibling` asks the model, which is
/// why this needs both and cannot live on `Wildcard`.
pub(crate) fn wildcard_admits(
    schemas: &Schemas,
    w: &Wildcard,
    name: QName,
    siblings: &FxHashSet<QName>,
) -> bool {
    w.namespace.admits(name.ns)
        && !w.not_qname.contains(&name)
        && !(w.not_defined_sibling && siblings.contains(&name))
        && !(w.not_defined && schemas.globals().elements.contains_key(&name))
}

/// The declaration each written-out name carries, for the dynamic Element
/// Declarations Consistent check.
///
/// A name written twice with two declarations is exactly the clash the check
/// looks for, so the first is kept and the walk does not merge them.
fn named_declarations(schemas: &Schemas, particles: &[ParticleId]) -> FxHashMap<QName, ElementId> {
    fn walk(
        schemas: &Schemas,
        p: ParticleId,
        out: &mut FxHashMap<QName, ElementId>,
        seen: &mut FxHashSet<GroupId>,
    ) {
        match &schemas[p].term {
            Term::Element(e) if !e.is_placeholder() => {
                out.entry(schemas[*e].name).or_insert(*e);
            }
            Term::Group(g) => {
                for c in &g.particles {
                    walk(schemas, *c, out, seen);
                }
            }
            Term::GroupRef(gid) if !gid.is_placeholder() && seen.insert(*gid) => {
                for c in &schemas[*gid].group.particles {
                    walk(schemas, *c, out, seen);
                }
            }
            _ => {}
        }
    }
    let mut out = FxHashMap::default();
    let mut seen = FxHashSet::default();
    for p in particles {
        walk(schemas, *p, &mut out, &mut seen);
    }
    out
}

/// The element names a set of particles writes out, with group references
/// expanded.
fn named_elements(schemas: &Schemas, particles: &[ParticleId]) -> FxHashSet<QName> {
    fn walk(
        schemas: &Schemas,
        p: ParticleId,
        out: &mut FxHashSet<QName>,
        seen: &mut FxHashSet<GroupId>,
    ) {
        match &schemas[p].term {
            Term::Element(e) if !e.is_placeholder() => {
                out.insert(schemas[*e].name);
            }
            Term::Group(g) => {
                for c in &g.particles {
                    walk(schemas, *c, out, seen);
                }
            }
            // A group that reaches itself is rejected elsewhere; the guard
            // here only needs to make this walk terminate.
            Term::GroupRef(gid) if !gid.is_placeholder() && seen.insert(*gid) => {
                for c in &schemas[*gid].group.particles {
                    walk(schemas, *c, out, seen);
                }
            }
            _ => {}
        }
    }

    let mut out = FxHashSet::default();
    let mut seen = FxHashSet::default();
    for p in particles {
        walk(schemas, *p, &mut out, &mut seen);
    }
    out
}

fn first_admitted(
    schemas: &Schemas,
    w: &Wildcard,
    elements: &[ElementId],
    siblings: &FxHashSet<QName>,
) -> Option<Overlap> {
    elements
        .iter()
        .map(|e| schemas[*e].name)
        .find(|n| wildcard_admits(schemas, w, *n, siblings))
        .map(Overlap::ElementAndWildcard)
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
    open: Option<&'a OpenContent>,
    /// The names this model writes out, for `notQName="##definedSibling"`.
    siblings: &'a FxHashSet<QName>,
    /// Their declarations, for the dynamic Element Declarations Consistent
    /// check.
    sibling_decls: &'a FxHashMap<QName, ElementId>,
    /// Active positions, for an automaton model.
    active: Vec<PositionId>,
    /// The declaration the last successful `step` matched, if it named one.
    matched: Option<ElementId>,
    /// How the wildcard that admitted the last child says it must be
    /// processed, when a wildcard admitted it rather than a declaration.
    matched_wildcard: Option<ProcessContents>,
    /// Per-member counts, for an `xs:all` model.
    counts: Vec<u32>,
    started: bool,
    failed: bool,
}

impl<'a> ContentMatcher<'a> {
    pub fn new(schemas: &'a Schemas, content: &'a Content) -> Self {
        let model = &content.model;
        let siblings = &content.siblings;
        let counts = match model {
            ContentModel::All(g) => vec![0; g.members.len()],
            _ => Vec::new(),
        };
        Self {
            schemas,
            model,
            siblings,
            open: content.open.as_ref(),
            sibling_decls: &content.sibling_decls,
            active: Vec::new(),
            matched: None,
            matched_wildcard: None,
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
        self.matched_wildcard = None;
        let ok = match self.model {
            ContentModel::Empty => false,
            ContentModel::Automaton(a) => self.step_automaton(a, name),
            ContentModel::All(g) => Self::step_all(
                self.schemas,
                g,
                &mut self.counts,
                name,
                self.siblings,
                &mut self.matched,
                &mut self.matched_wildcard,
            ),
        };
        if ok {
            return true;
        }
        // The declared model rejected it; XSD 1.1 open content may still
        // admit it, *without* advancing the model — which is exactly what
        // makes interleaved open content a shuffle rather than a sequence.
        if self.open_admits(name) {
            self.matched = None;
            self.matched_wildcard = self.open.map(|o| o.wildcard.process_contents);
            return true;
        }
        self.failed = true;
        false
    }

    /// Whether open content admits an already-known name at this point.
    fn open_admits(&self, name: QName) -> bool {
        let Some(open) = self.open else { return false };
        if !wildcard_admits(self.schemas, &open.wildcard, name, self.siblings) {
            return false;
        }
        match open.mode {
            OpenContentMode::Interleave => true,
            // Suffix content is only legal once the declared model is
            // satisfied, so ask it.
            OpenContentMode::Suffix => self.model_accepts_end(),
        }
    }

    fn model_accepts_end(&self) -> bool {
        match self.model {
            ContentModel::Empty => !self.started,
            ContentModel::Automaton(a) => {
                if !self.started {
                    a.is_nullable()
                } else {
                    self.active.iter().any(|p| a.last().contains(p))
                }
            }
            ContentModel::All(g) => g
                .members
                .iter()
                .enumerate()
                .all(|(i, m)| self.counts[i] >= m.min_occurs),
        }
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
        let mut matched = None;
        let mut wildcard = None;
        for c in candidates {
            if let Some(decl) = admits(self.schemas, a.position(c), name, self.siblings) {
                if !next.contains(&c) {
                    next.push(c);
                }
                // On a model that breaches UPA several positions may accept.
                // The first wins, which is what a processor would have done
                // had the model been unambiguous.
                if matched.is_none() {
                    matched = decl;
                }
                if wildcard.is_none() && decl.is_none() {
                    if let Term::Wildcard(w) = &self.schemas[a.position(c).particle].term {
                        wildcard = Some(w.process_contents);
                    }
                }
            }
        }
        if next.is_empty() {
            // Leave the model where it was. Since open content can accept a
            // child the declared model rejects, a failed step must not have
            // consumed the position the next one needs.
            return false;
        }
        self.started = true;
        self.active = next;
        self.matched = matched;
        // Only the wildcard's word when no declaration named the child.
        self.matched_wildcard = matched.is_none().then_some(wildcard).flatten();
        true
    }

    fn step_all(
        schemas: &Schemas,
        g: &AllGroup,
        counts: &mut [u32],
        name: QName,
        siblings: &FxHashSet<QName>,
        matched: &mut Option<ElementId>,
        wildcard: &mut Option<ProcessContents>,
    ) -> bool {
        for (i, m) in g.members.iter().enumerate() {
            let decl = match &m.label {
                Label::Element(_) => m
                    .admits
                    .iter()
                    .find(|e| schemas[**e].name == name)
                    .map(|e| Some(*e)),
                Label::Wildcard => match &schemas[m.particle].term {
                    Term::Wildcard(w) if wildcard_admits(schemas, w, name, siblings) => Some(None),
                    _ => None,
                },
            };
            let Some(decl) = decl else { continue };
            let room = match m.max_occurs {
                MaxOccurs::Unbounded => true,
                MaxOccurs::Bounded(n) => counts[i] < n,
            };
            if room {
                counts[i] += 1;
                *matched = decl;
                if decl.is_none() {
                    if let Term::Wildcard(w) = &schemas[m.particle].term {
                        *wildcard = Some(w.process_contents);
                    }
                }
                return true;
            }
        }
        false
    }

    /// Consumes a child whose name the schema does not declare.
    ///
    /// Only a wildcard can admit such a name — which is the point of
    /// wildcards — so element positions are skipped entirely.
    pub fn step_foreign(&mut self, ns_uri: Option<&str>, local: &str) -> bool {
        if self.failed {
            return false;
        }
        self.matched_wildcard = None;
        let ok = match self.model {
            ContentModel::Empty => false,
            ContentModel::Automaton(a) => self.step_automaton_foreign(a, ns_uri, local),
            ContentModel::All(g) => Self::step_all_foreign(
                self.schemas,
                g,
                &mut self.counts,
                ns_uri,
                local,
                &mut self.matched_wildcard,
            ),
        };
        if ok {
            self.matched = None;
            return true;
        }
        if self.open_admits_uri(ns_uri, local) {
            self.matched = None;
            self.matched_wildcard = self.open.map(|o| o.wildcard.process_contents);
            return true;
        }
        self.failed = true;
        false
    }

    fn step_automaton_foreign(
        &mut self,
        a: &ContentAutomaton,
        ns_uri: Option<&str>,
        local: &str,
    ) -> bool {
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
            let pos = a.position(c);
            if !matches!(pos.label, Label::Wildcard) {
                continue;
            }
            let Term::Wildcard(w) = &self.schemas[pos.particle].term else {
                continue;
            };
            if w.namespace.admits_uri(self.schemas.names(), ns_uri)
                && !excluded(self.schemas, w, ns_uri, local)
                && !next.contains(&c)
            {
                if next.is_empty() {
                    self.matched_wildcard = Some(w.process_contents);
                }
                next.push(c);
            }
        }
        if next.is_empty() {
            return false;
        }
        self.started = true;
        self.active = next;
        true
    }

    fn step_all_foreign(
        schemas: &Schemas,
        g: &AllGroup,
        counts: &mut [u32],
        ns_uri: Option<&str>,
        local: &str,
        wildcard: &mut Option<ProcessContents>,
    ) -> bool {
        for (i, m) in g.members.iter().enumerate() {
            if !matches!(m.label, Label::Wildcard) {
                continue;
            }
            let Term::Wildcard(w) = &schemas[m.particle].term else {
                continue;
            };
            if !w.namespace.admits_uri(schemas.names(), ns_uri)
                || excluded(schemas, w, ns_uri, local)
            {
                continue;
            }
            let room = match m.max_occurs {
                MaxOccurs::Unbounded => true,
                MaxOccurs::Bounded(n) => counts[i] < n,
            };
            if room {
                counts[i] += 1;
                *wildcard = Some(w.process_contents);
                return true;
            }
        }
        false
    }

    fn open_admits_uri(&self, ns_uri: Option<&str>, local: &str) -> bool {
        let Some(open) = self.open else { return false };
        if !open
            .wildcard
            .namespace
            .admits_uri(self.schemas.names(), ns_uri)
            || excluded(self.schemas, &open.wildcard, ns_uri, local)
        {
            return false;
        }
        match open.mode {
            OpenContentMode::Interleave => true,
            OpenContentMode::Suffix => self.model_accepts_end(),
        }
    }

    /// The declaration the last successful [`Self::step`] matched.
    ///
    /// `None` when a wildcard matched, which admits a name without naming a
    /// declaration for it.
    pub fn matched(&self) -> Option<ElementId> {
        self.matched
    }

    /// The declaration this content model writes out for `name`, if any.
    ///
    /// A wildcard that admits a name the model also declares has to agree
    /// with it about the type; this is how the validator asks.
    pub fn sibling_declaration(&self, name: QName) -> Option<ElementId> {
        self.sibling_decls.get(&name).copied()
    }

    /// How the wildcard that admitted the last child says it must be
    /// processed, when it was a wildcard rather than a declaration that
    /// admitted it.
    ///
    /// `skip` means the subtree is not looked at, `lax` validates it against
    /// a global declaration if one exists, and `strict` requires one. Without
    /// this a wildcard is a hole in the document where nothing is checked.
    pub fn matched_wildcard(&self) -> Option<ProcessContents> {
        self.matched_wildcard
    }

    /// Whether the content seen so far is a complete, valid match.
    pub fn accepts_end(&self) -> bool {
        if self.failed {
            return false;
        }
        self.model_accepts_end()
    }
}

/// The declaration this position matches `name` with, if any.
///
/// `Some(None)` means a wildcard matched, which admits a name without naming
/// a declaration for it.
/// Whether a wildcard's XSD 1.1 `notQName` list excludes this name.
fn excluded(schemas: &Schemas, w: &Wildcard, ns_uri: Option<&str>, local: &str) -> bool {
    w.not_qname.iter().any(|&q| {
        let ns_matches = match (schemas.namespace_of(q), ns_uri) {
            (None, None) | (None, Some("")) => true,
            (Some(n), Some(u)) => n == u,
            _ => false,
        };
        ns_matches && schemas.local_of(q) == local
    })
}

fn admits(
    schemas: &Schemas,
    p: &Position,
    name: QName,
    siblings: &FxHashSet<QName>,
) -> Option<Option<ElementId>> {
    match &p.label {
        Label::Element(_) => p
            .admits
            .iter()
            .find(|e| schemas[**e].name == name)
            .map(|e| Some(*e)),
        Label::Wildcard => match &schemas[p.particle].term {
            Term::Wildcard(w) if wildcard_admits(schemas, w, name, siblings) => Some(None),
            _ => None,
        },
    }
}

// ---------------------------------------------------------------------------
// Queries built on the automaton
// ---------------------------------------------------------------------------

impl Schemas {
    /// The compiled content of a complex type: its model plus any XSD 1.1
    /// open content around it.
    ///
    /// `None` for simple types.
    pub fn content(&self, id: TypeId) -> Option<&Content> {
        self.content_models.get(id.index())?.as_ref()
    }

    /// The compiled content model of a complex type, without its open
    /// content.
    pub fn content_model(&self, id: TypeId) -> Option<&ContentModel> {
        Some(&self.content(id)?.model)
    }

    /// Whether a type's content admits character data.
    ///
    /// Not the same as the type's own `mixed` attribute. An extension whose
    /// own content is empty takes the base's content type whole, so
    /// `<xs:extension base="SomethingMixed"><xs:attribute .../></xs:extension>`
    /// is mixed without ever saying so. Restriction states its content in
    /// full, which is where the walk stops — the same rule the effective
    /// particles follow.
    pub fn content_is_mixed(&self, id: TypeId) -> bool {
        let mut cur = id;
        let mut guard = 0usize;
        let limit = self.component_counts().types;
        loop {
            let Some(c) = self[cur].as_complex() else {
                return false;
            };
            match c.content {
                ContentType::Mixed(_) => return true,
                // Empty content of its own is the one case that defers to the
                // base; a particle or simple content settles the question
                // here.
                ContentType::Empty => {}
                _ => return false,
            }
            if c.derivation != DerivationMethod::Extension {
                return false;
            }
            let base = c.base;
            guard += 1;
            if base == cur || base.is_placeholder() || guard > limit {
                return false;
            }
            cur = base;
        }
    }

    /// Starts matching a sequence of children against a type's content.
    pub fn match_content(&self, id: TypeId) -> Option<ContentMatcher<'_>> {
        self.content(id).map(|c| ContentMatcher::new(self, c))
    }

    /// Every element that may appear directly inside this type, with
    /// substitution groups expanded.
    pub fn possible_children(&self, id: TypeId) -> Vec<ElementId> {
        self.content_model(id)
            .map(ContentModel::admitted_elements)
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
        match model {
            ContentModel::Empty => false,
            ContentModel::All(g) => g
                .members
                .iter()
                .any(|m| m.admits.contains(&child) && m.max_occurs.is_repeating()),
            ContentModel::Automaton(a) => a
                .positions()
                .iter()
                .enumerate()
                .filter(|(_, p)| p.admits.contains(&child))
                .any(|(i, p)| self[p.particle].is_repeating() || a.repeats(i as PositionId)),
        }
    }

    /// Whether `child` may be absent from `parent`, making a column derived
    /// from it nullable.
    pub fn child_is_optional(&self, parent: TypeId, child: ElementId) -> bool {
        let Some(model) = self.content_model(parent) else {
            return true;
        };
        match model {
            ContentModel::Empty => true,
            ContentModel::All(g) => g
                .members
                .iter()
                .filter(|m| m.admits.contains(&child))
                .all(|m| m.min_occurs == 0),
            ContentModel::Automaton(a) => {
                // Optional unless every path to the end is forced through it.
                let forced: Vec<PositionId> = a
                    .positions()
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| forces(p, child))
                    .map(|(i, _)| i as PositionId)
                    .collect();
                if forced.is_empty() {
                    return true;
                }
                // Optional exactly when some accepting path skips it.
                reaches_end_avoiding(a, &forced)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Children, answered together
// ---------------------------------------------------------------------------

/// An element that may appear directly inside a type, and what its
/// occurrence allows.
///
/// The two flags are the questions a config or schema generator asks of
/// every child: does it repeat (a table rather than a column), and may it be
/// absent (a nullable column).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Child {
    pub element: ElementId,
    /// Whether it may appear more than once, whether from its own
    /// `maxOccurs` or from a repeating ancestor group.
    pub repeats: bool,
    /// Whether some valid content leaves it out.
    pub optional: bool,
}

/// Which positions lie on a cycle, for the whole automaton at once.
///
/// [`ContentAutomaton::repeats`] answers this one position at a time by
/// walking the follow graph, so asking it for every child walked that graph
/// once per child. A position may repeat exactly when it can reach itself —
/// when it lies in a strongly connected component larger than one, or is its
/// own successor — and Tarjan finds every one of them in a single pass.
fn positions_on_a_cycle(a: &ContentAutomaton) -> Vec<bool> {
    const UNVISITED: u32 = u32::MAX;
    let n = a.positions().len();

    // Glushkov numbers positions in order of appearance, so a model with no
    // repetition has every edge pointing forward. Any cycle needs an edge
    // that does not, and looking for one costs no allocation at all — which
    // is worth having, because most content models are a plain sequence.
    let cyclic_at_all = (0..n as PositionId).any(|p| a.follow(p).iter().any(|&q| q <= p));
    if !cyclic_at_all {
        return vec![false; n];
    }

    let mut on_cycle = vec![false; n];
    let (mut index, mut low) = (vec![UNVISITED; n], vec![0u32; n]);
    let mut on_stack = vec![false; n];
    let mut stack: Vec<PositionId> = Vec::new();
    let mut next = 0u32;
    // Tarjan's recursion made explicit: a model may unroll to
    // `MAX_POSITIONS`, and recursion that deep is a stack overflow rather
    // than a slow path.
    let mut call: Vec<(PositionId, usize)> = Vec::new();

    for root in 0..n as PositionId {
        if index[root as usize] != UNVISITED {
            continue;
        }
        index[root as usize] = next;
        low[root as usize] = next;
        next += 1;
        stack.push(root);
        on_stack[root as usize] = true;
        call.push((root, 0));

        while let Some(&(v, i)) = call.last() {
            let succs = a.follow(v);
            if i < succs.len() {
                call.last_mut().expect("the frame just read").1 += 1;
                let w = succs[i];
                if index[w as usize] == UNVISITED {
                    index[w as usize] = next;
                    low[w as usize] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w as usize] = true;
                    call.push((w, 0));
                } else if on_stack[w as usize] {
                    low[v as usize] = low[v as usize].min(index[w as usize]);
                }
                continue;
            }
            call.pop();
            if let Some(&(parent, _)) = call.last() {
                low[parent as usize] = low[parent as usize].min(low[v as usize]);
            }
            if low[v as usize] != index[v as usize] {
                continue;
            }
            // `v` roots a component; everything above it on the stack is in it.
            let base = stack.len()
                - stack
                    .iter()
                    .rev()
                    .position(|&w| w == v)
                    .expect("a root is on its own stack")
                - 1;
            // A component of one is a cycle only if the position is its own
            // successor, which is what `a+` over a single element gives.
            let cyclic = stack.len() - base > 1 || a.follow(v).contains(&v);
            for &w in &stack[base..] {
                on_stack[w as usize] = false;
                on_cycle[w as usize] = cyclic;
            }
            stack.truncate(base);
        }
    }
    on_cycle
}

/// Which children every accepting path must pass through.
///
/// The per-child question is "does some accepting path avoid it?", and
/// answering it one child at a time re-walks the automaton once per child.
/// Turned around it is a single dataflow: `must[p]`, the children common to
/// every path from the start to `p`, is what `p` forces united with the
/// intersection of `must` over `p`'s predecessors. What every accepting path
/// must contain is then the intersection of `must` over the accepting
/// positions, and a child is optional exactly when it is not in that set.
///
/// The framework is distributive — `f(X) = X ∪ forces(p)` distributes over
/// intersection — so iterating to a fixpoint gives the same answer as
/// enumerating paths. That equivalence is what the differential test pins,
/// against the per-child walk this replaces.
///
/// Bit sets are stored flat, `words` per position in one allocation, because
/// most content models are small enough that a `Vec` per position would cost
/// more than the analysis.
fn required_children(a: &ContentAutomaton, forced: &[u64], words: usize) -> Vec<u64> {
    let n = a.positions().len();
    // Empty content is accepted outright, so nothing is required.
    if a.is_nullable() {
        return vec![0; words];
    }

    // Predecessors, as one flat array with offsets rather than a `Vec` each.
    // Counts are accumulated one slot high and then shifted back down, which
    // is the usual trick for filling a CSR array without a second cursor.
    let mut offset = vec![0u32; n + 2];
    for p in 0..n as PositionId {
        for &q in a.follow(p) {
            offset[q as usize + 2] += 1;
        }
    }
    for i in 0..n {
        offset[i + 2] += offset[i + 1];
    }
    let mut preds = vec![0u32; offset[n + 1] as usize];
    for p in 0..n as PositionId {
        for &q in a.follow(p) {
            preds[offset[q as usize + 1] as usize] = p;
            offset[q as usize + 1] += 1;
        }
    }

    // One byte of state per position: whether the empty path reaches it, and
    // whether its `must` has a value yet.
    const FIRST: u8 = 1;
    const KNOWN: u8 = 2;
    let mut flag = vec![0u8; n];
    // A starting position is reached by the empty path, whose contribution to
    // the meet is nothing at all — so its `must` is its own admits, final,
    // and no predecessor can add to it.
    for &p in a.first() {
        flag[p as usize] |= FIRST;
    }

    let mut must = vec![0u64; n * words];
    // A LIFO worklist: a fixpoint does not care about the order, only that
    // everything whose inputs changed is revisited.
    let mut queue: Vec<PositionId> = Vec::new();
    for &p in a.first() {
        let i = p as usize;
        if flag[i] & KNOWN == 0 {
            must[i * words..(i + 1) * words].copy_from_slice(&forced[i * words..(i + 1) * words]);
            flag[i] |= KNOWN;
            queue.push(p);
        }
    }

    // A predecessor with no value yet stands for "everything", which is the
    // optimistic start an intersection dataflow needs to settle on a cycle at
    // the right answer rather than a smaller one. Values only shrink from
    // there, so this terminates.
    let mut next = vec![0u64; words];
    while let Some(p) = queue.pop() {
        for &q in a.follow(p) {
            let j = q as usize;
            if flag[j] & FIRST != 0 {
                continue;
            }
            let mut seeded = false;
            for &r in &preds[offset[j] as usize..offset[j + 1] as usize] {
                let r = r as usize;
                if flag[r] & KNOWN == 0 {
                    continue;
                }
                let bits = &must[r * words..(r + 1) * words];
                if seeded {
                    for (w, b) in next.iter_mut().zip(bits) {
                        *w &= b;
                    }
                } else {
                    next.copy_from_slice(bits);
                    seeded = true;
                }
            }
            if !seeded {
                next.fill(0);
            }
            for (w, b) in next.iter_mut().zip(&forced[j * words..(j + 1) * words]) {
                *w |= b;
            }
            if flag[j] & KNOWN == 0 || next[..] != must[j * words..(j + 1) * words] {
                must[j * words..(j + 1) * words].copy_from_slice(&next);
                flag[j] |= KNOWN;
                queue.push(q);
            }
        }
    }

    // An automaton with no reachable accepting position accepts nothing, so
    // no content can leave any child out: the empty intersection is
    // everything, which is what an unseeded result means here.
    let mut required = vec![u64::MAX; words];
    let mut seeded = false;
    for &p in a.last() {
        let i = p as usize;
        if flag[i] & KNOWN == 0 {
            continue;
        }
        let bits = &must[i * words..(i + 1) * words];
        if seeded {
            for (w, b) in required.iter_mut().zip(bits) {
                *w &= b;
            }
        } else {
            required.copy_from_slice(bits);
            seeded = true;
        }
    }
    required
}

impl Schemas {
    /// Every element that may appear directly inside this type, with
    /// substitution groups expanded and its occurrence resolved.
    ///
    /// Prefer this to [`Self::possible_children`] followed by
    /// [`Self::child_repeats`] and [`Self::child_is_optional`]: those answer
    /// one child at a time, and each of them re-walks the whole content
    /// model. Over a type with hundreds of children — ordinary in GML, UBL or
    /// WITSML — that is the difference between one pass and several hundred.
    /// The singular forms remain for a one-off question.
    ///
    /// Children come back in the order [`Self::possible_children`] gives.
    pub fn children(&self, ty: TypeId) -> Vec<Child> {
        let Some(model) = self.content_model(ty) else {
            return Vec::new();
        };
        match model {
            ContentModel::Empty => Vec::new(),
            ContentModel::All(g) => {
                let mut out: Vec<Child> = Vec::new();
                let mut at: FxHashMap<ElementId, usize> = FxHashMap::default();
                for m in &g.members {
                    for &e in &m.admits {
                        let i = *at.entry(e).or_insert_with(|| {
                            out.push(Child {
                                element: e,
                                repeats: false,
                                // Every member admitting it has to be
                                // skippable for the child to be.
                                optional: true,
                            });
                            out.len() - 1
                        });
                        out[i].repeats |= m.max_occurs.is_repeating();
                        out[i].optional &= m.min_occurs == 0;
                    }
                }
                out
            }
            ContentModel::Automaton(a) => self.automaton_children(a),
        }
    }

    fn automaton_children(&self, a: &ContentAutomaton) -> Vec<Child> {
        let mut out: Vec<Child> = Vec::new();
        let mut at: FxHashMap<ElementId, usize> = FxHashMap::default();
        for p in a.positions() {
            for &e in &p.admits {
                at.entry(e).or_insert_with(|| {
                    out.push(Child {
                        element: e,
                        repeats: false,
                        optional: false,
                    });
                    out.len() - 1
                });
            }
        }
        if out.is_empty() {
            return out;
        }

        // A position repeats if its own particle does or if it lies on a
        // cycle; every child it admits repeats with it.
        let cyclic = positions_on_a_cycle(a);
        for (i, p) in a.positions().iter().enumerate() {
            if p.admits.is_empty() || !(cyclic[i] || self[p.particle].is_repeating()) {
                continue;
            }
            for &e in &p.admits {
                out[at[&e]].repeats = true;
            }
        }

        // What each position *forces*, which is not what it admits — see
        // `forces`. The dataflow below is about obligation, so a position
        // offering a choice of elements contributes nothing to it.
        let words = out.len().div_ceil(64);
        let mut forced = vec![0u64; a.positions().len() * words];
        for (i, p) in a.positions().iter().enumerate() {
            if let [e] = p.admits[..] {
                let k = at[&e];
                forced[i * words + k / 64] |= 1 << (k % 64);
            }
        }
        let required = required_children(a, &forced, words);
        for (k, child) in out.iter_mut().enumerate() {
            child.optional = required[k / 64] >> (k % 64) & 1 == 0;
        }
        out
    }
}

/// Whether reaching `p` obliges the content to contain `child`.
///
/// Only when `p` admits nothing else. A position stands for one place in the
/// content, and a substitution group puts every member on a single position:
/// reaching it means matching *one* of them, so it forces none of them in
/// particular. Confusing the position with the element made every member of
/// a substitution group look required, even though a document naming only
/// its sibling validates — the shape this crate exists to describe, since
/// GML and UBL are substitution groups almost all the way down.
fn forces(p: &Position, child: ElementId) -> bool {
    p.admits.len() == 1 && p.admits[0] == child
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
    /// Models carrying XSD 1.1 open content.
    pub open_content: usize,
}

impl Schemas {
    pub fn content_stats(&self) -> ContentStats {
        let mut s = ContentStats::default();
        for c in self.content_models.iter().flatten() {
            s.models += 1;
            if c.open.is_some() {
                s.open_content += 1;
            }
            match &c.model {
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

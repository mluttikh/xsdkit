//! Are the facets on a simple type legal facets for that type?
//!
//! Three rules from the datatypes specification, all of which are about the
//! *schema* rather than about any document:
//!
//! 1. **Applicable facets.** Each primitive admits a fixed set. `length` on an
//!    `xs:duration` is not a constraint no document satisfies — it is not a
//!    constraint at all, and the schema is in error.
//! 2. **Facet value validity.** A bound has to be a value of the type it
//!    bounds. `maxInclusive="2005-01-01T00:00:00"` on a restriction of
//!    `xs:dateTimeStamp` names no instant, because a dateTimeStamp requires a
//!    timezone.
//! 3. **Internal consistency.** `minLength` above `maxLength`, `length` beside
//!    either, `fractionDigits` above `totalDigits`: pairs that cannot both
//!    hold.
//!
//! These run against the assembled `Schemas` rather than at load time,
//! because every one of them needs the base type resolved.
//!
//! 4. **Narrowing.** A restriction may only narrow. A bound that widens the
//!    one it inherits admits values the base rejects, so the derived type is
//!    not a subset of it — and a composed range whose minimum sits above its
//!    maximum describes a type that accepts nothing.
//!
//! Deliberately *not* here: particle subsumption, the same "may only narrow"
//! idea applied to content models. That one is unimplemented as a set (see
//! AGENTS.md §7); these four are each complete on their own.

use crate::datatypes::{Builtin, FacetKind, FacetSet, Variety};
use crate::diagnostics::{DiagCode, Diagnostic, Diagnostics, Span};
use crate::model::{Schemas, SimpleType, TypeId};
use crate::validate::{self, Validator};
use crate::values;
use std::cmp::Ordering;

pub(crate) fn check_all(schemas: &Schemas) -> Diagnostics {
    let mut diags = Diagnostics::new();
    let v = schemas.validator();
    for (id, def) in schemas.iter_types() {
        let Some(s) = def.as_simple() else { continue };
        // The built-ins are installed by this crate, not read from a
        // document; checking them would only ever accuse ourselves.
        if s.builtin.is_some() {
            continue;
        }
        check_one(schemas, &v, id, s, &mut diags);
    }
    diags
}

fn check_one(
    schemas: &Schemas,
    v: &Validator<'_>,
    id: TypeId,
    s: &SimpleType,
    diags: &mut Diagnostics,
) {
    applicable(schemas, id, s, diags);
    values_are_in_the_base_space(schemas, id, s, diags);
    consistent(s, &s.span, diags);
    narrows(schemas, v, id, s, diags);
}

/// Rule 4: a restriction may only narrow.
///
/// Two halves, and both are needed. A bound declared here must not widen the
/// one it inherits — `minInclusive="5"` under a base that already said 10
/// admits values the base rejects, so the derived type is not a subset of it.
/// And the composed set has to describe a non-empty range at all: a type whose
/// effective minimum sits above its effective maximum accepts nothing, which
/// is never what the author meant and is not a legal type.
fn narrows(
    schemas: &Schemas,
    v: &Validator<'_>,
    id: TypeId,
    s: &SimpleType,
    diags: &mut Diagnostics,
) {
    let (variety, ..) = validate::effective_variety(schemas, id);
    let own = &s.facets;

    // Sizes and digit counts are plain integers, so they compare without a
    // value space and apply to lists as well as atomic types.
    let base = v.effective_facets(s.base);
    let mut err = |msg: String| {
        diags.push(Diagnostic::error(DiagCode::ConflictingFacets, msg).at(s.span.clone()));
    };
    if let Some(b) = base {
        let widened = |name: &str, own: Option<u64>, inherited: Option<u64>, grew: bool| match (
            own, inherited,
        ) {
            (Some(o), Some(i)) if grew == (o > i) && o != i => {
                Some(format!("`xs:{name}` {o} is wider than the inherited {i}"))
            }
            _ => None,
        };
        for m in [
            widened("minLength", own.min_length, b.min_length, false),
            widened("maxLength", own.max_length, b.max_length, true),
            widened(
                "totalDigits",
                own.total_digits.map(u64::from),
                b.total_digits.map(u64::from),
                true,
            ),
            widened(
                "fractionDigits",
                own.fraction_digits.map(u64::from),
                b.fraction_digits.map(u64::from),
                true,
            ),
        ]
        .into_iter()
        .flatten()
        {
            err(m);
        }
        // `length` fixes the size outright, so any inherited one it disagrees
        // with is a contradiction rather than a widening.
        if let (Some(o), Some(i)) = (own.length, b.length) {
            if o != i {
                err(format!("`xs:length` {o} disagrees with the inherited {i}"));
            }
        }
    }

    // The bounds live in a value space, so they need a built-in to parse
    // against — and a union has no single one.
    if variety != Variety::Atomic {
        return;
    }
    let Some(builtin) = validate::nearest_builtin(schemas, s.base) else {
        return;
    };
    if matches!(builtin, Builtin::QName | Builtin::Notation) {
        return;
    }
    let cmp = |a: &str, b: &str| {
        values::parse_in(builtin, a, schemas.xsd_version)
            .ok()?
            .partial_cmp_value(&values::parse_in(builtin, b, schemas.xsd_version).ok()?)
    };

    // Half one: each bound declared here against the one it inherits.
    if let Some(b) = base {
        let pairs: [(&str, &Option<String>, &Option<String>, bool); 4] = [
            ("minInclusive", &own.min_inclusive, &b.min_inclusive, false),
            ("minExclusive", &own.min_exclusive, &b.min_exclusive, false),
            ("maxInclusive", &own.max_inclusive, &b.max_inclusive, true),
            ("maxExclusive", &own.max_exclusive, &b.max_exclusive, true),
        ];
        for (name, o, i, upper) in pairs {
            let (Some(o), Some(i)) = (o, i) else { continue };
            let Some(ord) = cmp(o, i) else { continue };
            let widened = if upper {
                ord == Ordering::Greater
            } else {
                ord == Ordering::Less
            };
            if widened {
                err(format!(
                    "`xs:{name}` `{o}` is wider than the inherited `{i}`"
                ));
            }
        }
    }

    // Half two: the composed range has to hold something. Taking the effective
    // set rather than the declared one is what catches a bound that widens
    // past a *different* bound on the base — a minimum raised above an
    // inherited maximum is not a narrowing, it is an empty type.
    let Some(eff) = v.effective_facets(id) else {
        return;
    };
    let lower = eff.min_inclusive.as_ref().or(eff.min_exclusive.as_ref());
    let upper = eff.max_inclusive.as_ref().or(eff.max_exclusive.as_ref());
    if let (Some(lo), Some(hi)) = (lower, upper) {
        let strict = eff.min_exclusive.is_some() || eff.max_exclusive.is_some();
        match cmp(lo, hi) {
            Some(Ordering::Greater) => {
                err(format!("the minimum `{lo}` is above the maximum `{hi}`"))
            }
            // Equal bounds admit that one value — unless either end excludes
            // it, and then the type accepts nothing at all.
            Some(Ordering::Equal) if strict => err(format!(
                "`{lo}` is both the minimum and the maximum, and one end excludes it"
            )),
            _ => {}
        }
    }
}

/// Rule 1: every facet declared here must be one this datatype admits.
fn applicable(schemas: &Schemas, id: TypeId, s: &SimpleType, diags: &mut Diagnostics) {
    // The variety decides for a list or a union; only an atomic type defers
    // to its primitive.
    let (variety, ..) = validate::effective_variety(schemas, id);
    let declared = declared_kinds(&s.facets);

    for kind in declared {
        let ok = match variety {
            Variety::List => matches!(
                kind,
                FacetKind::Length
                    | FacetKind::MinLength
                    | FacetKind::MaxLength
                    | FacetKind::Pattern
                    | FacetKind::Enumeration
                    | FacetKind::WhiteSpace
                    | FacetKind::Assertion
            ),
            // A union has no lexical space of its own to measure and no
            // order to bound; all it can do is name values and shapes.
            Variety::Union => matches!(
                kind,
                FacetKind::Pattern | FacetKind::Enumeration | FacetKind::Assertion
            ),
            // No built-in ancestor means the base never resolved. Silence
            // here: the unresolved reference is the error worth reporting.
            Variety::Atomic => match validate::nearest_builtin(schemas, id) {
                Some(b) => b.allows_facet(kind),
                None => true,
            },
        };
        if !ok {
            let what = match variety {
                Variety::Atomic => validate::nearest_builtin(schemas, id)
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "this type".into()),
                Variety::List => "a list type".into(),
                Variety::Union => "a union type".into(),
            };
            diags.push(
                Diagnostic::error(
                    DiagCode::FacetNotApplicable,
                    format!("`xs:{kind}` is not applicable to {what}"),
                )
                .at(s.span.clone()),
            );
        }
    }
}

/// Rule 2: a bound, and each enumerated value, must be a value of the base.
///
/// Checked against the base's *built-in* ancestor rather than the base itself.
/// The narrower question — does the bound also satisfy the base's own facets —
/// is a derivation rule, and answering half of one is worse than answering
/// none: a schema that legally narrows a range in two steps would be rejected.
fn values_are_in_the_base_space(
    schemas: &Schemas,
    id: TypeId,
    s: &SimpleType,
    diags: &mut Diagnostics,
) {
    let (variety, ..) = validate::effective_variety(schemas, id);
    if variety != Variety::Atomic {
        return;
    }
    let Some(builtin) = validate::nearest_builtin(schemas, s.base) else {
        return;
    };
    // A QName or NOTATION value is a prefix plus a local name, and the prefix
    // only means something against the namespace bindings in scope where it
    // was written. Those live in the document, not in the type, so by the time
    // the model is assembled there is nothing left to check it against — and
    // `values::parse` says so by refusing every one of them.
    if matches!(builtin, Builtin::QName | Builtin::Notation) {
        return;
    }
    let f = &s.facets;
    let bounds = [
        (FacetKind::MinInclusive, &f.min_inclusive),
        (FacetKind::MaxInclusive, &f.max_inclusive),
        (FacetKind::MinExclusive, &f.min_exclusive),
        (FacetKind::MaxExclusive, &f.max_exclusive),
    ];
    for (kind, value) in bounds {
        let Some(v) = value else { continue };
        if let Err(e) = values::parse_in(builtin, v, schemas.xsd_version) {
            diags.push(
                Diagnostic::error(
                    DiagCode::InvalidFacetValue,
                    format!(
                        "`xs:{kind}` value `{v}` is not a valid {builtin}: {}",
                        e.reason
                    ),
                )
                .at(s.span.clone()),
            );
        }
    }
    for v in f.enumeration.iter().flatten() {
        if let Err(e) = values::parse_in(builtin, v, schemas.xsd_version) {
            diags.push(
                Diagnostic::error(
                    DiagCode::InvalidFacetValue,
                    format!(
                        "`xs:enumeration` value `{v}` is not a valid {builtin}: {}",
                        e.reason
                    ),
                )
                .at(s.span.clone()),
            );
        }
    }
}

/// Rule 3: pairs declared at this step that cannot both hold.
fn consistent(s: &SimpleType, span: &Span, diags: &mut Diagnostics) {
    let f = &s.facets;
    let mut err = |msg: String| {
        diags.push(Diagnostic::error(DiagCode::ConflictingFacets, msg).at(span.clone()));
    };

    // `length` fixes what the other two bound, so declaring them together at
    // one step is a contradiction even when the numbers happen to agree.
    if f.length.is_some() {
        if f.min_length.is_some() {
            err("`xs:length` and `xs:minLength` cannot both be declared here".into());
        }
        if f.max_length.is_some() {
            err("`xs:length` and `xs:maxLength` cannot both be declared here".into());
        }
    }
    if let (Some(min), Some(max)) = (f.min_length, f.max_length) {
        if min > max {
            err(format!("`xs:minLength` {min} exceeds `xs:maxLength` {max}"));
        }
    }
    // The inclusive/exclusive pairs are checked in the loader instead:
    // `FacetSet::restrict` clears one when the other is set, which is right
    // across restriction steps and leaves nothing to see within one.
    if let (Some(t), Some(fd)) = (f.total_digits, f.fraction_digits) {
        if fd > t {
            err(format!(
                "`xs:fractionDigits` {fd} exceeds `xs:totalDigits` {t}"
            ));
        }
    }
    // totalDigits is a positiveInteger; zero significant digits is no number.
    if f.total_digits == Some(0) {
        err("`xs:totalDigits` must be at least 1".into());
    }
}

/// Which facets this step actually declared.
fn declared_kinds(f: &FacetSet) -> Vec<FacetKind> {
    let mut out = Vec::new();
    let mut add = |present: bool, k: FacetKind| {
        if present {
            out.push(k);
        }
    };
    add(f.length.is_some(), FacetKind::Length);
    add(f.min_length.is_some(), FacetKind::MinLength);
    add(f.max_length.is_some(), FacetKind::MaxLength);
    add(!f.patterns.is_empty(), FacetKind::Pattern);
    add(f.enumeration.is_some(), FacetKind::Enumeration);
    add(f.white_space.is_some(), FacetKind::WhiteSpace);
    add(f.max_inclusive.is_some(), FacetKind::MaxInclusive);
    add(f.max_exclusive.is_some(), FacetKind::MaxExclusive);
    add(f.min_inclusive.is_some(), FacetKind::MinInclusive);
    add(f.min_exclusive.is_some(), FacetKind::MinExclusive);
    add(f.total_digits.is_some(), FacetKind::TotalDigits);
    add(f.fraction_digits.is_some(), FacetKind::FractionDigits);
    add(f.explicit_timezone.is_some(), FacetKind::ExplicitTimezone);
    add(!f.assertions.is_empty(), FacetKind::Assertion);
    out
}

//! May this type derive from that one?
//!
//! A type's `final` names the derivations it refuses to be the base of. It is
//! how a schema author says "this is the last word" — a `final="extension"`
//! measurement type cannot quietly grow an attribute in a schema that imports
//! it — and ignoring it turns a deliberate seal into decoration.
//!
//! Only the sealing rule lives here. The rest of *Derivation Valid* — whether
//! a restriction's content model actually accepts a subset of its base's — is
//! particle subsumption, and it stays unimplemented as a set rather than
//! half-done (AGENTS.md §7). The distinction is that this rule is complete on
//! its own: it needs the base's `final` and the method used, and nothing else.

use crate::datatypes::Variety;
use crate::diagnostics::{DiagCode, Diagnostic, Diagnostics};
use crate::model::{DerivationMethod, DerivationSet, Schemas, TypeId};

pub(crate) fn check_all(schemas: &Schemas) -> Diagnostics {
    let mut diags = Diagnostics::new();

    for (id, def) in schemas.iter_types() {
        match def {
            crate::model::TypeDefinition::Complex(c) => {
                let method = match c.derivation {
                    DerivationMethod::Extension => "extension",
                    DerivationMethod::Restriction => "restriction",
                };
                seal(schemas, id, c.base, method, &mut diags);
            }
            crate::model::TypeDefinition::Simple(s) => match s.variety {
                // Every atomic simple type reaches its base by restriction;
                // there is no other way down the atomic branch.
                Variety::Atomic => seal(schemas, id, s.base, "restriction", &mut diags),
                Variety::List => {
                    if let Some(item) = s.item_type {
                        seal(schemas, id, item, "list", &mut diags);
                    }
                }
                Variety::Union => {
                    for &m in &s.member_types {
                        seal(schemas, id, m, "union", &mut diags);
                    }
                }
            },
        }
    }
    diags
}

/// Reports the derivation if `base` seals itself against `method`.
fn seal(schemas: &Schemas, derived: TypeId, base: TypeId, method: &str, diags: &mut Diagnostics) {
    // A type is its own base at the top of each branch, and the built-ins seal
    // nothing, so neither case can produce a finding.
    if base == derived || base.is_placeholder() {
        return;
    }
    if !blocks(final_of(schemas, base), method) {
        return;
    }
    let name = |t: TypeId| {
        schemas[t]
            .name()
            .map(|n| schemas.display_name(n))
            .unwrap_or_else(|| "an anonymous type".into())
    };
    diags.push(
        Diagnostic::error(
            DiagCode::DerivationBlocked,
            format!(
                "{} cannot derive from {} by {method}",
                name(derived),
                name(base)
            ),
        )
        .at(schemas[derived].span().clone())
        .with_help(format!("its `final` forbids {method}")),
    );
}

fn final_of(schemas: &Schemas, id: TypeId) -> DerivationSet {
    match &schemas[id] {
        crate::model::TypeDefinition::Simple(s) => s.final_,
        crate::model::TypeDefinition::Complex(c) => c.final_,
    }
}

fn blocks(set: DerivationSet, method: &str) -> bool {
    match method {
        "extension" => set.extension,
        "restriction" => set.restriction,
        "list" => set.list,
        "union" => set.union,
        _ => false,
    }
}

//! Are an element's or attribute's declared properties consistent with its
//! type?
//!
//! The specification calls these *Element Declaration Properties Correct* and
//! *Attribute Declaration Properties Correct*. They are about the declaration
//! rather than about any document, and the one that matters most is the
//! obvious one: a `default` or `fixed` value the type itself rejects. A schema
//! that supplies `<length unit="feets"/>` into every instance where the type
//! enumerates `m` and `ft` is broken, and it is broken whether or not anyone
//! ever writes a document.
//!
//! Runs against the assembled `Schemas` because every rule needs the type
//! resolved, and the value check needs the composed facet set that
//! [`crate::validate::Validator`] builds.

use crate::datatypes::Builtin;
use crate::diagnostics::{DiagCode, Diagnostic, Diagnostics, Span};
use crate::load::Version;
use crate::model::{ContentType, Schemas, TypeId, ValueConstraint};
use crate::validate::{ValidationError, Validator};

pub(crate) fn check_all(schemas: &Schemas) -> Diagnostics {
    let mut diags = Diagnostics::new();
    let v = schemas.validator();

    for (_, a) in schemas.iter_attributes() {
        let Some(vc) = &a.value_constraint else {
            continue;
        };
        // An ID is a document-wide name, and a schema that supplies the same
        // one into every instance of the attribute supplies a duplicate the
        // moment the element appears twice. So 1.0 rules it out by type.
        //
        // 1.1 dropped the rule — the duplicate it guards against is caught by
        // ID uniqueness anyway, and only when it actually happens — so this is
        // one of the few places where the version changes a verdict.
        if schemas.xsd_version() == Version::Xsd10 && is_id(schemas, a.type_id) {
            diags.push(
                Diagnostic::error(
                    DiagCode::InvalidValueConstraint,
                    format!(
                        "attribute `{}` is an ID, so it cannot have a `{}` value",
                        schemas.display_name(a.name),
                        keyword(vc)
                    ),
                )
                .at(a.span.clone())
                .with_help("an ID must be unique in the document, so no schema can supply one"),
            );
            continue;
        }
        check_value(&v, schemas, a.type_id, vc, &a.span, "attribute", &mut diags);
    }

    for (_, e) in schemas.iter_elements() {
        let Some(vc) = &e.value_constraint else {
            continue;
        };
        // A value constraint supplies *character* content. An element whose
        // content model admits only child elements has nowhere to put it.
        let target = match &schemas[e.type_id] {
            def if def.is_simple() => Some(e.type_id),
            def => match def.as_complex().map(|c| c.content) {
                Some(ContentType::Simple(t)) => Some(t),
                // Mixed content can hold the characters. The specification
                // additionally requires the particle be emptiable; that is a
                // content-model question, and answering it here would mean a
                // second, divergent emptiness analysis.
                Some(ContentType::Mixed(_)) => None,
                _ => {
                    diags.push(
                        Diagnostic::error(
                            DiagCode::InvalidValueConstraint,
                            format!(
                                "element `{}` has element-only content, so it cannot have a `{}` value",
                                schemas.display_name(e.name),
                                keyword(vc)
                            ),
                        )
                        .at(e.span.clone()),
                    );
                    continue;
                }
            },
        };
        if let Some(t) = target {
            check_value(&v, schemas, t, vc, &e.span, "element", &mut diags);
        }
    }

    diags
}

/// The declared value has to be one the type accepts — patterns, enumeration,
/// bounds and all, which is why this goes through the validator rather than
/// through `values::parse`.
fn check_value(
    v: &Validator<'_>,
    schemas: &Schemas,
    ty: TypeId,
    vc: &ValueConstraint,
    span: &Span,
    what: &str,
    diags: &mut Diagnostics,
) {
    // A QName's value depends on the prefix bindings in scope where it was
    // written — here, in the *schema* document. `FacetSet::namespaces` keeps
    // those for an enumeration's literals, but a `default` or `fixed` value
    // has no equivalent, so checking would reject every prefixed one outright.
    // Skipping is the honest answer until `ValueConstraint` carries them too.
    if matches!(
        schemas[ty].as_simple().and_then(|t| t.primitive),
        Some(crate::datatypes::Builtin::QName | crate::datatypes::Builtin::Notation)
    ) {
        return;
    }

    let lexical = vc.value();
    match v.validate(ty, lexical) {
        Ok(_) => {}
        // Not a verdict: a complex type has no value space to check against.
        Err(ValidationError::NotSimple) => {}
        Err(e) => {
            let name = schemas[ty]
                .name()
                .map(|n| schemas.display_name(n))
                .unwrap_or_else(|| "its type".into());
            diags.push(
                Diagnostic::error(
                    DiagCode::InvalidValueConstraint,
                    format!(
                        "{what} `{}` value `{lexical}` is not valid for {name}: {e}",
                        keyword(vc)
                    ),
                )
                .at(span.clone()),
            );
        }
    }
}

fn keyword(vc: &ValueConstraint) -> &'static str {
    if vc.is_fixed() { "fixed" } else { "default" }
}

/// Whether the type is `xs:ID` or derived from it.
fn is_id(schemas: &Schemas, ty: TypeId) -> bool {
    schemas.base_chain(ty).into_iter().any(|t| {
        schemas[t]
            .as_simple()
            .and_then(|s| s.builtin)
            .is_some_and(|b| b == Builtin::Id)
    })
}

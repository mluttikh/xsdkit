//! Structured diagnostics with source spans.
//!
//! Two rules, both load-bearing:
//!
//! 1. **Never bail on the first error.** A schema author fixing a 40-file
//!    import graph needs the whole list, not the first entry.
//! 2. **`Display` output is a stability surface.** Codes and messages are
//!    matched by downstream tooling; don't reword casually.

use std::fmt;

/// Where a diagnostic points, in the source schema document.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Span {
    /// Absolute URI of the schema document.
    pub uri: String,
    /// 1-based line number, or 0 when unknown.
    pub line: u32,
    /// What this span contributes, e.g. "declared here".
    pub label: Option<String>,
}

impl Span {
    pub fn new(uri: impl Into<String>, line: u32) -> Self {
        Self {
            uri: uri.into(),
            line,
            label: None,
        }
    }

    pub fn labelled(uri: impl Into<String>, line: u32, label: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            line,
            label: Some(label.into()),
        }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}", self.uri, self.line)?;
        } else {
            write!(f, "{}", self.uri)?;
        }
        if let Some(l) = &self.label {
            write!(f, " ({l})")?;
        }
        Ok(())
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        })
    }
}

/// A stable, greppable identifier for a class of problem.
///
/// Codes are permanent once released. Adding a variant is non-breaking;
/// renumbering or reusing one is not.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum DiagCode {
    // 1000-1099 — document level
    /// The document is not well-formed XML.
    MalformedXml,
    /// The document root is not `xs:schema`.
    NotASchemaDocument,
    /// An element in the XSD namespace that this version does not define.
    UnknownSchemaElement,
    /// A required attribute is missing.
    MissingAttribute,
    /// An `xs:annotation` appears where the schema for schemas does not
    /// allow one: after a sibling, or a second time.
    MisplacedAnnotation,
    /// A child element the schema for schemas requires is absent.
    MissingElement,
    /// An attribute's value does not match its expected form.
    InvalidAttributeValue,
    /// The document declares an encoding this build cannot decode.
    UnsupportedEncoding,
    /// The document's bytes are not valid in the encoding it claims.
    MalformedEncoding,

    // 1100-1199 — composition
    /// `schemaLocation` could not be resolved to a document.
    UnresolvedSchemaLocation,
    /// `xs:include` names a document whose target namespace conflicts.
    IncludeNamespaceMismatch,
    /// `xs:import` names a namespace the imported document does not define.
    ImportNamespaceMismatch,
    /// A construct is recognised but not yet implemented.
    Unsupported,

    // 1200-1299 — resolution
    /// A `ref`, `base`, `type` or `itemType` names something not declared.
    UnresolvedReference,
    /// Two global components share a name within one symbol space.
    DuplicateGlobal,
    /// A derivation, substitution or group chain refers back to itself.
    CircularDefinition,

    // 1300-1399 — component constraints
    /// A simple type declares more than one of restriction/list/union.
    ConflictingSimpleTypeVariety,
    /// `minOccurs` exceeds `maxOccurs`.
    InvalidOccurrence,
    /// An element declaration has both a `type` attribute and inline content.
    ConflictingTypeDefinition,
    /// A content model breaches Unique Particle Attribution: one element
    /// could be matched by two different particles.
    AmbiguousContentModel,
    /// A facet was applied to a datatype that does not admit it, e.g.
    /// `length` on a `xs:duration`.
    FacetNotApplicable,
    /// A facet's own value is not legal — a bound that is not a value of the
    /// type it bounds, or a count the facet's own type forbids.
    InvalidFacetValue,
    /// Two facets on one restriction step cannot both hold, e.g. `length`
    /// beside `minLength`, or `minInclusive` above `maxInclusive`.
    ConflictingFacets,
    /// A `default` or `fixed` value its own type rejects, or one on a
    /// declaration that cannot carry one at all.
    InvalidValueConstraint,
    /// A type derives from a base whose `final` forbids that derivation.
    DerivationBlocked,
    /// A restriction's content model accepts something its base does not.
    InvalidRestriction,

    // 2000-2099 — instance documents
    /// No global element declaration matches the document's root.
    ElementNotDeclared,
    /// An element appeared where its parent's content model does not allow it.
    UnexpectedElement,
    /// An element ended before its content model was satisfied.
    IncompleteContent,
    /// A value is not valid against its type.
    InvalidValue,
    /// An attribute is not permitted on this element.
    AttributeNotAllowed,
    /// A required attribute is absent.
    MissingRequiredAttribute,
    /// Character data appeared where the content model permits none.
    UnexpectedText,
    /// `xsi:type` names a type that is unknown, or not derived from the
    /// declared one.
    InvalidXsiType,
    /// An element marked `xsi:nil="true"` is not empty.
    NilElementNotEmpty,
    /// The type in force for an element is abstract, so no element may be
    /// validated against it — an `xsi:type` naming a concrete derivation is
    /// what an abstract type is for.
    AbstractType,
}

impl DiagCode {
    /// The stable printed form, e.g. `XSD1201`.
    pub fn as_str(self) -> &'static str {
        match self {
            DiagCode::MalformedXml => "XSD1001",
            DiagCode::NotASchemaDocument => "XSD1002",
            DiagCode::UnknownSchemaElement => "XSD1003",
            DiagCode::MissingAttribute => "XSD1004",
            DiagCode::MisplacedAnnotation => "XSD1008",
            DiagCode::MissingElement => "XSD1009",
            DiagCode::InvalidAttributeValue => "XSD1005",
            DiagCode::UnsupportedEncoding => "XSD1006",
            DiagCode::MalformedEncoding => "XSD1007",

            DiagCode::UnresolvedSchemaLocation => "XSD1101",
            DiagCode::IncludeNamespaceMismatch => "XSD1102",
            DiagCode::ImportNamespaceMismatch => "XSD1103",
            DiagCode::Unsupported => "XSD1104",

            DiagCode::UnresolvedReference => "XSD1201",
            DiagCode::DuplicateGlobal => "XSD1202",
            DiagCode::CircularDefinition => "XSD1203",

            DiagCode::ConflictingSimpleTypeVariety => "XSD1301",
            DiagCode::InvalidOccurrence => "XSD1302",
            DiagCode::ConflictingTypeDefinition => "XSD1303",
            DiagCode::AmbiguousContentModel => "XSD1304",
            DiagCode::FacetNotApplicable => "XSD1305",
            DiagCode::InvalidFacetValue => "XSD1306",
            DiagCode::ConflictingFacets => "XSD1307",
            DiagCode::InvalidValueConstraint => "XSD1308",
            DiagCode::DerivationBlocked => "XSD1309",
            DiagCode::InvalidRestriction => "XSD1310",

            DiagCode::ElementNotDeclared => "XSD2001",
            DiagCode::UnexpectedElement => "XSD2002",
            DiagCode::IncompleteContent => "XSD2003",
            DiagCode::InvalidValue => "XSD2004",
            DiagCode::AttributeNotAllowed => "XSD2005",
            DiagCode::MissingRequiredAttribute => "XSD2006",
            DiagCode::UnexpectedText => "XSD2007",
            DiagCode::InvalidXsiType => "XSD2008",
            DiagCode::NilElementNotEmpty => "XSD2009",
            DiagCode::AbstractType => "XSD2010",
        }
    }
}

impl fmt::Display for DiagCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    pub code: DiagCode,
    pub severity: Severity,
    pub message: String,
    pub spans: Vec<Span>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn error(code: DiagCode, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            spans: Vec::new(),
            help: None,
        }
    }

    pub fn warning(code: DiagCode, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message: message.into(),
            spans: Vec::new(),
            help: None,
        }
    }

    pub fn at(mut self, span: Span) -> Self {
        self.spans.push(span);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]: {}", self.severity, self.code, self.message)?;
        for span in &self.spans {
            write!(f, "\n  --> {span}")?;
        }
        if let Some(h) = &self.help {
            write!(f, "\n  help: {h}")?;
        }
        Ok(())
    }
}

/// A collection of diagnostics, returned whole rather than one at a time.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.0.push(d);
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.0.extend(other.0);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter()
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter().filter(|d| d.is_error())
    }

    pub fn has_errors(&self) -> bool {
        self.0.iter().any(Diagnostic::is_error)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Drops warnings and notes, keeping only errors.
    pub fn into_errors(self) -> Vec<Diagnostic> {
        self.0.into_iter().filter(Diagnostic::is_error).collect()
    }
}

impl IntoIterator for Diagnostics {
    type Item = Diagnostic;
    type IntoIter = std::vec::IntoIter<Diagnostic>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl FromIterator<Diagnostic> for Diagnostics {
    fn from_iter<T: IntoIterator<Item = Diagnostic>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, d) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{d}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Diagnostics {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique() {
        let all = [
            DiagCode::MalformedXml,
            DiagCode::NotASchemaDocument,
            DiagCode::UnknownSchemaElement,
            DiagCode::MissingAttribute,
            DiagCode::MisplacedAnnotation,
            DiagCode::MissingElement,
            DiagCode::InvalidAttributeValue,
            DiagCode::UnsupportedEncoding,
            DiagCode::MalformedEncoding,
            DiagCode::UnresolvedSchemaLocation,
            DiagCode::IncludeNamespaceMismatch,
            DiagCode::ImportNamespaceMismatch,
            DiagCode::Unsupported,
            DiagCode::UnresolvedReference,
            DiagCode::DuplicateGlobal,
            DiagCode::CircularDefinition,
            DiagCode::ConflictingSimpleTypeVariety,
            DiagCode::InvalidOccurrence,
            DiagCode::ConflictingTypeDefinition,
            DiagCode::AmbiguousContentModel,
            DiagCode::ElementNotDeclared,
            DiagCode::UnexpectedElement,
            DiagCode::IncompleteContent,
            DiagCode::InvalidValue,
            DiagCode::AttributeNotAllowed,
            DiagCode::MissingRequiredAttribute,
            DiagCode::UnexpectedText,
            DiagCode::InvalidXsiType,
            DiagCode::NilElementNotEmpty,
            DiagCode::AbstractType,
        ];
        let mut seen = std::collections::HashSet::new();
        for c in all {
            assert!(seen.insert(c.as_str()), "duplicate code {}", c.as_str());
            assert!(c.as_str().starts_with("XSD"));
        }
    }

    #[test]
    fn display_includes_code_spans_and_help() {
        let d = Diagnostic::error(DiagCode::UnresolvedReference, "unknown type `{urn:x}Foo`")
            .at(Span::labelled("file:///a.xsd", 12, "referenced here"))
            .with_help("did you forget an xs:import?");
        let s = d.to_string();
        assert!(s.contains("error[XSD1201]"), "{s}");
        assert!(s.contains("file:///a.xsd:12 (referenced here)"), "{s}");
        assert!(s.contains("help: did you forget"), "{s}");
    }

    #[test]
    fn collection_separates_errors_from_warnings() {
        let mut ds = Diagnostics::new();
        ds.push(Diagnostic::warning(DiagCode::Unsupported, "w"));
        assert!(!ds.has_errors());
        ds.push(Diagnostic::error(DiagCode::MalformedXml, "e"));
        assert!(ds.has_errors());
        assert_eq!(ds.len(), 2);
        assert_eq!(ds.errors().count(), 1);
    }
}

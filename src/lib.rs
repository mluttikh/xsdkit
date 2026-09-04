//! Parse W3C XML Schema (XSD) into a queryable schema component model.
//!
//! XSD is three languages: schema *documents*, schema *components*, and
//! validation *semantics* defined over those components. `xsdkit` builds the
//! middle layer — the graph the specification defines all its semantics
//! against — and hands it to you to query.
//!
//! ```no_run
//! use xsdkit::SchemaSetBuilder;
//!
//! let schemas = SchemaSetBuilder::new()
//!     .search_path("schemas/")
//!     .file("report.xsd")
//!     .build()?;
//!
//! let report = schemas.element(Some("urn:example"), "report").unwrap();
//! println!("{}", schemas.display_name(schemas[report].name));
//! # Ok::<_, xsdkit::Diagnostics>(())
//! ```
//!
//! # Lifecycle
//!
//! [`SchemaSetBuilder`] reads documents; [`Schemas`] is the compiled result.
//! They are separate types on purpose: a `Schemas` never exists in an
//! unresolved state, so ".NET's did you call `Compile()`?" is not
//! representable. Compilation is not cheap — build once, query many times.
//!
//! # Errors
//!
//! Building returns *every* diagnostic, not the first. A schema author
//! fixing a 40-file import graph needs the whole list.
//!
//! # Status
//!
//! Implemented: the component model, document loading with `include` /
//! `import` (chameleon includes included), reference resolution, attribute
//! group flattening and substitution-group closure.
//!
//! Not yet: content-model automata and UPA, instance validation, XSD 1.1
//! assertions and conditional type assignment, and `redefine`/`override`
//! (currently read as plain includes, with a warning).

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub(crate) mod compile;
pub mod content;
pub mod datatypes;
pub mod diagnostics;
pub mod encoding;
pub mod load;
pub mod model;
pub mod names;
pub mod values;

#[cfg(feature = "python")]
mod python;

pub use content::{
    AllGroup, AllMember, ContentAutomaton, ContentMatcher, ContentModel, ContentStats, Label,
    MAX_POSITIONS, Position,
};
pub use diagnostics::{DiagCode, Diagnostic, Diagnostics, Severity, Span};
pub use load::{Conformance, DEFAULT_NODES_LIMIT, FileResolver, Resolver};
pub use model::{
    Annotation, AppInfo, AttrGroupId, AttributeDecl, AttributeId, AttributeUse, AttributeUseKind,
    ComplexType, ComponentCounts, Compositor, ContentType, DerivationMethod, DerivationSet,
    ElementDecl, ElementId, IdcId, IdcKind, IdentityConstraint, MaxOccurs, ModelGroup, Particle,
    ParticleId, Schemas, Scope, SimpleType, SourceDocument, SymbolSpace, Term, TypeDefinition,
    TypeId, ValueConstraint, Wildcard,
};
pub use names::{Interner, QName};
pub use values::{FacetViolation, Value, ValueError, check_facets};

use load::Loader;

/// Reads schema documents and compiles them into a [`Schemas`].
///
/// ```no_run
/// # use xsdkit::{SchemaSetBuilder, Conformance};
/// let schemas = SchemaSetBuilder::new()
///     .conformance(Conformance::Lax)
///     .search_path("vendor/schemas")
///     .file("witsml.xsd")
///     .build();
/// ```
#[derive(Debug)]
pub struct SchemaSetBuilder {
    resolver: Option<Box<dyn Resolver>>,
    search_paths: Vec<std::path::PathBuf>,
    mode: Conformance,
    nodes_limit: u32,
    sources: Vec<Source>,
}

#[derive(Debug)]
enum Source {
    Uri(String),
    Text { text: String, uri: String },
    Bytes { bytes: Vec<u8>, uri: String },
}

impl std::fmt::Debug for dyn Resolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<resolver>")
    }
}

impl Default for SchemaSetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaSetBuilder {
    pub fn new() -> Self {
        Self {
            resolver: None,
            search_paths: Vec::new(),
            mode: Conformance::Strict,
            nodes_limit: DEFAULT_NODES_LIMIT,
            sources: Vec::new(),
        }
    }

    /// Replaces the resolver used for every `schemaLocation` hint.
    ///
    /// The default reads the filesystem and refuses network URLs.
    pub fn resolver(mut self, r: impl Resolver + 'static) -> Self {
        self.resolver = Some(Box::new(r));
        self
    }

    /// Adds a directory to the default resolver's search path.
    ///
    /// Ignored once [`Self::resolver`] has replaced the default.
    pub fn search_path(mut self, p: impl Into<std::path::PathBuf>) -> Self {
        self.search_paths.push(p.into());
        self
    }

    /// Caps the XML nodes a single schema document may contain.
    ///
    /// Defaults to [`DEFAULT_NODES_LIMIT`]. Lower it when loading schemas
    /// from an untrusted source.
    pub fn nodes_limit(mut self, limit: u32) -> Self {
        self.nodes_limit = limit;
        self
    }

    /// Chooses how strictly specification violations are treated.
    pub fn conformance(mut self, mode: Conformance) -> Self {
        self.mode = mode;
        self
    }

    /// Queues a schema document, resolved through the resolver.
    pub fn file(mut self, path: impl Into<String>) -> Self {
        self.sources.push(Source::Uri(path.into()));
        self
    }

    /// Queues a schema document already in memory, as text.
    ///
    /// `uri` is used for diagnostics and to resolve relative
    /// `schemaLocation`s inside it.
    pub fn text(mut self, xsd: impl Into<String>, uri: impl Into<String>) -> Self {
        self.sources.push(Source::Text {
            text: xsd.into(),
            uri: uri.into(),
        });
        self
    }

    /// Queues a schema document already in memory, as raw bytes.
    ///
    /// Prefer this over [`Self::text`] whenever the encoding is not known to
    /// be UTF-8: the bytes go through the same detection as a resolved file —
    /// byte-order mark, then the XML declaration, then UTF-8.
    pub fn bytes(mut self, xsd: impl Into<Vec<u8>>, uri: impl Into<String>) -> Self {
        self.sources.push(Source::Bytes {
            bytes: xsd.into(),
            uri: uri.into(),
        });
        self
    }

    /// Reads every queued document and compiles the result.
    ///
    /// Returns `Err` with **all** diagnostics when any is an error;
    /// warnings alone still yield a `Schemas`, which
    /// [`Self::build_with_warnings`] hands back alongside them.
    pub fn build(self) -> Result<Schemas, Diagnostics> {
        let (schemas, diags) = self.build_with_warnings();
        if diags.has_errors() {
            Err(diags)
        } else {
            Ok(schemas)
        }
    }

    /// Like [`Self::build`], but returns the components even when there were
    /// errors — a partial model is what an editor or language server wants.
    pub fn build_with_warnings(self) -> (Schemas, Diagnostics) {
        let default_resolver;
        let resolver: &dyn Resolver = match self.resolver.as_ref() {
            Some(r) => r.as_ref(),
            None => {
                default_resolver = FileResolver {
                    search_paths: self.search_paths.clone(),
                };
                &default_resolver
            }
        };
        let mut loader = Loader::new(resolver, self.mode);
        loader.set_nodes_limit(self.nodes_limit);
        for s in &self.sources {
            match s {
                Source::Uri(u) => loader.load_uri(u, None),
                Source::Text { text, uri } => loader.load_text(text, uri, None),
                Source::Bytes { bytes, uri } => loader.load_bytes(bytes, uri, None),
            }
        }
        compile::compile(loader, self.mode)
    }
}

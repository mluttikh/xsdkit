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
//!     .compile().into_result()?;
//!
//! let report = schemas.element(Some("urn:example"), "report").unwrap();
//! for child in report.children() {
//!     println!(
//!         "{}{}{}: {}",
//!         child.local_name(),
//!         if child.repeats() { "+" } else { "" },
//!         if child.optional() { "?" } else { "" },
//!         child.type_of().display_name(),
//!     );
//! }
//! # Ok::<_, xsdkit::Diagnostics>(())
//! ```
//!
//! # Lifecycle
//!
//! [`SchemaSetBuilder`] reads documents; [`Schemas`] is the compiled result.
//! They are separate types on purpose: a `Schemas` never exists in an
//! unresolved state, so ".NET's did you call `Compile()`?" is not
//! representable. Compilation is not cheap — compile once, query many times.
//!
//! [`SchemaSetBuilder::compile`] hands back a [`Compilation`]: the components
//! *and* every diagnostic, always. [`Compilation::into_result`] converts to
//! the `Result` shape, which is also where warnings get discarded — an
//! explicit step rather than the default one.
//!
//! # Two ways to ask
//!
//! Components live in arenas addressed by `Copy` id, and [`refs`] is the
//! navigable view over them: [`ElementRef`], [`TypeRef`], [`ChildRef`] and
//! friends, each a borrow plus an id, so following a schema costs no
//! allocation and no reference counting. Name lookups
//! ([`Schemas::element`], [`Schemas::type_`]) hand back references; the
//! `_id` forms beside them ([`Schemas::element_id`] and so on) hand back
//! ids, for when the id is what you mean to store or compare.
//! [`Schemas::get`] turns any id back into a reference.
//!
//! # Validating
//!
//! Two validators, for two different questions.
//! [`ValueValidator`] checks a lexical form against a simple type;
//! [`DocumentValidator`] checks a whole document against the schema and
//! streams a typed PSVI. [`Schemas::value_validator`] and
//! [`Schemas::document_validator`] build them.
//!
//! # Errors
//!
//! Compiling returns *every* diagnostic, not the first. A schema author
//! fixing a 40-file import graph needs the whole list.
//!
//! # Status
//!
//! Implemented: the component model, document loading with `include`,
//! `import`, `redefine` and `override` (chameleon includes included),
//! reference resolution, attribute group flattening, substitution-group
//! closure, content-model automata with UPA, and streaming instance
//! validation with a typed PSVI.
//!
//! XSD 1.1 is opt-in via [`Version::Xsd11`]: `openContent`,
//! `defaultAttributes` and the relaxed UPA rule.
//!
//! Not yet: XSD 1.1 assertions and conditional type assignment, both of
//! which need an XPath 2.0 evaluator.
//!
//! # Cargo features
//!
//! `serde` derives `Serialize`/`Deserialize` for [`Schemas`] and everything
//! it holds, so a schema set is compiled once and loaded thereafter.
//! Any serde format works, self-describing ones included.
//!
#![cfg_attr(
    feature = "serde",
    doc = r#"```no_run
# let schemas = xsdkit::SchemaSetBuilder::new().file("report.xsd").compile().into_result().unwrap();
let cached = postcard::to_allocvec(&schemas)?;
let schemas: xsdkit::Schemas = postcard::from_bytes(&cached)?;
# Ok::<_, Box<dyn std::error::Error>>(())
```
"#
)]
//!
//! The format is not stable across versions of this crate — a cache written
//! by one version has to be rebuilt for the next. Names are interned and
//! every component refers to them by index, so a cache is only meaningful
//! alongside the code that wrote it. Key it on the crate version.
//!
//! `python` builds the PyO3 extension module and is not for library use.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod atomic;
pub(crate) mod compile;
pub mod content;
pub mod datatypes;
pub(crate) mod declarations;
pub(crate) mod derivation;
pub mod diagnostics;
pub mod encoding;
pub(crate) mod facets;
mod identity;
pub mod instance;
// Configuration, not a phase: `Version`, `Conformance`, `Resolver` and
// `FileResolver` are re-exported at the root, which is where a caller looks
// for them. Naming the module after the step that consumes them made the
// import path an implementation detail.
mod load;
pub mod model;
pub mod names;
pub mod refs;
pub mod regex;
pub(crate) mod restriction;
pub mod validate;
pub mod values;

#[cfg(feature = "python")]
mod python;

pub use content::{
    AllGroup, AllMember, Child, Content, ContentMatcher, ContentModel, ContentStats,
};
pub use diagnostics::{DiagCode, Diagnostic, Diagnostics, Severity, Span};
pub use instance::{DocumentValidator, ValidationReport};
pub use load::{Conformance, DEFAULT_NODES_LIMIT, FileResolver, Resolver, Version};
pub use model::{
    Annotation, AppInfo, AttrGroupId, AttributeDecl, AttributeId, AttributeUse, AttributeUseKind,
    ComplexType, ComponentCounts, Compositor, ContentType, DerivationMethod, DerivationSet,
    ElementDecl, ElementId, IdcId, IdcKind, IdentityConstraint, MaxOccurs, ModelGroup, OpenContent,
    OpenContentMode, Particle, ParticleId, Schemas, Scope, SimpleType, SourceDocument, SymbolSpace,
    Term, TypeDefinition, TypeId, ValueConstraint, Wildcard,
};
pub use names::{Interner, QName};
pub use refs::{AttributeRef, AttributeUseRef, ChildRef, Component, ElementRef, TypeRef};
pub use validate::{ValidationError, ValueValidator};
pub use values::{
    FacetViolation, Namespaces, NoNamespaces, QNameValue, Value, ValueError, check_facets,
};

use load::Loader;

/// Reads schema documents and compiles them into a [`Schemas`].
///
/// ```no_run
/// # use xsdkit::{SchemaSetBuilder, Conformance};
/// let schemas = SchemaSetBuilder::new()
///     .conformance(Conformance::Lax)
///     .search_path("vendor/schemas")
///     .file("witsml.xsd")
///     .compile().into_result();
/// ```
#[derive(Debug)]
pub struct SchemaSetBuilder {
    resolver: Option<Box<dyn Resolver>>,
    search_paths: Vec<std::path::PathBuf>,
    mode: Conformance,
    version: Version,
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
            version: Version::default(),
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

    /// Chooses which XSD version to process as.
    ///
    /// Defaults to [`Version::Xsd10`], which is what most shipping schemas
    /// are and the stricter reading of the two — a 1.0-clean schema is also
    /// 1.1-clean, but not the reverse.
    pub fn version(mut self, v: Version) -> Self {
        self.version = v;
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
    /// Always returns both halves. A schema with errors still compiles to
    /// components — a partial model is what an editor or a language server
    /// wants — and a schema without them can still have warnings worth
    /// reading. Call [`Compilation::into_result`] for the `Result` shape.
    ///
    /// ```no_run
    /// # use xsdkit::SchemaSetBuilder;
    /// let compiled = SchemaSetBuilder::new().file("report.xsd").compile();
    /// for d in compiled.diagnostics.iter() {
    ///     eprintln!("{d}");
    /// }
    /// let schemas = compiled.into_result()?;
    /// # Ok::<_, xsdkit::Diagnostics>(())
    /// ```
    pub fn compile(self) -> Compilation {
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
        loader.set_version(self.version);
        for s in &self.sources {
            match s {
                Source::Uri(u) => loader.load_uri(u, None),
                Source::Text { text, uri } => loader.load_text(text, uri, None),
                Source::Bytes { bytes, uri } => loader.load_bytes(bytes, uri, None),
            }
        }
        let (schemas, diagnostics) = compile::compile(loader, self.mode);
        Compilation {
            schemas,
            diagnostics,
        }
    }
}

/// What compiling a schema set produced: the components, and everything the
/// compiler had to say about them.
///
/// Both halves, always. This crate's position is "every diagnostic, not the
/// first", and a terminal method that returns a `Result` has to drop the
/// warnings to honour the success case — so the terminal method returns this
/// instead, and [`Self::into_result`] is where the choice to discard them is
/// made explicitly.
#[derive(Debug)]
pub struct Compilation {
    /// The components. Present even when compilation reported errors.
    pub schemas: Schemas,
    /// Every diagnostic, errors and warnings alike, in source order.
    pub diagnostics: Diagnostics,
}

impl Compilation {
    /// The components if nothing was an error, every diagnostic if something
    /// was.
    ///
    /// Warnings are discarded on success — which is the point of naming this
    /// separately rather than making it the terminal method.
    pub fn into_result(self) -> Result<Schemas, Diagnostics> {
        if self.diagnostics.has_errors() {
            Err(self.diagnostics)
        } else {
            Ok(self.schemas)
        }
    }

    /// Whether any diagnostic was an error.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.has_errors()
    }
}

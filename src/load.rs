//! Loading schema documents into components.
//!
//! `schemaLocation` is a **hint**, not a location, so every document reaches
//! this module through a [`Resolver`]. The default resolver is filesystem
//! only and never touches the network.
//!
//! References are not resolved here. A `ref`, `base` or `type` attribute
//! becomes a placeholder id plus an entry in a fixup list, which
//! the compile phase patches once every document has been read — the
//! classic linker split, and the only way to cope with the circular import
//! graphs that real schemas have.

use crate::datatypes::{
    Builtin, BuiltinKind, ExplicitTimezone, Facet, FacetKind, FacetSet, Variety, WhiteSpace,
};
use crate::diagnostics::{DiagCode, Diagnostic, Diagnostics, Span};
use crate::model::*;
use crate::names::{Interner, Namespace, QName, XML, XS, XSI};
use fxhash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};

/// Resolves a `schemaLocation` hint to document bytes.
///
/// Implement this to load schemas from a jar, a database, an in-memory map,
/// or the network — the default never leaves the filesystem.
///
/// # Why bytes, not text
///
/// A schema may declare any encoding, and getting the detection right means
/// a byte-order mark, then the XML declaration, then UTF-8. Returning
/// `String` would make every implementor redo that, and get it wrong in a
/// different way each time. Hand back the bytes; [`crate::encoding`] decodes
/// them once, in one place.
///
/// # Why `Send + Sync`
///
/// So callers can release the GIL around `build()` in the Python bindings,
/// and so a future parallel loader is not blocked by the trait.
pub trait Resolver: Send + Sync {
    /// Resolves `location`, relative to `base` when `base` is known.
    ///
    /// Returns the absolute URI it resolved to and the document's raw bytes.
    fn resolve(&self, location: &str, base: Option<&str>) -> Result<(String, Vec<u8>), String>;
}

/// Loads schema documents from the local filesystem.
#[derive(Debug, Default, Clone)]
pub struct FileResolver {
    /// Extra directories searched when a relative location misses.
    pub search_paths: Vec<PathBuf>,
}

impl FileResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_search_path(mut self, p: impl Into<PathBuf>) -> Self {
        self.search_paths.push(p.into());
        self
    }
}

fn strip_file_scheme(s: &str) -> &str {
    s.strip_prefix("file://").unwrap_or(s)
}

impl Resolver for FileResolver {
    fn resolve(&self, location: &str, base: Option<&str>) -> Result<(String, Vec<u8>), String> {
        if location.starts_with("http://") || location.starts_with("https://") {
            return Err(format!(
                "refusing to fetch `{location}` over the network; \
                 supply a resolver or a local copy"
            ));
        }

        let raw = strip_file_scheme(location);
        let mut candidates: Vec<PathBuf> = Vec::new();

        let p = Path::new(raw);
        if p.is_absolute() {
            candidates.push(p.to_path_buf());
        } else {
            if let Some(b) = base {
                if let Some(dir) = Path::new(strip_file_scheme(b)).parent() {
                    candidates.push(dir.join(raw));
                }
            }
            candidates.push(p.to_path_buf());
            for d in &self.search_paths {
                candidates.push(d.join(raw));
            }
        }

        for c in &candidates {
            if c.is_file() {
                let bytes = std::fs::read(c).map_err(|e| format!("{}: {e}", c.display()))?;
                let abs = c.canonicalize().unwrap_or_else(|_| c.clone());
                return Ok((abs.display().to_string(), bytes));
            }
        }
        Err(format!(
            "not found: {} (tried {})",
            location,
            candidates
                .iter()
                .map(|c| c.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

/// Which version of XSD to process as.
///
/// The two differ in more than added syntax: 1.1 also *relaxes* rules 1.0
/// enforces, most visibly Unique Particle Attribution, where an element
/// particle competing with a wildcard is an error in 1.0 and legal in 1.1.
/// A schema cannot be checked without knowing which is meant.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Version {
    /// XSD 1.0. The default: it is what most shipping schemas are, and it is
    /// the stricter reading, so a 1.0-clean schema is also 1.1-clean.
    #[default]
    Xsd10,
    /// XSD 1.1 — `openContent`, `defaultAttributes`, conditional type
    /// assignment, assertions, and the relaxed UPA rule.
    Xsd11,
}

/// How strictly to treat schemas that break the specification.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Conformance {
    /// Every violation is an error.
    #[default]
    Strict,
    /// Violations that do not prevent building components are downgraded to
    /// warnings. Real schemas ship with dangling imports and UPA breaches
    /// often enough that this mode earns its keep.
    Lax,
}

/// A reference that could not be resolved while reading a document, to be
/// patched once every document has been read.
#[derive(Clone, Debug)]
pub(crate) enum Fixup {
    ElementType {
        element: ElementId,
        name: QName,
        span: Span,
    },
    ElementSubstGroup {
        element: ElementId,
        name: QName,
        span: Span,
    },
    AttributeType {
        attribute: AttributeId,
        name: QName,
        span: Span,
    },
    SimpleBase {
        type_: TypeId,
        name: QName,
        span: Span,
    },
    SimpleItem {
        type_: TypeId,
        name: QName,
        span: Span,
    },
    SimpleMember {
        type_: TypeId,
        index: usize,
        name: QName,
        span: Span,
    },
    ComplexBase {
        type_: TypeId,
        name: QName,
        span: Span,
    },
    ParticleElementRef {
        particle: ParticleId,
        name: QName,
        span: Span,
    },
    ParticleGroupRef {
        particle: ParticleId,
        name: QName,
        span: Span,
    },
    AttrUseRef {
        owner: AttrOwner,
        index: usize,
        name: QName,
        span: Span,
    },
    AttrGroupRef {
        owner: AttrOwner,
        index: usize,
        name: QName,
        span: Span,
    },
    KeyRefRefer {
        idc: IdcId,
        name: QName,
        span: Span,
    },
    /// `<xs:unique ref="…"/>`: the element carries a constraint defined
    /// elsewhere, so the slot holds a placeholder until that one is found.
    ElementIdcRef {
        element: ElementId,
        index: usize,
        name: QName,
        span: Span,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum AttrOwner {
    ComplexType(TypeId),
    AttributeGroup(AttrGroupId),
}

/// The components a `redefine` is replacing, kept so its self-references can
/// still reach the originals.
#[derive(Default, Debug)]
pub(crate) struct Originals {
    types: FxHashMap<QName, TypeId>,
    groups: FxHashMap<QName, GroupId>,
    attribute_groups: FxHashMap<QName, AttrGroupId>,
}

/// Per-document state that local declarations inherit.
#[derive(Clone, Debug)]
struct DocCtx {
    uri: String,
    target_ns: Option<Namespace>,
    element_form_qualified: bool,
    attribute_form_qualified: bool,
    block_default: DerivationSet,
    final_default: DerivationSet,
    /// `xs:defaultAttributes` — an attribute group applied to every complex
    /// type in this document (1.1).
    default_attributes: Option<QName>,
    /// `xs:defaultOpenContent` — open content applied to every complex type
    /// in this document that does not state its own (1.1).
    default_open_content: Option<OpenContent>,
    /// Whether the default open content also reaches types with empty
    /// content.
    default_open_applies_to_empty: bool,
    /// Which XSD this document is being read as, so that every descend point
    /// can apply conditional inclusion (`vc:`).
    version: Version,
    /// Line starts for this document, shared rather than copied: a derived
    /// context (a chameleon include, the document defaults) is the same
    /// document.
    lines: std::sync::Arc<LineIndex>,
}

/// Accumulates components while documents are read.
pub(crate) struct Loader<'r> {
    pub(crate) types: Arena<TypeDefinition>,
    pub(crate) elements: Arena<ElementDecl>,
    pub(crate) attributes: Arena<AttributeDecl>,
    pub(crate) particles: Arena<Particle>,
    pub(crate) model_groups: Arena<ModelGroupDef>,
    pub(crate) attribute_groups: Arena<AttributeGroupDef>,
    pub(crate) identity_constraints: Arena<IdentityConstraint>,
    pub(crate) notations: Arena<NotationDecl>,
    pub(crate) annotations: Arena<Annotation>,

    pub(crate) names: Interner,
    pub(crate) globals: SymbolTables,
    pub(crate) builtins: FxHashMap<Builtin, TypeId>,
    pub(crate) fixups: Vec<Fixup>,
    pub(crate) diags: Diagnostics,
    pub(crate) documents: Vec<SourceDocument>,

    resolver: &'r dyn Resolver,
    mode: Conformance,
    /// Keyed by `(uri, coerced namespace)` — a chameleon include means the
    /// same file yields different components per includer, so the URI alone
    /// is not a cache key.
    seen: FxHashSet<(String, Option<Namespace>)>,
    depth: usize,
    nodes_limit: u32,
    /// Which XSD version to process as; see [`Version`].
    version: Version,
    /// Names this crate installed itself: the built-in types and the `xml:`
    /// attributes. A document redeclaring one is not a duplicate-global
    /// error — the schema-for-schemas declares all 50 built-ins.
    predeclared: FxHashSet<QName>,
    /// True while reading the children of a `redefine`/`override`, where a
    /// name colliding with the included document's is the whole point.
    in_redefine: bool,
    /// Anonymous simple types built for the facets on a `simpleContent`
    /// restriction, paired with the complex type that owns each.
    ///
    /// Their base is the *base complex type's* simple content, which is not
    /// known until `resolve_simple_content` has walked the chain — too late
    /// for the ordinary fixup pass, so they wait here instead.
    pub(crate) simple_content_facets: Vec<(TypeId, TypeId)>,
}

const MAX_DEPTH: usize = 64;

/// Default cap on XML nodes per schema document.
///
/// Generous enough for AUTOSAR-scale schemas, bounded enough that a hostile
/// document cannot exhaust memory before the first component is built.
pub const DEFAULT_NODES_LIMIT: u32 = 10_000_000;

impl<'r> Loader<'r> {
    pub(crate) fn new(resolver: &'r dyn Resolver, mode: Conformance) -> Self {
        let mut l = Self {
            types: Arena::new(),
            elements: Arena::new(),
            attributes: Arena::new(),
            particles: Arena::new(),
            model_groups: Arena::new(),
            attribute_groups: Arena::new(),
            identity_constraints: Arena::new(),
            notations: Arena::new(),
            annotations: Arena::new(),
            names: Interner::new(),
            globals: SymbolTables::default(),
            builtins: FxHashMap::default(),
            fixups: Vec::new(),
            diags: Diagnostics::new(),
            documents: Vec::new(),
            resolver,
            mode,
            seen: FxHashSet::default(),
            depth: 0,
            nodes_limit: DEFAULT_NODES_LIMIT,
            version: Version::default(),
            predeclared: FxHashSet::default(),
            in_redefine: false,
            simple_content_facets: Vec::new(),
        };
        l.install_builtins();
        l.install_xml_attributes();
        l.install_xsi_attributes();
        l
    }

    // -- built-ins ---------------------------------------------------------

    /// Materialises the 49 built-in types as real components, so a reference
    /// to `xs:string` resolves the same way as one to a user type.
    fn install_builtins(&mut self) {
        let xs = self.names.namespace(XS);
        let span = Span::new("http://www.w3.org/2001/XMLSchema", 0);

        // Pass 1: allocate, so bases can point at ids that already exist.
        for &b in Builtin::all() {
            let local = self.names.intern(b.local_name());
            let name = QName {
                ns: Some(xs),
                local,
            };
            let id = if b == Builtin::AnyType {
                let t = ComplexType {
                    name: Some(name),
                    base: TypeId::PLACEHOLDER,
                    derivation: DerivationMethod::Restriction,
                    content: ContentType::Mixed(ParticleId::PLACEHOLDER),
                    attribute_uses: Vec::new(),
                    attribute_group_refs: Vec::new(),
                    attribute_wildcard: Some(Wildcard {
                        namespace: NamespaceConstraint::Any,
                        process_contents: ProcessContents::Lax,
                        not_qname: Vec::new(),
                        not_defined: false,
                        not_defined_sibling: false,
                    }),
                    open_content: None,
                    is_abstract: false,
                    block: DerivationSet::default(),
                    final_: DerivationSet::default(),
                    annotation: None,
                    span: span.clone(),
                };
                TypeId(self.types.push(TypeDefinition::Complex(t)))
            } else {
                let variety = b.variety().unwrap_or(Variety::Atomic);
                let t = SimpleType {
                    name: Some(name),
                    base: TypeId::PLACEHOLDER,
                    variety,
                    primitive: b.primitive(),
                    builtin: Some(b),
                    item_type: None,
                    member_types: Vec::new(),
                    facets: FacetSet::new(),
                    final_: DerivationSet::default(),
                    annotation: None,
                    span: span.clone(),
                };
                TypeId(self.types.push(TypeDefinition::Simple(t)))
            };
            self.builtins.insert(b, id);
            self.globals.types.insert(name, id);
            self.predeclared.insert(name);
        }

        // Pass 2: link bases and list item types.
        for &b in Builtin::all() {
            let id = self.builtins[&b];
            let base = b.base().map(|x| self.builtins[&x]).unwrap_or(id);
            let item = match b.kind() {
                BuiltinKind::List(item) => Some(self.builtins[&item]),
                _ => None,
            };
            match self.types.get_mut(id.0) {
                TypeDefinition::Simple(t) => {
                    t.base = base;
                    t.item_type = item;
                }
                TypeDefinition::Complex(t) => t.base = base,
            }
        }
    }

    /// Declares the four attributes in the `xsi:` namespace.
    ///
    /// `xsi:type`, `xsi:nil` and the two schemaLocation hints are available
    /// to every schema without declaration — a schema may reference them by
    /// `ref` and real ones do.
    fn install_xsi_attributes(&mut self) {
        let xsi_ns = self.names.namespace(XSI);
        let span = Span::new(XSI, 0);
        for (local, ty) in [
            ("type", Builtin::QName),
            ("nil", Builtin::Boolean),
            ("schemaLocation", Builtin::AnySimpleType),
            ("noNamespaceSchemaLocation", Builtin::AnyUri),
        ] {
            let name = QName {
                ns: Some(xsi_ns),
                local: self.names.intern(local),
            };
            let id = AttributeId(self.attributes.push(AttributeDecl {
                name,
                type_id: self.builtins[&ty],
                scope: Scope::Global,
                value_constraint: None,
                annotation: None,
                span: span.clone(),
            }));
            self.globals.attributes.insert(name, id);
            self.predeclared.insert(name);
        }
    }

    /// Declares the four attributes in the `xml:` namespace.
    ///
    /// They are available to every schema without an `xs:import`, which is
    /// what keeps `xml:lang` from forcing a network fetch of `xml.xsd`.
    fn install_xml_attributes(&mut self) {
        let xml_ns = self.names.namespace(XML);
        let span = Span::new(XML, 0);
        for (local, ty) in [
            ("lang", Builtin::Language),
            ("space", Builtin::NcName),
            ("base", Builtin::AnyUri),
            ("id", Builtin::Id),
        ] {
            let name = QName {
                ns: Some(xml_ns),
                local: self.names.intern(local),
            };
            let id = AttributeId(self.attributes.push(AttributeDecl {
                name,
                type_id: self.builtins[&ty],
                scope: Scope::Global,
                value_constraint: None,
                annotation: None,
                span: span.clone(),
            }));
            self.globals.attributes.insert(name, id);
            self.predeclared.insert(name);
        }
    }

    // -- entry points ------------------------------------------------------

    pub(crate) fn set_nodes_limit(&mut self, limit: u32) {
        self.nodes_limit = limit;
    }

    pub(crate) fn set_version(&mut self, v: Version) {
        self.version = v;
    }

    pub(crate) fn version(&self) -> Version {
        self.version
    }

    pub(crate) fn load_uri(&mut self, location: &str, base: Option<&str>) {
        match self.resolver.resolve(location, base) {
            Ok((uri, bytes)) => self.load_bytes(&bytes, &uri, None),
            Err(e) => self.diags.push(
                Diagnostic::error(DiagCode::UnresolvedSchemaLocation, e)
                    .with_help("add a search path, or supply a custom Resolver"),
            ),
        }
    }

    /// Decodes a document and loads it.
    ///
    /// An encoding failure is reported as an encoding failure, not as a
    /// missing file — the file was found and read perfectly well.
    pub(crate) fn load_bytes(&mut self, bytes: &[u8], uri: &str, coerce_ns: Option<Namespace>) {
        match crate::encoding::decode_document(bytes, uri) {
            Ok(d) => self.load_text(&d.text, uri, coerce_ns),
            Err(diag) => self.diags.push(diag),
        }
    }

    pub(crate) fn load_text(&mut self, text: &str, uri: &str, coerce_ns: Option<Namespace>) {
        if self.depth > MAX_DEPTH {
            self.diags.push(Diagnostic::error(
                DiagCode::CircularDefinition,
                format!("schema include/import nesting exceeded {MAX_DEPTH} levels"),
            ));
            return;
        }

        // Internal DTD subsets are permitted: real schemas use parameter
        // entities, the W3C's own schema-for-schemas among them. This does
        // *not* open XXE — roxmltree performs no I/O, so an external entity
        // can never be fetched — and it detects entity-reference loops, which
        // closes the billion-laughs vector. `nodes_limit` bounds the rest.
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            nodes_limit: self.nodes_limit,
        };
        let doc = match roxmltree::Document::parse_with_options(text, opts) {
            Ok(d) => d,
            Err(e) => {
                self.diags.push(
                    Diagnostic::error(DiagCode::MalformedXml, e.to_string()).at(Span::new(uri, 0)),
                );
                return;
            }
        };

        let root = doc.root_element();
        if !(root.tag_name().namespace() == Some(XS) && root.tag_name().name() == "schema") {
            self.diags.push(
                Diagnostic::error(
                    DiagCode::NotASchemaDocument,
                    format!(
                        "root element is `{}`, expected `xs:schema`",
                        root.tag_name().name()
                    ),
                )
                .at(Span::new(
                    uri,
                    LineIndex::build(text).line(root.range().start),
                )),
            );
            return;
        }

        // The conditions can sit on `xs:schema` itself, which excludes the
        // whole document — the idiom for shipping one file that a processor of
        // another version reads as empty. Nothing is wrong with it, so there is
        // nothing to report either.
        if !vc_included(&root, self.version()) {
            return;
        }

        let declared_ns = root.attribute("targetNamespace");
        let target_ns = match (declared_ns, coerce_ns) {
            // Chameleon include: no targetNamespace of its own, so the
            // includer's namespace is adopted for every component here.
            (None, Some(ns)) => Some(ns),
            (Some(t), _) => self.names.opt_namespace(t),
            (None, None) => None,
        };
        let chameleon = declared_ns.is_none() && coerce_ns.is_some();

        let key = (uri.to_string(), target_ns);
        if !self.seen.insert(key) {
            return;
        }

        let ctx = DocCtx {
            uri: uri.to_string(),
            target_ns,
            element_form_qualified: root.attribute("elementFormDefault") == Some("qualified"),
            attribute_form_qualified: root.attribute("attributeFormDefault") == Some("qualified"),
            block_default: DerivationSet::parse(root.attribute("blockDefault").unwrap_or("")),
            final_default: DerivationSet::parse(root.attribute("finalDefault").unwrap_or("")),
            default_attributes: None,
            default_open_content: None,
            default_open_applies_to_empty: false,
            version: self.version(),
            lines: std::sync::Arc::new(LineIndex::build(text)),
        };
        let ctx = self.read_document_defaults(root, ctx);

        self.documents.push(SourceDocument {
            uri: uri.to_string(),
            target_namespace: target_ns,
            chameleon,
            version: root.attribute("version").map(str::to_string),
        });

        self.depth += 1;
        self.read_schema_body(root, &ctx);
        self.depth -= 1;
    }

    // -- schema body -------------------------------------------------------

    fn read_schema_body(&mut self, root: roxmltree::Node, ctx: &DocCtx) {
        check_representation(root, ctx, &mut self.diags);

        // Composition first: everything a later reference might name has to
        // exist before the references are collected.
        for child in root.children().filter(|n| reads(n, ctx.version)) {
            match child.tag_name().name() {
                "include" => self.read_include(child, ctx),
                "import" => self.read_import(child, ctx),
                "redefine" => self.read_redefine(child, ctx),
                "override" => self.read_override(child, ctx),
                _ => {}
            }
        }

        for child in root.children().filter(|n| reads(n, ctx.version)) {
            let span = Span::new(&ctx.uri, line_of(ctx, child));
            match child.tag_name().name() {
                "include" | "import" | "redefine" | "override" | "annotation" => {}
                "element" => {
                    let id = self.read_element_decl(child, ctx, Scope::Global, true);
                    self.register_global_element(id, span);
                }
                "attribute" => {
                    let id = self.read_attribute_decl(child, ctx, Scope::Global, true);
                    self.register_global_attribute(id, span);
                }
                "simpleType" => {
                    let id = self.read_simple_type(child, ctx, true);
                    self.register_global_type(id, span);
                }
                "complexType" => {
                    let id = self.read_complex_type(child, ctx, true);
                    self.register_global_type(id, span);
                }
                "group" => self.read_group_def(child, ctx),
                "attributeGroup" => self.read_attribute_group_def(child, ctx),
                "notation" => self.read_notation(child, ctx),
                other => self.diags.push(
                    Diagnostic::warning(
                        DiagCode::UnknownSchemaElement,
                        format!("ignoring unrecognised top-level element `xs:{other}`"),
                    )
                    .at(span),
                ),
            }
        }
    }

    fn read_include(&mut self, node: roxmltree::Node, ctx: &DocCtx) {
        let Some(loc) = node.attribute("schemaLocation") else {
            self.diags.push(
                Diagnostic::error(
                    DiagCode::MissingAttribute,
                    "`xs:include` needs a schemaLocation",
                )
                .at(Span::new(&ctx.uri, line_of(ctx, node))),
            );
            return;
        };
        match self.resolver.resolve(loc, Some(&ctx.uri)) {
            // The includer's namespace is passed down so a document with no
            // targetNamespace of its own is absorbed into it.
            Ok((uri, bytes)) => self.load_bytes(&bytes, &uri, ctx.target_ns),
            Err(e) => self.push_resolution_failure(e, node, ctx),
        }
    }

    /// Reads the document-level XSD 1.1 defaults: `xs:defaultAttributes` and
    /// `xs:defaultOpenContent`.
    ///
    /// Both are declared once on `xs:schema` and apply to every complex type
    /// in that document, which is why they live on the per-document context
    /// rather than being looked up per type.
    fn read_document_defaults(&mut self, root: roxmltree::Node, mut ctx: DocCtx) -> DocCtx {
        if let Some(v) = root.attribute("defaultAttributes") {
            let span = Span::new(&ctx.uri, line_of(&ctx, root));
            self.require_xsd11("defaultAttributes", &span);
            ctx.default_attributes = self.attr_qname(root, v, &ctx, &span);
        }
        if let Some(n) = root
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| c.tag_name().name() == "defaultOpenContent")
        {
            let span = Span::new(&ctx.uri, line_of(&ctx, n));
            self.require_xsd11("defaultOpenContent", &span);
            ctx.default_open_applies_to_empty = flag(n, "appliesToEmpty");
            ctx.default_open_content = self.read_open_content(n, &ctx);
        }
        ctx
    }

    /// Reports a construct that only XSD 1.1 defines, when reading as 1.0.
    fn require_xsd11(&mut self, what: &str, span: &Span) {
        if self.version == Version::Xsd10 {
            self.diags.push(
                Diagnostic::warning(
                    DiagCode::Unsupported,
                    format!("`xs:{what}` is an XSD 1.1 construct and is ignored"),
                )
                .at(span.clone())
                .with_help("build with `Version::Xsd11` to process it"),
            );
        }
    }

    /// Reads an `xs:openContent` or `xs:defaultOpenContent` element.
    fn read_open_content(&mut self, node: roxmltree::Node, ctx: &DocCtx) -> Option<OpenContent> {
        let mode = match node.attribute("mode").unwrap_or("interleave") {
            "none" => return None,
            "suffix" => OpenContentMode::Suffix,
            _ => OpenContentMode::Interleave,
        };
        let any = node
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| c.tag_name().name() == "any")?;
        Some(OpenContent {
            mode,
            wildcard: self.read_wildcard(any, ctx),
        })
    }

    /// `xs:redefine` — include a document, then modify some of its
    /// components.
    ///
    /// The awkward part is that a redefinition refers to **itself** by name to
    /// mean the *original*: `<xs:complexType name="T"><xs:extension
    /// base="T">` extends the T that was just included, not the T being
    /// declared. Every other reference means what it usually means.
    ///
    /// So the originals are captured before the redefinitions are read, and
    /// any reference a redefinition makes to a name it is itself redefining is
    /// resolved immediately against that capture, before the new component
    /// takes the name.
    fn read_redefine(&mut self, node: roxmltree::Node, ctx: &DocCtx) {
        if !self.include_target(node, ctx) {
            return;
        }
        let originals = self.capture_originals(node, ctx);
        self.read_modifications(node, ctx, Some(&originals));
    }

    /// `xs:override` — include a document, then replace some of its
    /// components outright.
    ///
    /// Unlike `redefine`, an override's own references mean the **new**
    /// components, so nothing needs capturing. XSD 1.1 also applies overrides
    /// transitively through the included document's own includes; that part is
    /// not implemented, and is reported rather than assumed.
    fn read_override(&mut self, node: roxmltree::Node, ctx: &DocCtx) {
        if !self.include_target(node, ctx) {
            return;
        }
        self.read_modifications(node, ctx, None);
    }

    /// Loads the document a `redefine`/`override` names, as an include would.
    fn include_target(&mut self, node: roxmltree::Node, ctx: &DocCtx) -> bool {
        let Some(loc) = node.attribute("schemaLocation") else {
            let what = node.tag_name().name();
            self.diags.push(
                Diagnostic::error(
                    DiagCode::MissingAttribute,
                    format!("`xs:{what}` needs a schemaLocation"),
                )
                .at(Span::new(&ctx.uri, line_of(ctx, node))),
            );
            return false;
        };
        match self.resolver.resolve(loc, Some(&ctx.uri)) {
            Ok((uri, bytes)) => {
                self.load_bytes(&bytes, &uri, ctx.target_ns);
                true
            }
            Err(e) => {
                self.push_resolution_failure(e, node, ctx);
                false
            }
        }
    }

    /// Records the components a `redefine` is about to replace, so its
    /// self-references can still reach them.
    fn capture_originals(&mut self, node: roxmltree::Node, ctx: &DocCtx) -> Originals {
        let mut out = Originals::default();
        for c in node.children().filter(|n| reads(n, ctx.version)) {
            let Some(local) = c.attribute("name") else {
                continue;
            };
            let name = self.qualified_name(local, true, ctx);
            match c.tag_name().name() {
                "simpleType" | "complexType" => {
                    if let Some(id) = self.globals.types.get(&name) {
                        out.types.insert(name, *id);
                    }
                }
                "group" => {
                    if let Some(id) = self.globals.model_groups.get(&name) {
                        out.groups.insert(name, *id);
                    }
                }
                "attributeGroup" => {
                    if let Some(id) = self.globals.attribute_groups.get(&name) {
                        out.attribute_groups.insert(name, *id);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Reads the children of a `redefine`/`override` and installs them over
    /// whatever the included document declared.
    fn read_modifications(
        &mut self,
        node: roxmltree::Node,
        ctx: &DocCtx,
        originals: Option<&Originals>,
    ) {
        // Replacing a name the included document declared is the point here,
        // not a duplicate-global error.
        let outer = std::mem::replace(&mut self.in_redefine, true);
        // `xs:redefine` may revise only types and the two kinds of group.
        // `xs:override` replaces *any* top-level component, which is most of
        // the difference between them.
        let overriding = node.tag_name().name() == "override";
        for c in node.children().filter(|n| reads(n, ctx.version)) {
            let span = Span::new(&ctx.uri, line_of(ctx, c));
            let before = self.fixups.len();
            let kind = c.tag_name().name();

            match kind {
                "simpleType" | "complexType" => {
                    let id = if kind == "simpleType" {
                        self.read_simple_type(c, ctx, true)
                    } else {
                        self.read_complex_type(c, ctx, true)
                    };
                    if let Some(o) = originals {
                        self.pin_self_references(before, o);
                    }
                    if let Some(name) = self.types.get(id.0).name() {
                        self.replace_global_type(name, id, span);
                    }
                }
                "group" => {
                    self.read_group_def(c, ctx);
                    if let Some(o) = originals {
                        self.pin_self_references(before, o);
                    }
                }
                "attributeGroup" => {
                    self.read_attribute_group_def(c, ctx);
                    if let Some(o) = originals {
                        self.pin_self_references(before, o);
                    }
                }
                "element" if overriding => {
                    let id = self.read_element_decl(c, ctx, Scope::Global, true);
                    let name = self.elements.get(id.0).name;
                    self.replace_global(name, span, |s| {
                        s.globals.elements.insert(name, id);
                    });
                }
                "attribute" if overriding => {
                    let id = self.read_attribute_decl(c, ctx, Scope::Global, true);
                    let name = self.attributes.get(id.0).name;
                    self.replace_global(name, span, |s| {
                        s.globals.attributes.insert(name, id);
                    });
                }
                "notation" if overriding => self.read_notation(c, ctx),
                "annotation" => {}
                other => {
                    let what = node.tag_name().name();
                    self.diags.push(
                        Diagnostic::warning(
                            DiagCode::UnknownSchemaElement,
                            format!("ignoring `xs:{other}` inside `xs:{what}`"),
                        )
                        .at(span),
                    );
                }
            }
        }
        self.in_redefine = outer;
    }

    /// Points a redefinition's self-references at the original component.
    ///
    /// Only the reference kinds a redefinition can legally make to itself:
    /// a type's base, and a group or attribute group reference.
    fn pin_self_references(&mut self, from: usize, originals: &Originals) {
        let pending: Vec<Fixup> = self.fixups.drain(from..).collect();
        for f in pending {
            match &f {
                Fixup::SimpleBase { type_, name, .. } => {
                    if let Some(orig) = originals.types.get(name) {
                        let (type_, orig) = (*type_, *orig);
                        self.set_simple_base(type_, orig);
                        continue;
                    }
                }
                Fixup::ComplexBase { type_, name, .. } => {
                    if let Some(orig) = originals.types.get(name) {
                        if let TypeDefinition::Complex(t) = self.types.get_mut(type_.0) {
                            t.base = *orig;
                        }
                        continue;
                    }
                }
                Fixup::ParticleGroupRef { particle, name, .. } => {
                    if let Some(orig) = originals.groups.get(name) {
                        self.particles.get_mut(particle.0).term = Term::GroupRef(*orig);
                        continue;
                    }
                }
                Fixup::AttrGroupRef {
                    owner, index, name, ..
                } => {
                    if let Some(orig) = originals.attribute_groups.get(name) {
                        let (owner, index, orig) = (*owner, *index, *orig);
                        self.set_attr_group_ref(owner, index, orig);
                        continue;
                    }
                }
                _ => {}
            }
            // Not a self-reference; resolve it normally at compile time.
            self.fixups.push(f);
        }
    }

    fn set_simple_base(&mut self, type_: TypeId, base: TypeId) {
        let primitive = match self.types.get(base.0) {
            TypeDefinition::Simple(b) => b.primitive.or(b.builtin),
            TypeDefinition::Complex(_) => None,
        };
        if let TypeDefinition::Simple(t) = self.types.get_mut(type_.0) {
            t.base = base;
            if t.primitive.is_none() {
                t.primitive = primitive;
            }
        }
    }

    fn set_attr_group_ref(&mut self, owner: AttrOwner, index: usize, id: AttrGroupId) {
        let refs = match owner {
            AttrOwner::ComplexType(t) => match self.types.get_mut(t.0) {
                TypeDefinition::Complex(c) => Some(&mut c.attribute_group_refs),
                TypeDefinition::Simple(_) => None,
            },
            AttrOwner::AttributeGroup(g) => {
                Some(&mut self.attribute_groups.get_mut(g.0).attribute_group_refs)
            }
        };
        if let Some(refs) = refs {
            if index < refs.len() {
                refs[index] = id;
            }
        }
    }

    /// Installs a redefined type over the one the included document declared,
    /// without the duplicate-global complaint a fresh declaration would draw.
    fn replace_global_type(&mut self, name: QName, id: TypeId, span: Span) {
        if self.predeclared.contains(&name) {
            let shown = self.names.display(name);
            self.diags.push(
                Diagnostic::error(
                    DiagCode::DuplicateGlobal,
                    format!("cannot redefine the built-in type `{shown}`"),
                )
                .at(span),
            );
            return;
        }
        self.globals.types.insert(name, id);
    }

    /// Installs a component over whatever the overridden document declared.
    ///
    /// Displacing a name is the point of `xs:override`, so this is not the
    /// duplicate-global error `register_global_*` would raise — but a
    /// predeclared built-in still may not be displaced, or `Schemas::builtin`
    /// would stop being a stable handle.
    fn replace_global(&mut self, name: QName, span: Span, insert: impl FnOnce(&mut Self)) {
        if self.predeclared.contains(&name) {
            let shown = self.names.display(name);
            self.diags.push(
                Diagnostic::error(
                    DiagCode::DuplicateGlobal,
                    format!("cannot override the built-in `{shown}`"),
                )
                .at(span),
            );
            return;
        }
        insert(self);
    }

    fn read_import(&mut self, node: roxmltree::Node, ctx: &DocCtx) {
        let Some(loc) = node.attribute("schemaLocation") else {
            // Legal: schemaLocation is a hint, and a namespace may already be
            // present from another document or be supplied by the caller.
            return;
        };
        match self.resolver.resolve(loc, Some(&ctx.uri)) {
            Ok((uri, bytes)) => self.load_bytes(&bytes, &uri, None),
            Err(e) => self.push_resolution_failure(e, node, ctx),
        }
    }

    fn push_resolution_failure(&mut self, e: String, node: roxmltree::Node, ctx: &DocCtx) {
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        let d = Diagnostic::error(DiagCode::UnresolvedSchemaLocation, e)
            .at(span)
            .with_help("`schemaLocation` is a hint; add a search path or a custom Resolver");
        self.diags.push(if self.mode == Conformance::Lax {
            Diagnostic {
                severity: crate::diagnostics::Severity::Warning,
                ..d
            }
        } else {
            d
        });
    }

    // -- global registration ----------------------------------------------

    fn register_global_type(&mut self, id: TypeId, span: Span) {
        let Some(name) = self.types.get(id.0).name() else {
            return;
        };
        self.insert_global(SymbolSpace::Type, name, span, |s| {
            s.globals.types.insert(name, id).is_some()
        });
    }

    fn register_global_element(&mut self, id: ElementId, span: Span) {
        let name = self.elements.get(id.0).name;
        self.insert_global(SymbolSpace::Element, name, span, |s| {
            s.globals.elements.insert(name, id).is_some()
        });
    }

    fn register_global_attribute(&mut self, id: AttributeId, span: Span) {
        let name = self.attributes.get(id.0).name;
        self.insert_global(SymbolSpace::Attribute, name, span, |s| {
            s.globals.attributes.insert(name, id).is_some()
        });
    }

    fn insert_global(
        &mut self,
        space: SymbolSpace,
        name: QName,
        span: Span,
        insert: impl FnOnce(&mut Self) -> bool,
    ) {
        // A document redeclaring something we installed keeps ours, so that
        // `Schemas::builtin` stays a stable handle. Not a user error.
        if self.predeclared.contains(&name) {
            return;
        }
        if insert(self) {
            let shown = self.names.display(name);
            self.diags.push(
                Diagnostic::error(
                    DiagCode::DuplicateGlobal,
                    format!("duplicate global {} `{shown}`", space.as_str()),
                )
                .at(span)
                .with_help("names collide only within one symbol space and namespace"),
            );
        }
    }

    // -- element declarations ---------------------------------------------

    fn read_element_decl(
        &mut self,
        node: roxmltree::Node,
        ctx: &DocCtx,
        scope: Scope,
        global: bool,
    ) -> ElementId {
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        let local = node.attribute("name").unwrap_or_default();
        if local.is_empty() {
            self.diags.push(
                Diagnostic::error(
                    DiagCode::MissingAttribute,
                    "`xs:element` needs a name or a ref",
                )
                .at(span.clone()),
            );
        }
        let qualified =
            global || ctx.element_form_qualified || node.attribute("form") == Some("qualified");
        let name = self.local_name(node, local, qualified, global, ctx);

        let annotation = self.read_annotation(node, ctx);
        let id = ElementId(
            self.elements.push(ElementDecl {
                name,
                type_id: TypeId::PLACEHOLDER,
                scope,
                nillable: flag(node, "nillable"),
                is_abstract: flag(node, "abstract"),
                substitution_group: Vec::new(),
                value_constraint: value_constraint(node),
                block: node
                    .attribute("block")
                    .map(DerivationSet::parse)
                    .unwrap_or(ctx.block_default),
                final_: node
                    .attribute("final")
                    .map(DerivationSet::parse)
                    .unwrap_or(ctx.final_default),
                identity_constraints: Vec::new(),
                annotation,
                span: span.clone(),
            }),
        );

        // type="..." or an inline simpleType/complexType, never both.
        let inline = node
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| matches!(c.tag_name().name(), "simpleType" | "complexType"));
        match (node.attribute("type"), inline) {
            (Some(t), None) => {
                if let Some(q) = self.attr_qname(node, t, ctx, &span) {
                    self.fixups.push(Fixup::ElementType {
                        element: id,
                        name: q,
                        span: span.clone(),
                    });
                }
            }
            (None, Some(c)) => {
                let tid = if c.tag_name().name() == "simpleType" {
                    self.read_simple_type(c, ctx, false)
                } else {
                    self.read_complex_type(c, ctx, false)
                };
                self.elements.get_mut(id.0).type_id = tid;
            }
            (Some(_), Some(_)) => {
                self.diags.push(
                    Diagnostic::error(
                        DiagCode::ConflictingTypeDefinition,
                        "`xs:element` has both a `type` attribute and an inline type",
                    )
                    .at(span.clone()),
                );
                self.elements.get_mut(id.0).type_id = self.builtins[&Builtin::AnyType];
            }
            // No type given: xs:anyType, per the spec's default.
            (None, None) => {
                self.elements.get_mut(id.0).type_id = self.builtins[&Builtin::AnyType];
            }
        }

        if let Some(sg) = node.attribute("substitutionGroup") {
            for tok in sg.split_whitespace() {
                if let Some(q) = self.attr_qname(node, tok, ctx, &span) {
                    self.fixups.push(Fixup::ElementSubstGroup {
                        element: id,
                        name: q,
                        span: span.clone(),
                    });
                }
            }
        }

        let idcs = self.read_identity_constraints(node, ctx, id);
        self.elements.get_mut(id.0).identity_constraints = idcs;
        id
    }

    fn read_identity_constraints(
        &mut self,
        node: roxmltree::Node,
        ctx: &DocCtx,
        owner: ElementId,
    ) -> Vec<IdcId> {
        let mut out = Vec::new();
        for c in node.children().filter(|n| reads(n, ctx.version)) {
            let kind = match c.tag_name().name() {
                "unique" => IdcKind::Unique,
                "key" => IdcKind::Key,
                "keyref" => IdcKind::KeyRef,
                _ => continue,
            };
            let span = Span::new(&ctx.uri, line_of(ctx, c));

            // XSD 1.1 lets a constraint be *referenced* rather than defined:
            // `<xs:unique ref="a:u1"/>` hangs the constraint named there off
            // this element too. There is no new component, and nothing to
            // register — reading it as a definition gives it the empty name,
            // and the second such reference then collides with the first.
            match (c.attribute("ref"), c.attribute("name")) {
                (Some(r), None) => {
                    if let Some(q) = self.attr_qname(c, r, ctx, &span) {
                        self.fixups.push(Fixup::ElementIdcRef {
                            element: owner,
                            index: out.len(),
                            name: q,
                            span,
                        });
                        out.push(IdcId::PLACEHOLDER);
                    }
                    continue;
                }
                (Some(_), Some(_)) => {
                    self.diags.push(
                        Diagnostic::error(
                            DiagCode::ConflictingTypeDefinition,
                            format!("`xs:{}` has both `name` and `ref`", c.tag_name().name()),
                        )
                        .at(span),
                    );
                    continue;
                }
                (None, None) => {
                    self.diags.push(
                        Diagnostic::error(
                            DiagCode::MissingAttribute,
                            format!("`xs:{}` needs a `name` or a `ref`", c.tag_name().name()),
                        )
                        .at(span),
                    );
                    continue;
                }
                (None, Some(_)) => {}
            }

            let name = self.qualified_name(c.attribute("name").unwrap_or_default(), true, ctx);
            let selector = c
                .children()
                .filter(|n| reads(n, ctx.version))
                .find(|n| n.tag_name().name() == "selector")
                .and_then(|n| n.attribute("xpath"))
                .unwrap_or_default()
                .to_string();
            let fields: Vec<String> = c
                .children()
                .filter(|n| reads(n, ctx.version))
                .filter(|n| n.tag_name().name() == "field")
                .filter_map(|n| n.attribute("xpath"))
                .map(str::to_string)
                .collect();
            let annotation = self.read_annotation(c, ctx);
            // These paths' prefixes bind here, in the schema document, so
            // they are parsed now rather than carried as text into a
            // validator that could no longer resolve them.
            let selector_node = c
                .children()
                .filter(|n| reads(n, ctx.version))
                .find(|n| n.tag_name().name() == "selector");
            let selector_paths =
                self.read_xpath(&selector, false, selector_node.unwrap_or(c), ctx, &span);
            let field_nodes: Vec<_> = c
                .children()
                .filter(|n| reads(n, ctx.version))
                .filter(|n| n.tag_name().name() == "field")
                .collect();
            let field_paths = fields
                .iter()
                .zip(field_nodes.iter().chain(std::iter::repeat(&c)))
                .map(|(f, n)| self.read_xpath(f, true, *n, ctx, &span))
                .collect();
            let id = IdcId(self.identity_constraints.push(IdentityConstraint {
                name,
                kind,
                selector,
                fields,
                selector_paths,
                field_paths,
                refer: None,
                annotation,
                span: span.clone(),
            }));
            if self.globals.identity_constraints.insert(name, id).is_some() {
                let shown = self.names.display(name);
                self.diags.push(
                    Diagnostic::error(
                        DiagCode::DuplicateGlobal,
                        format!("duplicate identity constraint `{shown}`"),
                    )
                    .at(span.clone()),
                );
            }
            if kind == IdcKind::KeyRef {
                if let Some(r) = c.attribute("refer") {
                    if let Some(q) = self.attr_qname(c, r, ctx, &span) {
                        self.fixups.push(Fixup::KeyRefRefer {
                            idc: id,
                            name: q,
                            span,
                        });
                    }
                }
            }
            out.push(id);
        }
        out
    }

    // -- attribute declarations -------------------------------------------

    fn read_attribute_decl(
        &mut self,
        node: roxmltree::Node,
        ctx: &DocCtx,
        scope: Scope,
        global: bool,
    ) -> AttributeId {
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        let local = node.attribute("name").unwrap_or_default();
        let qualified =
            global || ctx.attribute_form_qualified || node.attribute("form") == Some("qualified");
        let name = self.local_name(node, local, qualified, global, ctx);
        let annotation = self.read_annotation(node, ctx);

        let id = AttributeId(self.attributes.push(AttributeDecl {
            name,
            type_id: TypeId::PLACEHOLDER,
            scope,
            value_constraint: value_constraint(node),
            annotation,
            span: span.clone(),
        }));

        let inline = node
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| c.tag_name().name() == "simpleType");
        match (node.attribute("type"), inline) {
            (Some(t), _) => {
                if let Some(q) = self.attr_qname(node, t, ctx, &span) {
                    self.fixups.push(Fixup::AttributeType {
                        attribute: id,
                        name: q,
                        span,
                    });
                }
            }
            (None, Some(c)) => {
                let tid = self.read_simple_type(c, ctx, false);
                self.attributes.get_mut(id.0).type_id = tid;
            }
            (None, None) => {
                self.attributes.get_mut(id.0).type_id = self.builtins[&Builtin::AnySimpleType];
            }
        }
        id
    }

    /// Reads `xs:attribute` and `xs:attributeGroup` children into uses,
    /// returning the wildcard if an `xs:anyAttribute` was present.
    fn read_attribute_uses(
        &mut self,
        parent: roxmltree::Node,
        ctx: &DocCtx,
        owner: AttrOwner,
        scope: Scope,
    ) -> (Vec<AttributeUse>, Vec<AttrGroupId>, Option<Wildcard>) {
        let mut uses = Vec::new();
        let mut groups = Vec::new();
        let mut wildcard = None;

        for c in parent.children().filter(|n| reads(n, ctx.version)) {
            let span = Span::new(&ctx.uri, line_of(ctx, c));
            match c.tag_name().name() {
                "attribute" => {
                    let kind = match c.attribute("use") {
                        Some("required") => AttributeUseKind::Required,
                        Some("prohibited") => AttributeUseKind::Prohibited,
                        _ => AttributeUseKind::Optional,
                    };
                    let attribute = match c.attribute("ref") {
                        Some(r) => {
                            if let Some(q) = self.attr_qname(c, r, ctx, &span) {
                                self.fixups.push(Fixup::AttrUseRef {
                                    owner,
                                    index: uses.len(),
                                    name: q,
                                    span,
                                });
                            }
                            AttributeId::PLACEHOLDER
                        }
                        None => self.read_attribute_decl(c, ctx, scope, false),
                    };
                    uses.push(AttributeUse {
                        attribute,
                        kind,
                        value_constraint: value_constraint(c),
                    });
                }
                "attributeGroup" => {
                    if let Some(r) = c.attribute("ref") {
                        if let Some(q) = self.attr_qname(c, r, ctx, &span) {
                            self.fixups.push(Fixup::AttrGroupRef {
                                owner,
                                index: groups.len(),
                                name: q,
                                span,
                            });
                            groups.push(AttrGroupId::PLACEHOLDER);
                        }
                    }
                }
                "anyAttribute" => wildcard = Some(self.read_wildcard(c, ctx)),
                _ => {}
            }
        }
        (uses, groups, wildcard)
    }

    // -- simple types ------------------------------------------------------

    fn read_simple_type(&mut self, node: roxmltree::Node, ctx: &DocCtx, global: bool) -> TypeId {
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        let name = global
            .then(|| self.qualified_name(node.attribute("name").unwrap_or_default(), true, ctx));
        let annotation = self.read_annotation(node, ctx);

        let id = TypeId(
            self.types.push(TypeDefinition::Simple(SimpleType {
                name,
                base: self.builtins[&Builtin::AnySimpleType],
                variety: Variety::Atomic,
                primitive: None,
                builtin: None,
                item_type: None,
                member_types: Vec::new(),
                facets: FacetSet::new(),
                final_: node
                    .attribute("final")
                    .map(DerivationSet::parse)
                    .unwrap_or(ctx.final_default),
                annotation,
                span: span.clone(),
            })),
        );

        let derivations: Vec<_> = node
            .children()
            .filter(|n| reads(n, ctx.version))
            .filter(|c| matches!(c.tag_name().name(), "restriction" | "list" | "union"))
            .collect();

        if derivations.len() > 1 {
            self.diags.push(
                Diagnostic::error(
                    DiagCode::ConflictingSimpleTypeVariety,
                    "`xs:simpleType` declares more than one of restriction/list/union",
                )
                .at(span.clone()),
            );
        }

        let Some(d) = derivations.first().copied() else {
            return id;
        };

        match d.tag_name().name() {
            "restriction" => {
                self.set_simple_variety(id, Variety::Atomic);
                match d.attribute("base") {
                    Some(b) => {
                        if let Some(q) = self.attr_qname(d, b, ctx, &span) {
                            self.fixups.push(Fixup::SimpleBase {
                                type_: id,
                                name: q,
                                span: span.clone(),
                            });
                        }
                    }
                    None => {
                        if let Some(inner) = d
                            .children()
                            .filter(|n| reads(n, ctx.version))
                            .find(|c| c.tag_name().name() == "simpleType")
                        {
                            let b = self.read_simple_type(inner, ctx, false);
                            self.simple_mut(id).base = b;
                        }
                    }
                }
                let (facets, namespaces) = self.read_facets(d, ctx);
                let mut set = FacetSet::new().restrict(&facets);
                set.namespaces = namespaces;
                self.simple_mut(id).facets = set;
            }
            "list" => {
                self.set_simple_variety(id, Variety::List);
                match d.attribute("itemType") {
                    Some(it) => {
                        if let Some(q) = self.attr_qname(d, it, ctx, &span) {
                            self.fixups.push(Fixup::SimpleItem {
                                type_: id,
                                name: q,
                                span: span.clone(),
                            });
                        }
                    }
                    None => {
                        if let Some(inner) = d
                            .children()
                            .filter(|n| reads(n, ctx.version))
                            .find(|c| c.tag_name().name() == "simpleType")
                        {
                            let it = self.read_simple_type(inner, ctx, false);
                            self.simple_mut(id).item_type = Some(it);
                        }
                    }
                }
            }
            "union" => {
                self.set_simple_variety(id, Variety::Union);
                let mut members = Vec::new();
                if let Some(list) = d.attribute("memberTypes") {
                    for tok in list.split_whitespace() {
                        if let Some(q) = self.attr_qname(d, tok, ctx, &span) {
                            self.fixups.push(Fixup::SimpleMember {
                                type_: id,
                                index: members.len(),
                                name: q,
                                span: span.clone(),
                            });
                            members.push(TypeId::PLACEHOLDER);
                        }
                    }
                }
                for inner in d
                    .children()
                    .filter(|n| reads(n, ctx.version))
                    .filter(|c| c.tag_name().name() == "simpleType")
                {
                    members.push(self.read_simple_type(inner, ctx, false));
                }
                self.simple_mut(id).member_types = members;
            }
            _ => unreachable!(),
        }
        id
    }

    fn simple_mut(&mut self, id: TypeId) -> &mut SimpleType {
        match self.types.get_mut(id.0) {
            TypeDefinition::Simple(t) => t,
            TypeDefinition::Complex(_) => unreachable!("id was created as a simple type"),
        }
    }

    fn set_simple_variety(&mut self, id: TypeId, v: Variety) {
        self.simple_mut(id).variety = v;
    }

    /// The declared facets, and the namespace bindings any QName literal
    /// among them would need.
    fn read_facets(
        &mut self,
        node: roxmltree::Node,
        ctx: &DocCtx,
    ) -> (Vec<Facet>, Vec<(Option<String>, String)>) {
        let mut out = Vec::new();
        let mut namespaces = Vec::new();
        for c in node.children().filter(|n| reads(n, ctx.version)) {
            let v = c.attribute("value").unwrap_or_default();
            let span = || Span::new(&ctx.uri, line_of(ctx, c));
            let facet = match c.tag_name().name() {
                "length" => v.parse().ok().map(Facet::Length),
                "minLength" => v.parse().ok().map(Facet::MinLength),
                "maxLength" => v.parse().ok().map(Facet::MaxLength),
                "pattern" => Some(Facet::Pattern(v.to_string())),
                "enumeration" => {
                    qname_bindings(c, v, &mut namespaces);
                    Some(Facet::Enumeration(v.to_string()))
                }
                "whiteSpace" => match v {
                    "preserve" => Some(Facet::WhiteSpace(WhiteSpace::Preserve)),
                    "replace" => Some(Facet::WhiteSpace(WhiteSpace::Replace)),
                    "collapse" => Some(Facet::WhiteSpace(WhiteSpace::Collapse)),
                    _ => None,
                },
                "maxInclusive" => Some(Facet::MaxInclusive(v.to_string())),
                "maxExclusive" => Some(Facet::MaxExclusive(v.to_string())),
                "minInclusive" => Some(Facet::MinInclusive(v.to_string())),
                "minExclusive" => Some(Facet::MinExclusive(v.to_string())),
                "totalDigits" => v.parse().ok().map(Facet::TotalDigits),
                // Signed, unlike every other count here: a scale of -2 says
                // the value is a multiple of a hundred.
                "minScale" => v.parse().ok().map(Facet::MinScale),
                "maxScale" => v.parse().ok().map(Facet::MaxScale),
                "fractionDigits" => v.parse().ok().map(Facet::FractionDigits),
                "explicitTimezone" => match v {
                    "optional" => Some(Facet::ExplicitTimezone(ExplicitTimezone::Optional)),
                    "required" => Some(Facet::ExplicitTimezone(ExplicitTimezone::Required)),
                    "prohibited" => Some(Facet::ExplicitTimezone(ExplicitTimezone::Prohibited)),
                    _ => None,
                },
                "assertion" => Some(Facet::Assertion(
                    c.attribute("test").unwrap_or_default().to_string(),
                )),
                n if not_a_facet(n) => None,
                other => {
                    self.diags.push(
                        Diagnostic::warning(
                            DiagCode::UnknownSchemaElement,
                            format!("ignoring unrecognised facet `xs:{other}`"),
                        )
                        .at(span()),
                    );
                    None
                }
            };
            match facet {
                Some(f) => out.push(f),
                None if not_a_facet(c.tag_name().name()) => {}
                None => {
                    let n = c.tag_name().name();
                    self.diags.push(
                        Diagnostic::error(
                            DiagCode::InvalidAttributeValue,
                            format!("`xs:{n}` has an invalid value `{v}`"),
                        )
                        .at(span()),
                    );
                }
            }
        }
        self.check_step_facets(&out, &Span::new(&ctx.uri, line_of(ctx, node)));
        (out, namespaces)
    }

    /// Facets that contradict each other *on this restriction element*.
    ///
    /// This has to happen here rather than on the finished model, because
    /// `FacetSet::restrict` composes: a `minExclusive` clears any inherited
    /// `minInclusive`, which is right across steps and erases the evidence
    /// within one. All of it is answerable from the document alone anyway —
    /// nothing here needs the base type resolved.
    fn check_step_facets(&mut self, facets: &[Facet], span: &Span) {
        let mut err = |msg: String| {
            self.diags
                .push(Diagnostic::error(DiagCode::ConflictingFacets, msg).at(span.clone()));
        };

        // Each facet may be declared once per step. Patterns, enumerations and
        // assertions are the exceptions: several of them combine rather than
        // compete.
        let mut seen: Vec<FacetKind> = Vec::new();
        for f in facets {
            let k = f.kind();
            if matches!(
                k,
                FacetKind::Pattern | FacetKind::Enumeration | FacetKind::Assertion
            ) {
                continue;
            }
            if seen.contains(&k) {
                err(format!("`xs:{k}` is declared more than once here"));
            } else {
                seen.push(k);
            }
        }

        let has = |k: FacetKind| seen.contains(&k);
        if has(FacetKind::MinInclusive) && has(FacetKind::MinExclusive) {
            err("`xs:minInclusive` and `xs:minExclusive` cannot both be declared here".into());
        }
        if has(FacetKind::MaxInclusive) && has(FacetKind::MaxExclusive) {
            err("`xs:maxInclusive` and `xs:maxExclusive` cannot both be declared here".into());
        }
    }

    // -- complex types -----------------------------------------------------

    fn read_complex_type(&mut self, node: roxmltree::Node, ctx: &DocCtx, global: bool) -> TypeId {
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        let name = global
            .then(|| self.qualified_name(node.attribute("name").unwrap_or_default(), true, ctx));
        let annotation = self.read_annotation(node, ctx);
        let mixed_attr = flag(node, "mixed");

        let id = TypeId(
            self.types.push(TypeDefinition::Complex(ComplexType {
                name,
                base: self.builtins[&Builtin::AnyType],
                derivation: DerivationMethod::Restriction,
                content: ContentType::Empty,
                attribute_uses: Vec::new(),
                attribute_group_refs: Vec::new(),
                attribute_wildcard: None,
                open_content: None,
                is_abstract: flag(node, "abstract"),
                block: node
                    .attribute("block")
                    .map(DerivationSet::parse)
                    .unwrap_or(ctx.block_default),
                final_: node
                    .attribute("final")
                    .map(DerivationSet::parse)
                    .unwrap_or(ctx.final_default),
                annotation,
                span: span.clone(),
            })),
        );
        let scope = Scope::Local(id);

        let content_wrapper = node
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| matches!(c.tag_name().name(), "simpleContent" | "complexContent"));

        let (body, derivation_node) = match content_wrapper {
            Some(w) => {
                let d = w
                    .children()
                    .filter(|n| reads(n, ctx.version))
                    .find(|c| matches!(c.tag_name().name(), "extension" | "restriction"));
                (w, d)
            }
            None => (node, None),
        };

        let simple_content = content_wrapper
            .map(|w| w.tag_name().name() == "simpleContent")
            .unwrap_or(false);
        let mixed = mixed_attr || content_wrapper.map(|w| flag(w, "mixed")).unwrap_or(false);

        // The node whose children hold particles and attribute uses: the
        // extension/restriction when derived, else the complexType itself.
        let member_node = derivation_node.unwrap_or(body);

        if let Some(d) = derivation_node {
            let method = if d.tag_name().name() == "extension" {
                DerivationMethod::Extension
            } else {
                DerivationMethod::Restriction
            };
            match self.types.get_mut(id.0) {
                TypeDefinition::Complex(t) => t.derivation = method,
                _ => unreachable!(),
            }
            if let Some(b) = d.attribute("base") {
                if let Some(q) = self.attr_qname(d, b, ctx, &span) {
                    self.fixups.push(Fixup::ComplexBase {
                        type_: id,
                        name: q,
                        span: span.clone(),
                    });
                }
            }
        }

        let particle = self.read_content_particle(member_node, ctx, scope);
        let content = if simple_content {
            // The effective simple type is the base's; resolution fills the
            // base in, and `Schemas::simple_content_type` follows it.
            let inline = derivation_node.and_then(|d| {
                d.children()
                    .filter(|n| reads(n, ctx.version))
                    .find(|c| c.tag_name().name() == "simpleType")
            });
            match inline {
                Some(c) => ContentType::Simple(self.read_simple_type(c, ctx, false)),
                // A restriction may narrow the base's simple type with facets
                // written straight under it, with no `xs:simpleType` wrapper.
                // That declares a new simple type whose base is the one being
                // restricted — and dropping the facets, as this used to,
                // makes the whole restriction do nothing.
                None => {
                    let step = derivation_node
                        .filter(|d| d.tag_name().name() == "restriction")
                        .map(|d| self.read_facets(d, ctx));
                    match step {
                        Some((facets, namespaces)) if !facets.is_empty() => {
                            let mut set = FacetSet::new().restrict(&facets);
                            set.namespaces = namespaces;
                            let anon =
                                TypeId(self.types.push(TypeDefinition::Simple(SimpleType {
                                    name: None,
                                    base: TypeId::PLACEHOLDER,
                                    variety: Variety::Atomic,
                                    primitive: None,
                                    builtin: None,
                                    item_type: None,
                                    member_types: Vec::new(),
                                    facets: set,
                                    final_: DerivationSet::default(),
                                    annotation: None,
                                    span: span.clone(),
                                })));
                            self.simple_content_facets.push((anon, id));
                            ContentType::Simple(anon)
                        }
                        _ => ContentType::Simple(TypeId::PLACEHOLDER),
                    }
                }
            }
        } else {
            match particle {
                Some(p) if mixed => ContentType::Mixed(p),
                Some(p) => ContentType::ElementOnly(p),
                None if mixed => ContentType::Mixed(ParticleId::PLACEHOLDER),
                None => ContentType::Empty,
            }
        };

        let (uses, mut groups, wildcard) =
            self.read_attribute_uses(member_node, ctx, AttrOwner::ComplexType(id), scope);

        // `xs:defaultAttributes` reaches every complex type in the document,
        // as though each had named the group itself.
        if self.version == Version::Xsd11 {
            if let Some(default_group) = ctx.default_attributes {
                self.fixups.push(Fixup::AttrGroupRef {
                    owner: AttrOwner::ComplexType(id),
                    index: groups.len(),
                    name: default_group,
                    span: span.clone(),
                });
                groups.push(AttrGroupId::PLACEHOLDER);
            }
        }

        // Open content: the type's own `xs:openContent` when it has one, else
        // the document's default. `mode="none"` is how a type opts out.
        let open_content = if self.version == Version::Xsd11 {
            match member_node
                .children()
                .filter(|n| reads(n, ctx.version))
                .find(|c| c.tag_name().name() == "openContent")
            {
                Some(n) => self.read_open_content(n, ctx),
                None if matches!(content, ContentType::Empty)
                    && !ctx.default_open_applies_to_empty =>
                {
                    None
                }
                None => ctx.default_open_content.clone(),
            }
        } else {
            None
        };

        match self.types.get_mut(id.0) {
            TypeDefinition::Complex(t) => {
                t.content = content;
                t.attribute_uses = uses;
                t.attribute_group_refs = groups;
                t.attribute_wildcard = wildcard;
                t.open_content = open_content;
            }
            _ => unreachable!(),
        }
        id
    }

    /// Reads the single content particle of a complex type body, if present.
    fn read_content_particle(
        &mut self,
        node: roxmltree::Node,
        ctx: &DocCtx,
        scope: Scope,
    ) -> Option<ParticleId> {
        let c = node
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| matches!(c.tag_name().name(), "sequence" | "choice" | "all" | "group"))?;
        self.read_particle(c, ctx, scope)
    }

    fn read_particle(
        &mut self,
        node: roxmltree::Node,
        ctx: &DocCtx,
        scope: Scope,
    ) -> Option<ParticleId> {
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        let (min, max) = self.occurrences(node, &span);

        let term = match node.tag_name().name() {
            "element" => match node.attribute("ref") {
                Some(r) => {
                    let q = self.attr_qname(node, r, ctx, &span)?;
                    let pid = ParticleId(self.particles.push(Particle {
                        min_occurs: min,
                        max_occurs: max,
                        term: Term::Element(ElementId::PLACEHOLDER),
                        span: span.clone(),
                    }));
                    self.fixups.push(Fixup::ParticleElementRef {
                        particle: pid,
                        name: q,
                        span,
                    });
                    return Some(pid);
                }
                None => Term::Element(self.read_element_decl(node, ctx, scope, false)),
            },
            "group" => {
                // A `<xs:group>` particle is always a reference; an inline
                // group would have been `sequence`/`choice`/`all`.
                let r = node.attribute("ref")?;
                let q = self.attr_qname(node, r, ctx, &span)?;
                let pid = ParticleId(self.particles.push(Particle {
                    min_occurs: min,
                    max_occurs: max,
                    term: Term::GroupRef(GroupId::PLACEHOLDER),
                    span: span.clone(),
                }));
                self.fixups.push(Fixup::ParticleGroupRef {
                    particle: pid,
                    name: q,
                    span,
                });
                return Some(pid);
            }
            "any" => Term::Wildcard(self.read_wildcard(node, ctx)),
            "sequence" | "choice" | "all" => {
                let compositor = match node.tag_name().name() {
                    "sequence" => Compositor::Sequence,
                    "choice" => Compositor::Choice,
                    _ => Compositor::All,
                };
                let mut particles = Vec::new();
                for c in node.children().filter(|n| reads(n, ctx.version)) {
                    if matches!(
                        c.tag_name().name(),
                        "element" | "group" | "sequence" | "choice" | "all" | "any"
                    ) {
                        if let Some(p) = self.read_particle(c, ctx, scope) {
                            particles.push(p);
                        }
                    }
                }
                Term::Group(ModelGroup {
                    compositor,
                    particles,
                })
            }
            _ => return None,
        };

        Some(ParticleId(self.particles.push(Particle {
            min_occurs: min,
            max_occurs: max,
            term,
            span,
        })))
    }

    fn occurrences(&mut self, node: roxmltree::Node, span: &Span) -> (u32, MaxOccurs) {
        let min = node
            .attribute("minOccurs")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1);
        let max = match node.attribute("maxOccurs") {
            None => MaxOccurs::Bounded(1),
            Some("unbounded") => MaxOccurs::Unbounded,
            Some(v) => match v.parse::<u32>() {
                Ok(n) => MaxOccurs::Bounded(n),
                Err(_) => {
                    self.diags.push(
                        Diagnostic::error(
                            DiagCode::InvalidAttributeValue,
                            format!("`maxOccurs` is `{v}`, expected a number or `unbounded`"),
                        )
                        .at(span.clone()),
                    );
                    MaxOccurs::Bounded(1)
                }
            },
        };
        if let MaxOccurs::Bounded(m) = max {
            if min > m {
                self.diags.push(
                    Diagnostic::error(
                        DiagCode::InvalidOccurrence,
                        format!("minOccurs ({min}) exceeds maxOccurs ({m})"),
                    )
                    .at(span.clone()),
                );
            }
        }
        (min, max)
    }

    fn read_wildcard(&mut self, node: roxmltree::Node, ctx: &DocCtx) -> Wildcard {
        // Reads a namespace list: URIs plus the two keywords that stand for
        // "this document's namespace" and "no namespace at all".
        let list = |this: &mut Self, v: &str| -> Vec<Option<Namespace>> {
            v.split_whitespace()
                .map(|tok| match tok {
                    "##targetNamespace" => ctx.target_ns,
                    "##local" => None,
                    uri => this.names.opt_namespace(uri),
                })
                .collect()
        };

        // XSD 1.1's `notNamespace` is the inverse of `namespace`, and the two
        // are alternatives. `##other` is 1.0's one-namespace special case of
        // the same idea.
        let namespace = match (node.attribute("namespace"), node.attribute("notNamespace")) {
            (Some(v), _) => match v.trim() {
                "##any" => NamespaceConstraint::Any,
                // `##other` bars *no namespace* as well as the target one.
                // 1.0 says it in a separate rule ("and must not be absent"),
                // 1.1 by putting absent in the set; either way a wildcard
                // meaning "somebody else's namespace" does not mean
                // "unqualified". `notNamespace` is different — it bars what it
                // lists, and `##local` is how it would name absent.
                "##other" => NamespaceConstraint::Not(match ctx.target_ns {
                    Some(t) => vec![Some(t), None],
                    None => vec![None],
                }),
                other => NamespaceConstraint::Enumeration(list(self, other)),
            },
            (None, Some(v)) => NamespaceConstraint::Not(list(self, v)),
            (None, None) => NamespaceConstraint::Any,
        };
        let process_contents = match node.attribute("processContents") {
            Some("skip") => ProcessContents::Skip,
            Some("strict") => ProcessContents::Strict,
            _ => ProcessContents::Lax,
        };
        // `notQName` mixes plain names with two keywords standing for sets
        // the document cannot spell out.
        let raw = node.attribute("notQName").unwrap_or_default();
        let mut not_qname = Vec::new();
        let mut not_defined = false;
        let mut not_defined_sibling = false;
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        for tok in raw.split_whitespace() {
            match tok {
                "##defined" => not_defined = true,
                "##definedSibling" => not_defined_sibling = true,
                _ if tok.starts_with("##") => {}
                _ => {
                    // `xml:xml:lang` resolves a prefix and leaves a local part
                    // with a colon in it, which is not a name.
                    let local = tok.rsplit(':').next().unwrap_or(tok);
                    if !crate::values::is_ncname(local) || tok.matches(':').count() > 1 {
                        self.diags.push(
                            Diagnostic::error(
                                DiagCode::InvalidAttributeValue,
                                format!("`notQName` contains `{tok}`, which is not a QName"),
                            )
                            .at(span.clone()),
                        );
                        continue;
                    }
                    if let Some(q) = self.attr_qname(node, tok, ctx, &span) {
                        // A name the wildcard could never have matched cannot
                        // be excluded from it — the schema is describing a
                        // set it already does not contain.
                        if !namespace.admits(q.ns) {
                            let shown = self.names.display(q);
                            self.diags.push(
                                Diagnostic::error(
                                    DiagCode::InvalidAttributeValue,
                                    format!(
                                        "`notQName` excludes `{shown}`, which this wildcard does not admit anyway"
                                    ),
                                )
                                .at(span.clone()),
                            );
                        }
                        not_qname.push(q);
                    }
                }
            }
        }
        Wildcard {
            namespace,
            process_contents,
            not_qname,
            not_defined,
            not_defined_sibling,
        }
    }

    // -- group definitions -------------------------------------------------

    fn read_group_def(&mut self, node: roxmltree::Node, ctx: &DocCtx) {
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        let name = self.qualified_name(node.attribute("name").unwrap_or_default(), true, ctx);
        let annotation = self.read_annotation(node, ctx);

        let mut particles = Vec::new();
        let mut compositor = Compositor::Sequence;
        if let Some(c) = node
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| matches!(c.tag_name().name(), "sequence" | "choice" | "all"))
        {
            compositor = match c.tag_name().name() {
                "sequence" => Compositor::Sequence,
                "choice" => Compositor::Choice,
                _ => Compositor::All,
            };
            for gc in c.children().filter(|n| reads(n, ctx.version)) {
                if let Some(p) = self.read_particle(gc, ctx, Scope::Global) {
                    particles.push(p);
                }
            }
        }

        let id = GroupId(self.model_groups.push(ModelGroupDef {
            name,
            group: ModelGroup {
                compositor,
                particles,
            },
            annotation,
            span: span.clone(),
        }));
        if self.globals.model_groups.insert(name, id).is_some() && !self.in_redefine {
            let shown = self.names.display(name);
            self.diags.push(
                Diagnostic::error(
                    DiagCode::DuplicateGlobal,
                    format!("duplicate global model group `{shown}`"),
                )
                .at(span),
            );
        }
    }

    fn read_attribute_group_def(&mut self, node: roxmltree::Node, ctx: &DocCtx) {
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        let name = self.qualified_name(node.attribute("name").unwrap_or_default(), true, ctx);
        let annotation = self.read_annotation(node, ctx);

        let id = AttrGroupId(self.attribute_groups.push(AttributeGroupDef {
            name,
            attribute_uses: Vec::new(),
            attribute_group_refs: Vec::new(),
            attribute_wildcard: None,
            annotation,
            span: span.clone(),
        }));

        let (uses, groups, wildcard) =
            self.read_attribute_uses(node, ctx, AttrOwner::AttributeGroup(id), Scope::Global);
        let g = self.attribute_groups.get_mut(id.0);
        g.attribute_uses = uses;
        g.attribute_group_refs = groups;
        g.attribute_wildcard = wildcard;

        if self.globals.attribute_groups.insert(name, id).is_some() && !self.in_redefine {
            let shown = self.names.display(name);
            self.diags.push(
                Diagnostic::error(
                    DiagCode::DuplicateGlobal,
                    format!("duplicate global attribute group `{shown}`"),
                )
                .at(span),
            );
        }
    }

    /// Parses one `xpath` attribute of an identity constraint.
    ///
    /// A path that does not parse is reported and treated as selecting
    /// nothing, which is the only safe reading: guessing at what a malformed
    /// selector meant would silently change which nodes a key covers.
    fn read_xpath(
        &mut self,
        text: &str,
        is_field: bool,
        node: roxmltree::Node,
        _ctx: &DocCtx,
        span: &Span,
    ) -> crate::identity::Paths {
        struct Names<'a, 'i> {
            interner: &'a mut crate::names::Interner,
            node: roxmltree::Node<'i, 'i>,
        }
        impl crate::identity::PathNames for Names<'_, '_> {
            fn namespace(&mut self, prefix: &str) -> Option<crate::names::Namespace> {
                let uri = if prefix == "xml" {
                    Some(crate::names::XML)
                } else {
                    self.node.lookup_namespace_uri(Some(prefix))
                }?;
                Some(crate::names::Namespace::from_symbol(
                    self.interner.intern(uri),
                ))
            }
            fn intern(&mut self, local: &str) -> crate::names::Symbol {
                self.interner.intern(local)
            }
        }

        let mut names = Names {
            interner: &mut self.names,
            node,
        };
        match crate::identity::parse(text, is_field, &mut names) {
            Ok(p) => p,
            Err(e) => {
                self.diags.push(
                    Diagnostic::error(
                        DiagCode::InvalidAttributeValue,
                        format!(
                            "`{text}` is not a valid identity-constraint path: {}",
                            e.reason
                        ),
                    )
                    .at(span.clone()),
                );
                crate::identity::Paths(Vec::new())
            }
        }
    }

    fn read_notation(&mut self, node: roxmltree::Node, ctx: &DocCtx) {
        let span = Span::new(&ctx.uri, line_of(ctx, node));
        let name = self.qualified_name(node.attribute("name").unwrap_or_default(), true, ctx);
        let annotation = self.read_annotation(node, ctx);
        let id = NotationId(self.notations.push(NotationDecl {
            name,
            public_id: node.attribute("public").map(str::to_string),
            system_id: node.attribute("system").map(str::to_string),
            annotation,
            span: span.clone(),
        }));
        if self.globals.notations.insert(name, id).is_some() && !self.in_redefine {
            let shown = self.names.display(name);
            self.diags.push(
                Diagnostic::error(
                    DiagCode::DuplicateGlobal,
                    format!("duplicate global notation `{shown}`"),
                )
                .at(span),
            );
        }
    }

    // -- annotations -------------------------------------------------------

    /// Reads the `xs:annotation` child, keeping `appinfo` content verbatim.
    ///
    /// The raw XML is the point: whatever convention a schema family wrote
    /// into `appinfo` cannot be recovered from a summary of it.
    fn read_annotation(&mut self, node: roxmltree::Node, ctx: &DocCtx) -> Option<AnnotationId> {
        let ann = node
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| c.tag_name().name() == "annotation")?;

        let mut out = Annotation::default();
        for c in ann.children().filter(|n| reads(n, ctx.version)) {
            match c.tag_name().name() {
                "documentation" => {
                    let text = c.text().unwrap_or_default().trim();
                    if !text.is_empty() {
                        out.documentation.push(text.to_string());
                    }
                }
                "appinfo" => out.appinfo.push(AppInfo {
                    source: c.attribute("source").map(str::to_string),
                    xml: serialize_children(c),
                }),
                _ => {}
            }
        }
        if out.is_empty() {
            return None;
        }
        Some(AnnotationId(self.annotations.push(out)))
    }

    // -- name helpers ------------------------------------------------------

    /// The name of a *local* declaration, honouring XSD 1.1's
    /// `targetNamespace`.
    ///
    /// A local element or attribute normally lands in this document's target
    /// namespace or in none, decided by `form`. XSD 1.1 lets it name a
    /// namespace outright — which is how a schema puts a declaration in a
    /// namespace it does not own, and the only way to restrict a wildcard that
    /// admits one. Nothing else may carry the attribute, so this is only
    /// reached from the two local-declaration sites.
    fn local_name(
        &mut self,
        node: roxmltree::Node,
        local: &str,
        qualified: bool,
        global: bool,
        ctx: &DocCtx,
    ) -> QName {
        match node.attribute("targetNamespace").filter(|_| !global) {
            Some(uri) => QName {
                ns: self.names.opt_namespace(uri),
                local: self.names.intern(local),
            },
            None => self.qualified_name(local, qualified, ctx),
        }
    }

    fn qualified_name(&mut self, local: &str, qualified: bool, ctx: &DocCtx) -> QName {
        QName {
            ns: if qualified { ctx.target_ns } else { None },
            local: self.names.intern(local),
        }
    }

    /// Resolves a QName written in an attribute value, e.g. `type="xs:int"`.
    ///
    /// An unprefixed value takes the in-scope *default* namespace, per the
    /// rules for QNames in content.
    fn attr_qname(
        &mut self,
        node: roxmltree::Node,
        value: &str,
        _ctx: &DocCtx,
        span: &Span,
    ) -> Option<QName> {
        let value = value.trim();
        let (prefix, local) = match value.split_once(':') {
            Some((p, l)) => (Some(p), l),
            None => (None, value),
        };
        // The `xml` prefix is bound to the XML namespace implicitly and
        // permanently; no schema declares it, and many use `xml:lang`.
        let uri = match prefix {
            Some("xml") => Some(XML),
            p => node.lookup_namespace_uri(p),
        };
        match (prefix, uri) {
            (Some(p), None) => {
                self.diags.push(
                    Diagnostic::error(
                        DiagCode::InvalidAttributeValue,
                        format!("undeclared namespace prefix `{p}` in `{value}`"),
                    )
                    .at(span.clone()),
                );
                None
            }
            _ => {
                let ns = uri.and_then(|u| self.names.opt_namespace(u));
                Some(QName {
                    ns,
                    local: self.names.intern(local),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Schema Representation Constraints: the rules answerable from the document
/// alone, with nothing resolved and no other document consulted.
///
/// One sweep of the tree rather than a check at each reading site, because
/// these are about the XML the schema for schemas describes, not about the
/// components it produces — and several of them concern elements this loader
/// otherwise never visits.
fn check_representation(root: roxmltree::Node, ctx: &DocCtx, diags: &mut Diagnostics) {
    check_annotation_placement(root, ctx, diags);

    // Every `id` in the schema for schemas is an `xs:ID`, so the two rules
    // that come with that datatype apply throughout: it is an NCName, and it
    // is unique in its document. Nothing else in this crate reads them, which
    // is exactly why nothing else would notice.
    let mut ids: Vec<&str> = Vec::new();

    for node in root.descendants().filter(|n| reads(n, ctx.version)) {
        let name = node.tag_name().name();
        let span = || Span::new(&ctx.uri, line_of(ctx, node));

        if let Some(id) = node.attribute("id") {
            if !crate::values::is_ncname(id) {
                diags.push(
                    Diagnostic::error(
                        DiagCode::InvalidAttributeValue,
                        format!("`id` is `{id}`, which is not an NCName"),
                    )
                    .at(span()),
                );
            } else if ids.contains(&id) {
                diags.push(
                    Diagnostic::error(
                        DiagCode::DuplicateGlobal,
                        format!("`id` `{id}` is used more than once in this document"),
                    )
                    .at(span())
                    .with_help("an `xs:ID` is unique within the document that carries it"),
                );
            } else {
                ids.push(id);
            }
        }

        // `block` and `final` name derivation methods. Which subset is legal
        // depends on where the attribute sits, but a token outside the whole
        // vocabulary is wrong everywhere — and silently ignoring it turns a
        // typo into a constraint that quietly does nothing.
        for attr in ["block", "final", "blockDefault", "finalDefault"] {
            let Some(value) = node.attribute(attr) else {
                continue;
            };
            let trimmed = value.trim();
            if trimmed == "#all" {
                continue;
            }
            for tok in trimmed.split_whitespace() {
                if !DerivationSet::KEYWORDS.contains(&tok) {
                    let help = if tok == "#all" {
                        "`#all` is the whole value or none of it"
                    } else {
                        "expected `#all`, or a list of `extension`, `restriction`, `substitution`, `list` and `union`"
                    };
                    diags.push(
                        Diagnostic::error(
                            DiagCode::InvalidAttributeValue,
                            format!("`{attr}` contains `{tok}`, which is not a derivation method"),
                        )
                        .at(span())
                        .with_help(help),
                    );
                }
            }
        }

        // `default` supplies a value the document may override; `fixed`
        // supplies one it may not. A declaration cannot mean both.
        if matches!(name, "element" | "attribute")
            && node.attribute("default").is_some()
            && node.attribute("fixed").is_some()
        {
            diags.push(
                Diagnostic::error(
                    DiagCode::InvalidValueConstraint,
                    format!("`xs:{name}` has both `default` and `fixed`"),
                )
                .at(span())
                .with_help("`default` may be overridden and `fixed` may not, so only one applies"),
            );
        }

        // A `ref` names a global declaration whole, so everything that would
        // describe a *new* one is prohibited beside it — otherwise the schema
        // says two different things about one name. Occurrence bounds are not
        // description: they belong to the reference, not the declaration.
        if matches!(name, "element" | "attribute") && node.has_attribute("ref") {
            if node.has_attribute("name") {
                diags.push(
                    Diagnostic::error(
                        DiagCode::InvalidAttributeValue,
                        format!("`xs:{name}` has both `name` and `ref`"),
                    )
                    .at(span())
                    .with_help("`ref` points at a global declaration; `name` makes a new one"),
                );
            }
            let banned: &[&str] = match name {
                "element" => &["nillable", "default", "fixed", "form", "block", "type"],
                _ => &["form", "type"],
            };
            for attr in banned {
                if node.has_attribute(*attr) {
                    diags.push(
                        Diagnostic::error(
                            DiagCode::InvalidAttributeValue,
                            format!("`xs:{name}` may not carry `{attr}` beside `ref`"),
                        )
                        .at(span())
                        .with_help("the referenced declaration already says this"),
                    );
                }
            }
            for c in node.children().filter(|n| reads(n, ctx.version)) {
                let child = c.tag_name().name();
                let describes = match name {
                    "element" => {
                        matches!(
                            child,
                            "simpleType" | "complexType" | "key" | "keyref" | "unique"
                        )
                    }
                    _ => child == "simpleType",
                };
                if describes {
                    diags.push(
                        Diagnostic::error(
                            DiagCode::InvalidAttributeValue,
                            format!("`xs:{name}` may not contain `xs:{child}` beside `ref`"),
                        )
                        .at(Span::new(&ctx.uri, line_of(ctx, c)))
                        .with_help("the referenced declaration already says this"),
                    );
                }
            }
        }

        // Open content has to agree with itself: a mode other than `none`
        // says what it opens *to*, and `none` says it opens to nothing. An
        // `xs:any` is required by the first and contradicted by the second.
        if matches!(name, "openContent" | "defaultOpenContent") {
            let mode = node.attribute("mode").unwrap_or("interleave").trim();
            let has_any = node
                .children()
                .filter(|n| reads(n, ctx.version))
                .any(|c| c.tag_name().name() == "any");
            if mode != "none" && !has_any {
                diags.push(
                    Diagnostic::error(
                        DiagCode::MissingElement,
                        format!("`xs:{name}` with `mode=\"{mode}\"` needs an `xs:any`"),
                    )
                    .at(span())
                    .with_help("only `mode=\"none\"` may leave the wildcard out"),
                );
            }
            if mode == "none" && has_any {
                diags.push(
                    Diagnostic::error(
                        DiagCode::InvalidAttributeValue,
                        format!("`xs:{name}` with `mode=\"none\"` may not carry an `xs:any`"),
                    )
                    .at(span())
                    .with_help(
                        "`none` says the content is not open, so there is nothing to open it with",
                    ),
                );
            }
        }

        // `mixed` may be written on the `xs:complexType` or on its
        // `xs:complexContent`. Both is fine; disagreeing is not.
        if name == "complexType" {
            let inner = node
                .children()
                .filter(|n| reads(n, ctx.version))
                .find(|c| c.tag_name().name() == "complexContent")
                .and_then(|c| c.attribute("mixed"))
                .and_then(boolean_value);
            let outer = node.attribute("mixed").and_then(boolean_value);
            if let (Some(a), Some(b)) = (outer, inner) {
                if a != b {
                    diags.push(
                        Diagnostic::error(
                            DiagCode::InvalidAttributeValue,
                            "`mixed` on `xs:complexType` contradicts the one on `xs:complexContent`"
                                .to_string(),
                        )
                        .at(span()),
                    );
                }
            }
        }

        // `namespace` says which namespaces a wildcard admits and
        // `notNamespace` says which it refuses. Both at once names no set.
        if matches!(name, "any" | "anyAttribute")
            && node.has_attribute("namespace")
            && node.has_attribute("notNamespace")
        {
            diags.push(
                Diagnostic::error(
                    DiagCode::InvalidAttributeValue,
                    format!("`xs:{name}` has both `namespace` and `notNamespace`"),
                )
                .at(span())
                .with_help("they are alternatives: one admits namespaces, the other refuses them"),
            );
        }

        // XSD 1.1 lets a *local* declaration name its namespace outright,
        // under three conditions. The last is the one with a reason worth
        // stating: the declaration only means something if it corresponds to
        // one in a base type, so it has to sit inside a restriction of
        // something other than `xs:anyType`, which declares nothing.
        if matches!(name, "element" | "attribute") && node.has_attribute("targetNamespace") {
            let top_level = node.parent().is_some_and(|p| p.has_tag_name("schema"));
            let mut why = None;
            if top_level {
                why = Some("a top-level declaration is already in the document's namespace");
            } else if node.has_attribute("form") {
                why = Some("`form` already decides the namespace");
            } else if node.attribute("targetNamespace") != root.attribute("targetNamespace")
                && !within_restriction_of_a_named_type(node)
            {
                // Naming the document's own namespace is always allowed. Naming
                // a *different* one only means something against a base
                // declaration to correspond to, so it needs a restriction of
                // something more specific than `xs:anyType`, which declares
                // nothing to correspond to.
                why = Some(
                    "naming another namespace needs an `xs:restriction` of a type other than `xs:anyType`",
                );
            }
            if let Some(reason) = why {
                diags.push(
                    Diagnostic::error(
                        DiagCode::InvalidAttributeValue,
                        format!("`xs:{name}` may not carry `targetNamespace` here"),
                    )
                    .at(span())
                    .with_help(reason),
                );
            }
        }

        // A named model group *is* its one model group. Two of them name no
        // single content model, and nothing downstream would ever look at the
        // second.
        if name == "group" && node.has_attribute("name") {
            let groups = node
                .children()
                .filter(|n| reads(n, ctx.version))
                .filter(|c| matches!(c.tag_name().name(), "all" | "choice" | "sequence"))
                .count();
            if groups != 1 {
                diags.push(
                    Diagnostic::error(
                        DiagCode::ConflictingTypeDefinition,
                        format!(
                            "`xs:group` must contain exactly one of `xs:all`, `xs:choice` or `xs:sequence`, found {groups}"
                        ),
                    )
                    .at(span()),
                );
            }
        }
    }
}

/// Whether a node sits inside an `xs:restriction` of something other than
/// `xs:anyType`.
///
/// Read from the document rather than the model: this is a representation
/// constraint, and the base is a QName the loader has not resolved yet. An
/// unprefixed `anyType` is not matched — that would need the in-scope default
/// namespace — so this errs towards accepting, which is right for a rule whose
/// only job is to catch a declaration that could not mean anything.
fn within_restriction_of_a_named_type(node: roxmltree::Node) -> bool {
    let mut cur = node.parent();
    while let Some(n) = cur {
        if n.has_tag_name("schema") {
            return false;
        }
        if is_xs_element(&n) && n.tag_name().name() == "restriction" {
            let base = n.attribute("base").unwrap_or_default();
            return !base.is_empty() && !base.ends_with(":anyType") && base != "anyType";
        }
        cur = n.parent();
    }
    false
}

/// Where `xs:annotation` is allowed to sit.
///
/// The schema for schemas gives almost every component the same content
/// model: an optional annotation, then the rest. So one annotation, and it
/// comes first — a second one, or one after the `xs:selector` it was meant to
/// describe, does not match the grammar and the document is not a schema.
///
/// Three elements are exempt because their own content models say so.
/// `xs:schema` interleaves annotations with the declarations they document,
/// and `xs:redefine` and `xs:override` do the same with what they revise.
///
/// This is a Schema Representation Constraint: answerable from the document
/// alone, which is why it runs here rather than on the assembled model.
fn check_annotation_placement(root: roxmltree::Node, ctx: &DocCtx, diags: &mut Diagnostics) {
    for node in root.descendants().filter(|n| reads(n, ctx.version)) {
        if matches!(
            node.tag_name().name(),
            "schema" | "redefine" | "override" | "annotation"
        ) {
            continue;
        }
        let mut seen = false;
        for (i, child) in node
            .children()
            .filter(|n| reads(n, ctx.version))
            .enumerate()
        {
            if child.tag_name().name() != "annotation" {
                continue;
            }
            let span = Span::new(&ctx.uri, line_of(ctx, child));
            let owner = node.tag_name().name();
            if seen {
                diags.push(
                    Diagnostic::error(
                        DiagCode::MisplacedAnnotation,
                        format!("`xs:{owner}` may have only one `xs:annotation`"),
                    )
                    .at(span),
                );
            } else if i != 0 {
                diags.push(
                    Diagnostic::error(
                        DiagCode::MisplacedAnnotation,
                        format!("`xs:annotation` must be the first child of `xs:{owner}`"),
                    )
                    .at(span),
                );
            }
            seen = true;
        }
    }
}

/// The XSD namespace for conditional inclusion, XSD 1.1 §4.2.2.
const VC: &str = "http://www.w3.org/2007/XMLSchema-versioning";

/// An XSD element this processor should read.
///
/// Conditional inclusion lets one document serve processors of different
/// versions: an element whose `vc:` conditions this processor does not meet is
/// *ignored*, subtree and all, as though it were not written. That is why the
/// test belongs on every descend point rather than at the places components
/// are created — a skipped element's children must never be looked at either,
/// and two alternatives for the same name must not both be registered.
fn reads(n: &roxmltree::Node, version: Version) -> bool {
    is_xs_element(n) && vc_included(n, version)
}

/// Whether this processor meets an element's `vc:` conditions.
fn vc_included(n: &roxmltree::Node, version: Version) -> bool {
    // The version this processor claims to be. Conditional inclusion compares
    // against it numerically, so a document can say "1.1 and later".
    let ours = match version {
        Version::Xsd10 => 1.0_f64,
        Version::Xsd11 => 1.1_f64,
    };
    let num = |name: &str| {
        n.attribute((VC, name))
            .and_then(|v| v.trim().parse::<f64>().ok())
    };
    if num("minVersion").is_some_and(|m| ours < m) {
        return false;
    }
    // `maxVersion` is exclusive: it names the first version that must ignore
    // the element, not the last that may read it.
    if num("maxVersion").is_some_and(|m| ours >= m) {
        return false;
    }

    // The availability tests name components a processor may or may not have.
    // Every built-in type and facet this crate knows is exactly what
    // `Builtin::from_local_name` and the facet reader accept, so the answers
    // are the same for both versions except where the type itself is 1.1.
    let all_available = |list: &str| {
        list.split_whitespace()
            .all(|q| component_available(q, version))
    };
    if let Some(list) = n.attribute((VC, "typeAvailable")) {
        if !all_available(list) {
            return false;
        }
    }
    if let Some(list) = n.attribute((VC, "typeUnavailable")) {
        if all_available(list) {
            return false;
        }
    }
    true
}

/// Whether a `vc:typeAvailable` entry names a type this build supports.
///
/// Only the XSD namespace is answerable: a user-defined type's availability is
/// a question about the schema being assembled, not about the processor, and
/// answering "no" would drop constructs that are perfectly readable. So
/// anything else counts as available.
fn component_available(qname: &str, version: Version) -> bool {
    let local = qname.rsplit(':').next().unwrap_or(qname);
    match crate::datatypes::Builtin::from_local_name(local) {
        Some(b) => {
            version == Version::Xsd11
                || !matches!(
                    b,
                    crate::datatypes::Builtin::YearMonthDuration
                        | crate::datatypes::Builtin::DayTimeDuration
                        | crate::datatypes::Builtin::DateTimeStamp
                        | crate::datatypes::Builtin::AnyAtomicType
                )
        }
        None => true,
    }
}

fn is_xs_element(n: &roxmltree::Node) -> bool {
    n.is_element() && n.tag_name().namespace() == Some(XS)
}

/// Where every line of a document starts, so a byte offset becomes a line
/// number without rescanning.
///
/// `roxmltree`'s own `text_pos_at` counts newlines from the beginning of the
/// document on every call. That is fine once and quadratic when a span is
/// built for each declaration, which is what this loader does — a schema with
/// three thousand types spent almost all of its time here. Built once per
/// document, it turns each lookup into a binary search.
#[derive(Debug, Default)]
pub(crate) struct LineIndex {
    /// Byte offset of the first character of each line, ascending.
    starts: Vec<usize>,
}

impl LineIndex {
    fn build(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        Self { starts }
    }

    /// The one-based line holding `offset`.
    fn line(&self, offset: usize) -> u32 {
        // `partition_point` gives the number of starts at or before the
        // offset, which is the line number already.
        self.starts.partition_point(|&s| s <= offset).max(1) as u32
    }
}

fn line_of(ctx: &DocCtx, node: roxmltree::Node) -> u32 {
    ctx.lines.line(node.range().start)
}

/// Children that may sit beside facets without being one.
///
/// `xs:simpleType` under a `simpleType` restriction, and the attribute
/// machinery and `xs:assert` under a `simpleContent` one — which shares this
/// reader, and whose children are otherwise identical.
fn not_a_facet(name: &str) -> bool {
    matches!(
        name,
        "annotation" | "simpleType" | "attribute" | "attributeGroup" | "anyAttribute" | "assert"
    )
}

/// An `xs:boolean` attribute, read the way the datatype says.
///
/// Its lexical space is `true`, `false`, `1` and `0`, and the value is
/// whitespace-collapsed before it is looked at — so `mixed="1"` means exactly
/// what `mixed="true"` means. Comparing against `"true"` alone silently drops
/// the other spelling, which the W3C suite uses.
fn flag(node: roxmltree::Node, name: &str) -> bool {
    node.attribute(name)
        .and_then(boolean_value)
        .unwrap_or(false)
}

/// An `xs:boolean` lexical form, or `None` when the text is not one.
fn boolean_value(s: &str) -> Option<bool> {
    match s.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// The namespace bindings a facet literal would need, were its type
/// `xs:QName` or `xs:NOTATION`.
///
/// Captures only the prefix each token actually uses — or the default
/// declaration, for an unprefixed one. Copying every `xmlns` in scope would
/// put a schema's whole prologue on every facet set, and the NIST QName tests
/// declare thirty of them.
///
/// The literal is split on whitespace because a list type's enumeration
/// literal is a whole list, whose items may carry different prefixes. A token
/// that only looks like a QName costs a failed lookup and nothing else.
fn qname_bindings(node: roxmltree::Node, literal: &str, out: &mut Vec<(Option<String>, String)>) {
    for token in literal.split_whitespace() {
        let prefix = token.split_once(':').map(|(p, _)| p);
        if out.iter().any(|(p, _)| p.as_deref() == prefix) {
            continue;
        }
        if let Some(uri) = node.lookup_namespace_uri(prefix) {
            out.push((prefix.map(str::to_owned), uri.to_string()));
        }
    }
}

fn value_constraint(node: roxmltree::Node) -> Option<ValueConstraint> {
    if let Some(v) = node.attribute("fixed") {
        Some(ValueConstraint::Fixed(v.to_string()))
    } else {
        node.attribute("default")
            .map(|v| ValueConstraint::Default(v.to_string()))
    }
}

/// Re-serializes an element's children, for keeping `appinfo` verbatim.
fn serialize_children(node: roxmltree::Node) -> String {
    let mut out = String::new();
    for c in node.children() {
        serialize_node(c, &mut out);
    }
    out.trim().to_string()
}

fn serialize_node(node: roxmltree::Node, out: &mut String) {
    if node.is_text() {
        out.push_str(node.text().unwrap_or_default());
        return;
    }
    if !node.is_element() {
        return;
    }
    let name = qualified_tag(node);
    out.push('<');
    out.push_str(&name);
    for a in node.attributes() {
        out.push(' ');
        if let Some(ns) = a.namespace() {
            // Keep the URI rather than a prefix that may not survive.
            out.push_str(&format!("{{{ns}}}"));
        }
        out.push_str(a.name());
        out.push_str("=\"");
        out.push_str(&escape_attr(a.value()));
        out.push('"');
    }
    if !node.has_children() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for c in node.children() {
        serialize_node(c, out);
    }
    out.push_str("</");
    out.push_str(&name);
    out.push('>');
}

fn qualified_tag(node: roxmltree::Node) -> String {
    match node.tag_name().namespace() {
        Some(ns) => format!("{{{ns}}}{}", node.tag_name().name()),
        None => node.tag_name().name().to_string(),
    }
}

fn escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::LineIndex;

    /// `LineIndex` replaced `roxmltree::Document::text_pos_at` for speed, so it
    /// has to agree with it everywhere — including the awkward offsets: the
    /// very first byte, a byte just past a newline, a blank line, and the end
    /// of the document.
    #[test]
    fn the_line_index_agrees_with_roxmltree() {
        let src = "<a>\n  <b/>\n\n\t<c\n     x='1'/>\r\n<!--k-->\n</a>";
        let doc = roxmltree::Document::parse(src).expect("valid XML");
        let index = LineIndex::build(src);

        for node in doc.descendants() {
            let offset = node.range().start;
            assert_eq!(
                index.line(offset),
                doc.text_pos_at(offset).row,
                "line disagreed at byte {offset} ({node:?})"
            );
        }
        // Every byte, not just the ones a node happens to start at.
        for offset in 0..=src.len() {
            assert_eq!(
                index.line(offset),
                doc.text_pos_at(offset).row,
                "line disagreed at byte {offset}"
            );
        }
    }

    /// A document with no newline at all is one line, and an empty one is
    /// still line 1 — never 0, which would render as a span with no position.
    #[test]
    fn the_line_index_is_one_based_at_the_edges() {
        assert_eq!(LineIndex::build("").line(0), 1);
        assert_eq!(LineIndex::build("<a/>").line(3), 1);
        assert_eq!(LineIndex::build("\n").line(0), 1);
        assert_eq!(LineIndex::build("\n").line(1), 2);
    }
}

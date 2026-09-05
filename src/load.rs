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
                .at(Span::new(uri, line_of(&doc, root))),
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
        };
        let ctx = self.read_document_defaults(&doc, root, ctx);

        self.documents.push(SourceDocument {
            uri: uri.to_string(),
            target_namespace: target_ns,
            chameleon,
            version: root.attribute("version").map(str::to_string),
        });

        self.depth += 1;
        self.read_schema_body(&doc, root, &ctx);
        self.depth -= 1;
    }

    // -- schema body -------------------------------------------------------

    fn read_schema_body(&mut self, doc: &roxmltree::Document, root: roxmltree::Node, ctx: &DocCtx) {
        check_representation(doc, root, ctx, &mut self.diags);

        // Composition first: everything a later reference might name has to
        // exist before the references are collected.
        for child in root.children().filter(|n| reads(n, ctx.version)) {
            match child.tag_name().name() {
                "include" => self.read_include(doc, child, ctx),
                "import" => self.read_import(doc, child, ctx),
                "redefine" => self.read_redefine(doc, child, ctx),
                "override" => self.read_override(doc, child, ctx),
                _ => {}
            }
        }

        for child in root.children().filter(|n| reads(n, ctx.version)) {
            let span = Span::new(&ctx.uri, line_of(doc, child));
            match child.tag_name().name() {
                "include" | "import" | "redefine" | "override" | "annotation" => {}
                "element" => {
                    let id = self.read_element_decl(doc, child, ctx, Scope::Global, true);
                    self.register_global_element(id, span);
                }
                "attribute" => {
                    let id = self.read_attribute_decl(doc, child, ctx, Scope::Global, true);
                    self.register_global_attribute(id, span);
                }
                "simpleType" => {
                    let id = self.read_simple_type(doc, child, ctx, true);
                    self.register_global_type(id, span);
                }
                "complexType" => {
                    let id = self.read_complex_type(doc, child, ctx, true);
                    self.register_global_type(id, span);
                }
                "group" => self.read_group_def(doc, child, ctx),
                "attributeGroup" => self.read_attribute_group_def(doc, child, ctx),
                "notation" => self.read_notation(doc, child, ctx),
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

    fn read_include(&mut self, doc: &roxmltree::Document, node: roxmltree::Node, ctx: &DocCtx) {
        let Some(loc) = node.attribute("schemaLocation") else {
            self.diags.push(
                Diagnostic::error(
                    DiagCode::MissingAttribute,
                    "`xs:include` needs a schemaLocation",
                )
                .at(Span::new(&ctx.uri, line_of(doc, node))),
            );
            return;
        };
        match self.resolver.resolve(loc, Some(&ctx.uri)) {
            // The includer's namespace is passed down so a document with no
            // targetNamespace of its own is absorbed into it.
            Ok((uri, bytes)) => self.load_bytes(&bytes, &uri, ctx.target_ns),
            Err(e) => self.push_resolution_failure(e, doc, node, ctx),
        }
    }

    /// Reads the document-level XSD 1.1 defaults: `xs:defaultAttributes` and
    /// `xs:defaultOpenContent`.
    ///
    /// Both are declared once on `xs:schema` and apply to every complex type
    /// in that document, which is why they live on the per-document context
    /// rather than being looked up per type.
    fn read_document_defaults(
        &mut self,
        doc: &roxmltree::Document,
        root: roxmltree::Node,
        mut ctx: DocCtx,
    ) -> DocCtx {
        if let Some(v) = root.attribute("defaultAttributes") {
            let span = Span::new(&ctx.uri, line_of(doc, root));
            self.require_xsd11("defaultAttributes", &span);
            ctx.default_attributes = self.attr_qname(root, v, &ctx, &span);
        }
        if let Some(n) = root
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| c.tag_name().name() == "defaultOpenContent")
        {
            let span = Span::new(&ctx.uri, line_of(doc, n));
            self.require_xsd11("defaultOpenContent", &span);
            ctx.default_open_applies_to_empty = n.attribute("appliesToEmpty") == Some("true");
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
    fn read_redefine(&mut self, doc: &roxmltree::Document, node: roxmltree::Node, ctx: &DocCtx) {
        if !self.include_target(doc, node, ctx) {
            return;
        }
        let originals = self.capture_originals(node, ctx);
        self.read_modifications(doc, node, ctx, Some(&originals));
    }

    /// `xs:override` — include a document, then replace some of its
    /// components outright.
    ///
    /// Unlike `redefine`, an override's own references mean the **new**
    /// components, so nothing needs capturing. XSD 1.1 also applies overrides
    /// transitively through the included document's own includes; that part is
    /// not implemented, and is reported rather than assumed.
    fn read_override(&mut self, doc: &roxmltree::Document, node: roxmltree::Node, ctx: &DocCtx) {
        if !self.include_target(doc, node, ctx) {
            return;
        }
        self.read_modifications(doc, node, ctx, None);
    }

    /// Loads the document a `redefine`/`override` names, as an include would.
    fn include_target(
        &mut self,
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
    ) -> bool {
        let Some(loc) = node.attribute("schemaLocation") else {
            let what = node.tag_name().name();
            self.diags.push(
                Diagnostic::error(
                    DiagCode::MissingAttribute,
                    format!("`xs:{what}` needs a schemaLocation"),
                )
                .at(Span::new(&ctx.uri, line_of(doc, node))),
            );
            return false;
        };
        match self.resolver.resolve(loc, Some(&ctx.uri)) {
            Ok((uri, bytes)) => {
                self.load_bytes(&bytes, &uri, ctx.target_ns);
                true
            }
            Err(e) => {
                self.push_resolution_failure(e, doc, node, ctx);
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
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
        originals: Option<&Originals>,
    ) {
        // Replacing a name the included document declared is the point here,
        // not a duplicate-global error.
        let outer = std::mem::replace(&mut self.in_redefine, true);
        for c in node.children().filter(|n| reads(n, ctx.version)) {
            let span = Span::new(&ctx.uri, line_of(doc, c));
            let before = self.fixups.len();
            let kind = c.tag_name().name();

            match kind {
                "simpleType" | "complexType" => {
                    let id = if kind == "simpleType" {
                        self.read_simple_type(doc, c, ctx, true)
                    } else {
                        self.read_complex_type(doc, c, ctx, true)
                    };
                    if let Some(o) = originals {
                        self.pin_self_references(before, o);
                    }
                    if let Some(name) = self.types.get(id.0).name() {
                        self.replace_global_type(name, id, span);
                    }
                }
                "group" => {
                    self.read_group_def(doc, c, ctx);
                    if let Some(o) = originals {
                        self.pin_self_references(before, o);
                    }
                }
                "attributeGroup" => {
                    self.read_attribute_group_def(doc, c, ctx);
                    if let Some(o) = originals {
                        self.pin_self_references(before, o);
                    }
                }
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

    fn read_import(&mut self, doc: &roxmltree::Document, node: roxmltree::Node, ctx: &DocCtx) {
        let Some(loc) = node.attribute("schemaLocation") else {
            // Legal: schemaLocation is a hint, and a namespace may already be
            // present from another document or be supplied by the caller.
            return;
        };
        match self.resolver.resolve(loc, Some(&ctx.uri)) {
            Ok((uri, bytes)) => self.load_bytes(&bytes, &uri, None),
            Err(e) => self.push_resolution_failure(e, doc, node, ctx),
        }
    }

    fn push_resolution_failure(
        &mut self,
        e: String,
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
    ) {
        let span = Span::new(&ctx.uri, line_of(doc, node));
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
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
        scope: Scope,
        global: bool,
    ) -> ElementId {
        let span = Span::new(&ctx.uri, line_of(doc, node));
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
                nillable: node.attribute("nillable") == Some("true"),
                is_abstract: node.attribute("abstract") == Some("true"),
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
                    self.read_simple_type(doc, c, ctx, false)
                } else {
                    self.read_complex_type(doc, c, ctx, false)
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

        let idcs = self.read_identity_constraints(doc, node, ctx, id);
        self.elements.get_mut(id.0).identity_constraints = idcs;
        id
    }

    fn read_identity_constraints(
        &mut self,
        doc: &roxmltree::Document,
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
            let span = Span::new(&ctx.uri, line_of(doc, c));

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
            let fields = c
                .children()
                .filter(|n| reads(n, ctx.version))
                .filter(|n| n.tag_name().name() == "field")
                .filter_map(|n| n.attribute("xpath"))
                .map(str::to_string)
                .collect();
            let annotation = self.read_annotation(c, ctx);
            let id = IdcId(self.identity_constraints.push(IdentityConstraint {
                name,
                kind,
                selector,
                fields,
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
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
        scope: Scope,
        global: bool,
    ) -> AttributeId {
        let span = Span::new(&ctx.uri, line_of(doc, node));
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
                let tid = self.read_simple_type(doc, c, ctx, false);
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
        doc: &roxmltree::Document,
        parent: roxmltree::Node,
        ctx: &DocCtx,
        owner: AttrOwner,
        scope: Scope,
    ) -> (Vec<AttributeUse>, Vec<AttrGroupId>, Option<Wildcard>) {
        let mut uses = Vec::new();
        let mut groups = Vec::new();
        let mut wildcard = None;

        for c in parent.children().filter(|n| reads(n, ctx.version)) {
            let span = Span::new(&ctx.uri, line_of(doc, c));
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
                        None => self.read_attribute_decl(doc, c, ctx, scope, false),
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

    fn read_simple_type(
        &mut self,
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
        global: bool,
    ) -> TypeId {
        let span = Span::new(&ctx.uri, line_of(doc, node));
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
                            let b = self.read_simple_type(doc, inner, ctx, false);
                            self.simple_mut(id).base = b;
                        }
                    }
                }
                let facets = self.read_facets(doc, d, ctx);
                self.simple_mut(id).facets = FacetSet::new().restrict(&facets);
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
                            let it = self.read_simple_type(doc, inner, ctx, false);
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
                    members.push(self.read_simple_type(doc, inner, ctx, false));
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

    fn read_facets(
        &mut self,
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
    ) -> Vec<Facet> {
        let mut out = Vec::new();
        for c in node.children().filter(|n| reads(n, ctx.version)) {
            let v = c.attribute("value").unwrap_or_default();
            let span = || Span::new(&ctx.uri, line_of(doc, c));
            let facet = match c.tag_name().name() {
                "length" => v.parse().ok().map(Facet::Length),
                "minLength" => v.parse().ok().map(Facet::MinLength),
                "maxLength" => v.parse().ok().map(Facet::MaxLength),
                "pattern" => Some(Facet::Pattern(v.to_string())),
                "enumeration" => Some(Facet::Enumeration(v.to_string())),
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
                "annotation" | "simpleType" => None,
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
                None if matches!(c.tag_name().name(), "annotation" | "simpleType") => {}
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
        self.check_step_facets(&out, &Span::new(&ctx.uri, line_of(doc, node)));
        out
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

    fn read_complex_type(
        &mut self,
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
        global: bool,
    ) -> TypeId {
        let span = Span::new(&ctx.uri, line_of(doc, node));
        let name = global
            .then(|| self.qualified_name(node.attribute("name").unwrap_or_default(), true, ctx));
        let annotation = self.read_annotation(node, ctx);
        let mixed_attr = node.attribute("mixed") == Some("true");

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
                is_abstract: node.attribute("abstract") == Some("true"),
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
        let mixed = mixed_attr
            || content_wrapper
                .map(|w| w.attribute("mixed") == Some("true"))
                .unwrap_or(false);

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

        let particle = self.read_content_particle(doc, member_node, ctx, scope);
        let content = if simple_content {
            // The effective simple type is the base's; resolution fills the
            // base in, and `Schemas::simple_content_type` follows it.
            let inline = derivation_node.and_then(|d| {
                d.children()
                    .filter(|n| reads(n, ctx.version))
                    .find(|c| c.tag_name().name() == "simpleType")
            });
            match inline {
                Some(c) => ContentType::Simple(self.read_simple_type(doc, c, ctx, false)),
                None => ContentType::Simple(TypeId::PLACEHOLDER),
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
            self.read_attribute_uses(doc, member_node, ctx, AttrOwner::ComplexType(id), scope);

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
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
        scope: Scope,
    ) -> Option<ParticleId> {
        let c = node
            .children()
            .filter(|n| reads(n, ctx.version))
            .find(|c| matches!(c.tag_name().name(), "sequence" | "choice" | "all" | "group"))?;
        self.read_particle(doc, c, ctx, scope)
    }

    fn read_particle(
        &mut self,
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
        scope: Scope,
    ) -> Option<ParticleId> {
        let span = Span::new(&ctx.uri, line_of(doc, node));
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
                None => Term::Element(self.read_element_decl(doc, node, ctx, scope, false)),
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
                        if let Some(p) = self.read_particle(doc, c, ctx, scope) {
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
                "##other" => NamespaceConstraint::Not(vec![ctx.target_ns]),
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
        for tok in raw.split_whitespace() {
            match tok {
                "##defined" => not_defined = true,
                "##definedSibling" => not_defined_sibling = true,
                _ if tok.starts_with("##") => {}
                _ => {
                    if let Some(q) = self.attr_qname(node, tok, ctx, &Span::new(&ctx.uri, 0)) {
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

    fn read_group_def(&mut self, doc: &roxmltree::Document, node: roxmltree::Node, ctx: &DocCtx) {
        let span = Span::new(&ctx.uri, line_of(doc, node));
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
                if let Some(p) = self.read_particle(doc, gc, ctx, Scope::Global) {
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

    fn read_attribute_group_def(
        &mut self,
        doc: &roxmltree::Document,
        node: roxmltree::Node,
        ctx: &DocCtx,
    ) {
        let span = Span::new(&ctx.uri, line_of(doc, node));
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
            self.read_attribute_uses(doc, node, ctx, AttrOwner::AttributeGroup(id), Scope::Global);
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

    fn read_notation(&mut self, doc: &roxmltree::Document, node: roxmltree::Node, ctx: &DocCtx) {
        let span = Span::new(&ctx.uri, line_of(doc, node));
        let name = self.qualified_name(node.attribute("name").unwrap_or_default(), true, ctx);
        let annotation = self.read_annotation(node, ctx);
        let id = NotationId(self.notations.push(NotationDecl {
            name,
            public_id: node.attribute("public").map(str::to_string),
            system_id: node.attribute("system").map(str::to_string),
            annotation,
            span: span.clone(),
        }));
        if self.globals.notations.insert(name, id).is_some() {
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
    /// The raw XML is what the units layer will need: a unit written into
    /// `appinfo` cannot be recovered from a summary.
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
fn check_representation(
    doc: &roxmltree::Document,
    root: roxmltree::Node,
    ctx: &DocCtx,
    diags: &mut Diagnostics,
) {
    check_annotation_placement(doc, root, ctx, diags);

    for node in root.descendants().filter(|n| reads(n, ctx.version)) {
        let name = node.tag_name().name();
        let span = || Span::new(&ctx.uri, line_of(doc, node));

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
fn check_annotation_placement(
    doc: &roxmltree::Document,
    root: roxmltree::Node,
    ctx: &DocCtx,
    diags: &mut Diagnostics,
) {
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
            let span = Span::new(&ctx.uri, line_of(doc, child));
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

fn line_of(doc: &roxmltree::Document, node: roxmltree::Node) -> u32 {
    doc.text_pos_at(node.range().start).row
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

//! Validating instance documents, in one streaming pass.
//!
//! `quick-xml` events in, diagnostics and a PSVI out. No DOM: a document may
//! be larger than memory, and the point of the content automata built in
//! [`crate::content`] is that validation needs only the current element
//! stack.
//!
//! # What the schema decides, and what the instance decides
//!
//! An element's declaration comes from the *enclosing content model*, so the
//! same name can be a different declaration in different places. On top of
//! that the instance gets two words of its own:
//!
//! - `xsi:type` replaces the declared type with any type derived from it.
//! - `xsi:nil="true"` says the element is present but valueless, which is not
//!   the same as being absent.
//!
//! # Not yet
//!
//! Identity constraints (`xs:key`/`keyref`) need document-scope state, and
//! XSD 1.1 assertions need a whole subtree buffered. Both are deliberately
//! absent rather than half-done — see `DESIGN.md` §3.6.

use crate::content::ContentMatcher;
use crate::diagnostics::{DiagCode, Diagnostic, Diagnostics, Span};
use crate::model::*;
use crate::names::{QName, XSI};
use crate::validate::{Validator, nearest_builtin};
use crate::values::{Namespaces, Value};
use fxhash::FxHashMap;
use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

/// One element or attribute after validation: what the schema says it is.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum PsviEvent {
    StartElement {
        name: QName,
        /// The declaration matched, absent under a `skip` wildcard or a `lax`
        /// one with nothing to match.
        declaration: Option<ElementId>,
        /// The type actually in force, after any `xsi:type` override.
        type_id: TypeId,
        /// Whether `xsi:type` chose that type rather than the declaration.
        type_from_instance: bool,
        nil: bool,
        attributes: Vec<AttributePsvi>,
        line: u32,
    },
    /// Character content, typed when the element's type has a value space.
    Text {
        value: Option<Value>,
        type_id: TypeId,
        lexical: String,
        /// True when the element was empty and its declaration's `default`
        /// or `fixed` value supplied the content, as for an absent attribute.
        from_schema: bool,
        line: u32,
    },
    EndElement {
        name: QName,
        declaration: Option<ElementId>,
        line: u32,
    },
}

#[derive(Clone, Debug)]
pub struct AttributePsvi {
    pub name: QName,
    pub declaration: Option<AttributeId>,
    pub value: Option<Value>,
    pub lexical: String,
    /// True when the document did not spell this attribute out and the
    /// schema supplied it from a `default` or `fixed` value.
    pub from_schema: bool,
}

/// The outcome of validating a document.
#[derive(Clone, Debug)]
pub struct ValidationReport {
    pub diagnostics: Diagnostics,
}

impl ValidationReport {
    pub fn is_valid(&self) -> bool {
        !self.diagnostics.has_errors()
    }
}

/// One level of the element stack.
struct Frame<'a> {
    /// Which element this is, for attributing an `xs:ID` it carries.
    id_scope: u32,
    name: QName,
    declaration: Option<ElementId>,
    type_id: TypeId,
    matcher: Option<ContentMatcher<'a>>,
    /// Character data accumulated for this element.
    text: String,
    nil: bool,
    /// Inside a `processContents="skip"` wildcard nothing is checked until
    /// the subtree closes.
    skipped: bool,
    line: u32,
}

/// An attribute as read from the document, with its prefix already resolved.
struct RawAttr {
    namespace: Option<String>,
    local: String,
    value: String,
}

/// Validates instance documents against a compiled schema.
///
/// Holds a [`Validator`], so simple types are prepared once and reused across
/// documents.
#[derive(Debug)]
pub struct InstanceValidator<'a> {
    schemas: &'a Schemas,
    values: Validator<'a>,
}

impl<'a> InstanceValidator<'a> {
    pub fn new(schemas: &'a Schemas) -> Self {
        Self {
            schemas,
            values: Validator::new(schemas),
        }
    }

    /// Validates a document, discarding the PSVI.
    pub fn validate(&self, xml: &str) -> ValidationReport {
        self.validate_with(xml, |_| {})
    }

    /// Validates a document, handing every PSVI event to `sink`.
    ///
    /// A callback rather than an iterator: the events are produced inside the
    /// streaming loop, and an iterator would have to buffer them.
    pub fn validate_with(&self, xml: &str, sink: impl FnMut(PsviEvent)) -> ValidationReport {
        let mut run = Run {
            v: self,
            diags: Diagnostics::new(),
            stack: Vec::new(),
            namespaces: Vec::new(),
            ids: FxHashMap::default(),
            idrefs: Vec::new(),
            elements_seen: 0,
            id_roles: FxHashMap::default(),
            uri: "<instance>".to_string(),
            sink,
        };
        run.drive(xml);
        ValidationReport {
            diagnostics: run.diags,
        }
    }

    /// Names the document for diagnostics, e.g. a file path.
    pub fn validate_named(
        &self,
        xml: &str,
        uri: &str,
        sink: impl FnMut(PsviEvent),
    ) -> ValidationReport {
        let mut run = Run {
            v: self,
            diags: Diagnostics::new(),
            stack: Vec::new(),
            namespaces: Vec::new(),
            ids: FxHashMap::default(),
            idrefs: Vec::new(),
            elements_seen: 0,
            id_roles: FxHashMap::default(),
            uri: uri.to_string(),
            sink,
        };
        run.drive(xml);
        ValidationReport {
            diagnostics: run.diags,
        }
    }
}

struct Run<'a, 'b, S: FnMut(PsviEvent)> {
    v: &'b InstanceValidator<'a>,
    diags: Diagnostics,
    stack: Vec<Frame<'a>>,
    /// Namespace declarations from the elements currently open, outermost
    /// first, one entry per open element.
    ///
    /// `xs:QName` and `xs:NOTATION` are the only datatypes whose value depends
    /// on the document rather than the schema, and this is what they resolve
    /// against. Pushed and popped in step with `stack`.
    namespaces: Vec<Vec<(Option<String>, String)>>,
    /// Every `xs:ID` value seen so far, and every `xs:IDREF` still waiting to
    /// match one.
    ///
    /// The only state here that outlives the element stack. It has to: both
    /// rules are document-scope by definition, and a reference may point
    /// forward, so the references cannot be settled until the end.
    ids: FxHashMap<String, u32>,
    idrefs: Vec<(String, u32)>,
    /// A counter distinguishing elements, so an `xs:ID` can be attributed to
    /// the one that claimed it.
    elements_seen: u32,
    /// Whether a type plays an identifier role, keyed by type — answering it
    /// walks a base chain, and most attributes are not identifiers.
    id_roles: FxHashMap<TypeId, Option<IdKind>>,
    uri: String,
    sink: S,
}

/// The two document-scope roles a value can play.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum IdRole {
    /// An `xs:ID`: no other element may claim it.
    Defines,
    /// An `xs:IDREF`: must match one.
    References,
}

/// What a *type* can tell us, before seeing a value.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum IdKind {
    Defines,
    References,
    /// A union: the member that matched decides, so ask the value.
    PerValue,
}

/// A borrowed view of the open elements' namespace declarations.
struct Scopes<'s>(&'s [Vec<(Option<String>, String)>]);

impl Namespaces for Scopes<'_> {
    fn resolve(&self, prefix: Option<&str>) -> Option<&str> {
        // The `xml` prefix is bound everywhere and cannot be redeclared to
        // anything else, so it never reaches the stack.
        if prefix == Some("xml") {
            return Some(crate::names::XML);
        }
        self.0
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .find(|(p, _)| p.as_deref() == prefix)
            // `xmlns=""` undeclares the default namespace rather than binding
            // it to the empty string.
            .and_then(|(_, uri)| (!uri.is_empty()).then_some(uri.as_str()))
    }
}

/// The namespace declarations an element carries, as bindings.
fn declared_namespaces(attrs: &[RawAttr]) -> Vec<(Option<String>, String)> {
    attrs
        .iter()
        .filter_map(|a| match &a.namespace {
            Some(ns) if ns == crate::names::XMLNS => Some((Some(a.local.clone()), a.value.clone())),
            None if a.local == "xmlns" => Some((None, a.value.clone())),
            _ => None,
        })
        .collect()
}

impl<'a, S: FnMut(PsviEvent)> Run<'a, '_, S> {
    fn error(&mut self, code: DiagCode, line: u32, msg: impl Into<String>) {
        let span = Span::new(&self.uri, line);
        self.diags.push(Diagnostic::error(code, msg).at(span));
    }

    /// Records the identifiers a value carries, if its type gives it that
    /// role.
    ///
    /// `xs:ID` binds a value to the element carrying it, and two *different*
    /// elements may not claim one — repeating it on a single element is a
    /// binding with one member, which XSD 1.1 says is fine, so `scope` is
    /// which element this is.
    ///
    /// Splitting on whitespace serves both the atomic types and the list ones
    /// (`xs:IDREFS`, or a user list of either): an `NCName` cannot contain
    /// whitespace, so an atomic value is exactly one token, and the split is
    /// over the *collapsed* value, which is why ` aaa ` and `aaa` are one
    /// identifier rather than two.
    fn record_identifiers(&mut self, ty: TypeId, lexical: &str, scope: u32, line: u32) {
        let kind = match self.id_roles.get(&ty) {
            Some(k) => *k,
            None => {
                let k = id_kind(self.v.schemas, ty);
                self.id_roles.insert(ty, k);
                k
            }
        };
        let role = match kind {
            None => return,
            Some(IdKind::Defines) => IdRole::Defines,
            Some(IdKind::References) => IdRole::References,
            // A union's members are tried in order and the first that
            // validates wins, so which one it was decides the role — and that
            // is a question about the value, not about the type.
            Some(IdKind::PerValue) => match self.union_member_role(ty, lexical) {
                Some(r) => r,
                None => return,
            },
        };
        for token in lexical.split_whitespace() {
            match role {
                IdRole::Defines => {
                    let claimed = self.ids.entry(token.to_string()).or_insert(scope);
                    if *claimed != scope {
                        self.error(
                            DiagCode::DuplicateId,
                            line,
                            format!("`{token}` is already the `xs:ID` of another element"),
                        );
                    }
                }
                // Not resolvable yet: an `xs:IDREF` may point forward.
                IdRole::References => self.idrefs.push((token.to_string(), line)),
            }
        }
    }

    /// The role a union value plays, found the way the validator found its
    /// member: in declaration order, first one that validates.
    fn union_member_role(&self, ty: TypeId, lexical: &str) -> Option<IdRole> {
        let members = self.v.schemas[ty].as_simple()?.member_types.clone();
        for m in members {
            if self
                .v
                .values
                .validate_in(m, lexical, &Scopes(&self.namespaces))
                .is_ok()
            {
                return match id_kind(self.v.schemas, m) {
                    Some(IdKind::Defines) => Some(IdRole::Defines),
                    Some(IdKind::References) => Some(IdRole::References),
                    _ => None,
                };
            }
        }
        None
    }

    /// How to refer to a type in a message. An anonymous type has no name
    /// to give, so it is described instead of quoted.
    fn show_type(&self, id: TypeId) -> String {
        match self.v.schemas[id].name() {
            Some(q) => format!("`{}`", self.show(q)),
            None => "an anonymous type".to_string(),
        }
    }

    fn show(&self, q: QName) -> String {
        self.v.schemas.display_name(q)
    }

    // -- the event loop ----------------------------------------------------

    fn drive(&mut self, xml: &str) {
        let mut reader = NsReader::from_str(xml);
        reader.config_mut().trim_text(false);
        reader.config_mut().check_end_names = true;
        // No DTD support at all: an instance document is untrusted input, so
        // there is no entity expansion to bound in the first place.

        let mut line = 1u32;
        loop {
            let event = match reader.read_resolved_event() {
                Ok((ns, event)) => {
                    // The resolved namespace borrows the reader, so anything
                    // needed from it is turned into interned, `Copy` data
                    // before the borrow ends.
                    let name = match &event {
                        Event::Start(e) | Event::Empty(e) => {
                            let uri = match &ns {
                                ResolveResult::Bound(n) => {
                                    std::str::from_utf8(n.as_ref()).ok().map(str::to_owned)
                                }
                                _ => None,
                            };
                            let local =
                                String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                            Some((uri, local))
                        }
                        _ => None,
                    };
                    (name, event)
                }
                Err(e) => {
                    self.error(DiagCode::MalformedXml, line, e.to_string());
                    return;
                }
            };
            let (name, event) = event;
            line = count_lines(xml, reader.buffer_position() as usize);

            match event {
                Event::Start(ref e) | Event::Empty(ref e) => {
                    let (ns, local) = name.expect("start events always carry a name");
                    let attrs = read_attributes(&mut reader, e);
                    let qname = self.v.schemas.qname(ns.as_deref(), &local);
                    self.start(qname, &ns, &local, attrs, line);
                    if matches!(event, Event::Empty(_)) {
                        // An empty element is a start and an end at once.
                        self.end(line);
                    }
                }
                Event::End(_) => self.end(line),
                Event::Text(t) => {
                    // References arrive as their own `GeneralRef` events, so
                    // what reaches here is literal text with nothing left to
                    // unescape.
                    match t.decode() {
                        Ok(text) => {
                            if let Some(f) = self.stack.last_mut() {
                                f.text.push_str(&text);
                            }
                        }
                        Err(e) => self.error(DiagCode::MalformedXml, line, e.to_string()),
                    }
                }
                // `&amp;` and `&#233;` are events of their own, *not* part of
                // the surrounding text. Ignoring them reads `caf&#233;` as
                // `caf` — silently, which is the worst way to be wrong about
                // a value.
                Event::GeneralRef(r) => {
                    let resolved = match r.resolve_char_ref() {
                        Ok(Some(c)) => Some(c.to_string()),
                        Ok(None) => r
                            .decode()
                            .ok()
                            .and_then(|name| predefined_entity(&name).map(str::to_owned)),
                        Err(e) => {
                            self.error(DiagCode::MalformedXml, line, e.to_string());
                            continue;
                        }
                    };
                    match resolved {
                        Some(text) => {
                            if let Some(f) = self.stack.last_mut() {
                                f.text.push_str(&text);
                            }
                        }
                        // A general entity from a DTD. Expanding it needs the
                        // declaration, which this reader does not process, and
                        // guessing the empty string would quietly corrupt the
                        // value.
                        None => {
                            let name = r.decode().unwrap_or_default().into_owned();
                            self.error(
                                DiagCode::MalformedXml,
                                line,
                                format!(
                                    "cannot expand the entity reference `&{name};`; \
                                     only the five XML predefines and character \
                                     references are resolved"
                                ),
                            );
                        }
                    }
                }
                Event::CData(c) => {
                    if let Ok(text) = std::str::from_utf8(c.as_ref()) {
                        if let Some(f) = self.stack.last_mut() {
                            f.text.push_str(text);
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }

        if let Some(f) = self.stack.last() {
            let (line, name) = (f.line, f.name);
            let shown = self.show(name);
            self.error(
                DiagCode::MalformedXml,
                line,
                format!("document ended with `{shown}` still open"),
            );
        }

        // Only now: an `xs:IDREF` may name an `xs:ID` that appears later.
        for (reference, line) in std::mem::take(&mut self.idrefs) {
            if !self.ids.contains_key(&reference) {
                self.error(
                    DiagCode::UnresolvedIdRef,
                    line,
                    format!("`{reference}` matches no `xs:ID` in this document"),
                );
            }
        }
    }

    // -- elements ----------------------------------------------------------

    fn start(
        &mut self,
        qname: Option<QName>,
        ns: &Option<String>,
        local: &str,
        attrs: Vec<RawAttr>,
        line: u32,
    ) {
        // In scope for this element's attributes and for its text, so it goes
        // on before any of them are looked at and comes off in `end`.
        self.namespaces.push(declared_namespaces(&attrs));
        // An `xs:ID` binds a value to *this* element, so it needs an identity
        // before its attributes are read.
        self.elements_seen += 1;

        // Inside a skipped subtree nothing is checked, but nesting still has
        // to be tracked so the right `End` closes it.
        if self.stack.last().is_some_and(|f| f.skipped) {
            let name = qname.unwrap_or_else(|| self.stack.last().unwrap().name);
            self.stack.push(Frame {
                id_scope: self.elements_seen,
                name,
                declaration: None,
                type_id: self.v.schemas.builtin(crate::datatypes::Builtin::AnyType),
                matcher: None,
                text: String::new(),
                nil: false,
                skipped: true,
                line,
            });
            return;
        }

        let shown = match qname {
            Some(q) => self.show(q),
            None => match ns {
                Some(u) => format!("{{{u}}}{local}"),
                None => local.to_string(),
            },
        };

        // Which declaration? The root asks the schema; anything else asks its
        // parent's content model, because the same name can be a different
        // declaration elsewhere.
        let (declaration, mut skipped) = if self.stack.is_empty() {
            match qname.and_then(|q| self.v.schemas.globals().elements.get(&q).copied()) {
                Some(id) => (Some(id), false),
                None => {
                    self.error(
                        DiagCode::ElementNotDeclared,
                        line,
                        format!("no global element declaration for `{shown}`"),
                    );
                    (None, true)
                }
            }
        } else {
            self.match_in_parent(qname, ns.as_deref(), local, &shown, line)
        };

        if declaration.is_none() && !skipped {
            // A `lax` wildcard with nothing to match: legal, unchecked.
            skipped = true;
        }

        let declared_type = declaration
            .map(|d| self.v.schemas[d].type_id)
            .unwrap_or_else(|| self.v.schemas.builtin(crate::datatypes::Builtin::AnyType));

        let (type_id, type_from_instance) =
            self.resolve_xsi_type(&attrs, declaration, declared_type, line);

        // An abstract type is a placeholder for its derivations; nothing is
        // ever validated against it directly, whether it was declared or
        // chosen by `xsi:type`.
        if !skipped
            && self.v.schemas[type_id]
                .as_complex()
                .is_some_and(|c| c.is_abstract)
        {
            let name = self.show_type(type_id);
            self.error(
                DiagCode::AbstractType,
                line,
                format!("`{shown}` is validated against {name}, which is abstract"),
            );
        }

        // `xsi:nil` is an `xs:boolean`, so `1` says the same as `true`.
        let nil = attrs.iter().any(|a| {
            a.namespace.as_deref() == Some(XSI)
                && a.local == "nil"
                && matches!(a.value.trim(), "true" | "1")
        });

        let attributes = if skipped {
            Vec::new()
        } else {
            self.check_attributes(type_id, &attrs, line)
        };

        let matcher = (!skipped)
            .then(|| self.v.schemas.match_content(type_id))
            .flatten();

        let name = qname.unwrap_or_else(|| {
            // An undeclared name still needs a key for the stack, and interning
            // is not possible after compilation. The parent's name stands in
            // where there is one; at the root there is not, and reaching for
            // some arbitrary global instead used to panic on a schema that
            // declares no elements at all.
            self.stack
                .last()
                .map(|f| f.name)
                .unwrap_or(crate::names::QName::UNKNOWN)
        });

        (self.sink)(PsviEvent::StartElement {
            name,
            declaration,
            type_id,
            type_from_instance,
            nil,
            attributes: attributes.clone(),
            line,
        });

        self.stack.push(Frame {
            id_scope: self.elements_seen,
            name,
            declaration,
            type_id,
            matcher,
            text: String::new(),
            nil,
            skipped,
            line,
        });
    }

    /// Asks the enclosing content model whether this element belongs here,
    /// and which declaration it is.
    fn match_in_parent(
        &mut self,
        qname: Option<QName>,
        ns_uri: Option<&str>,
        local: &str,
        shown: &str,
        line: u32,
    ) -> (Option<ElementId>, bool) {
        let parent = self.stack.last_mut().expect("called with a parent");
        let parent_name = parent.name;
        let Some(matcher) = parent.matcher.as_mut() else {
            let owner = self.show(parent_name);
            self.error(
                DiagCode::UnexpectedElement,
                line,
                format!("`{owner}` has no element content, so `{shown}` cannot appear in it"),
            );
            return (None, true);
        };
        let Some(q) = qname else {
            // The schema declares no such name, so only a wildcard can admit
            // it — which is exactly what wildcards are for. Matching on
            // interned ids alone would reject every foreign element.
            if matcher.step_foreign(ns_uri, local) {
                // A name the schema never interned has no global declaration
                // to find, so `lax` has nothing to check it against and only
                // `strict` has anything to say.
                let strict = matcher.matched_wildcard() == Some(ProcessContents::Strict);
                if strict {
                    self.error(
                        DiagCode::ElementNotDeclared,
                        line,
                        format!("`{shown}` is admitted by a `strict` wildcard, which requires a global element declaration"),
                    );
                }
                return (None, true);
            }
            let owner = self.show(parent_name);
            self.error(
                DiagCode::UnexpectedElement,
                line,
                format!("`{shown}` is not permitted in `{owner}`"),
            );
            return (None, true);
        };
        if matcher.step(q) {
            if let Some(mode) = matcher.matched_wildcard() {
                // A wildcard admitted it, so `processContents` decides what
                // happens next rather than the wildcard's mere presence.
                // `skip` looks no further; the other two want the global
                // declaration this name has, if it has one.
                let global = self.v.schemas.globals().elements.get(&q).copied();
                return match (mode, global) {
                    (ProcessContents::Skip, _) => (None, true),
                    (_, Some(id)) => (Some(id), false),
                    (ProcessContents::Lax, None) => (None, true),
                    (ProcessContents::Strict, None) => {
                        self.error(
                            DiagCode::ElementNotDeclared,
                            line,
                            format!("`{shown}` is admitted by a `strict` wildcard, which requires a global element declaration"),
                        );
                        (None, true)
                    }
                };
            }
            (matcher.matched(), false)
        } else {
            let owner = self.show(parent_name);
            self.error(
                DiagCode::UnexpectedElement,
                line,
                format!("`{shown}` is not permitted in `{owner}` at this position"),
            );
            (None, true)
        }
    }

    /// Applies `xsi:type`, which lets the instance choose any type derived
    /// from the declared one.
    fn resolve_xsi_type(
        &mut self,
        attrs: &[RawAttr],
        declaration: Option<ElementId>,
        declared: TypeId,
        line: u32,
    ) -> (TypeId, bool) {
        let Some(attr) = attrs
            .iter()
            .find(|a| a.namespace.as_deref() == Some(XSI) && a.local == "type")
        else {
            return (declared, false);
        };

        // The value is a QName, so its prefix binds in the document — on any
        // open element, not just this one.
        let (prefix, local) = match attr.value.split_once(':') {
            Some((p, l)) => (Some(p), l),
            None => (None, attr.value.as_str()),
        };
        let uri = Scopes(&self.namespaces).resolve(prefix).map(str::to_owned);
        if let Some(p) = prefix.filter(|_| uri.is_none()) {
            let msg = format!("`xsi:type` uses the prefix `{p}`, which is not bound here");
            self.error(DiagCode::InvalidXsiType, line, msg);
            return (declared, false);
        }
        let Some(q) = self.v.schemas.qname(uri.as_deref(), local) else {
            self.error(
                DiagCode::InvalidXsiType,
                line,
                format!("`xsi:type` names an unknown type `{}`", attr.value),
            );
            return (declared, false);
        };
        let Some(&chosen) = self.v.schemas.globals().types.get(&q) else {
            self.error(
                DiagCode::InvalidXsiType,
                line,
                format!("`xsi:type` names an unknown type `{}`", attr.value),
            );
            return (declared, false);
        };
        if !self.v.schemas.derives_from(chosen, declared) {
            let want = self.show(q);
            self.error(
                DiagCode::InvalidXsiType,
                line,
                format!("`xsi:type` names `{want}`, which is not derived from the declared type"),
            );
            return (declared, false);
        }

        // The substitution has to be one the schema permits: `block` on the
        // element declaration and on the declared type both forbid reaching a
        // type by the methods they name.
        let blocked = declaration
            .map(|d| self.v.schemas[d].block)
            .unwrap_or_default()
            .union(
                self.v.schemas[declared]
                    .as_complex()
                    .map(|c| c.block)
                    .unwrap_or_default(),
            );
        if !self
            .v
            .schemas
            .derives_from_unblocked(chosen, declared, blocked)
        {
            let want = self.show(q);
            self.error(
                DiagCode::InvalidXsiType,
                line,
                format!("`xsi:type` names `{want}`, which is blocked from substituting here"),
            );
            return (declared, false);
        }
        (chosen, true)
    }

    // -- attributes --------------------------------------------------------

    fn check_attributes(
        &mut self,
        type_id: TypeId,
        attrs: &[RawAttr],
        line: u32,
    ) -> Vec<AttributePsvi> {
        let uses: Vec<AttributeUse> = self.v.schemas.attribute_uses(type_id).to_vec();
        let wildcard = self.v.schemas[type_id]
            .as_complex()
            .and_then(|c| c.attribute_wildcard.clone());

        let mut out = Vec::new();
        let mut seen = Vec::new();

        for a in attrs {
            // Namespace declarations are not attributes for validation.
            if a.local == "xmlns" || a.namespace.as_deref() == Some(crate::names::XMLNS) {
                continue;
            }
            // xsi:* is defined by the instance schema, not the document's.
            if a.namespace.as_deref() == Some(XSI) {
                continue;
            }

            let Some(q) = self.v.schemas.qname(a.namespace.as_deref(), &a.local) else {
                // A name the schema never interned has no global declaration
                // to find, so `lax` has nothing to check it against and only
                // `strict` has anything to say.
                let msg = match self.wildcard_for_attribute(a.namespace.as_deref(), None, &wildcard)
                {
                    Some(ProcessContents::Strict) => Some(format!(
                        "attribute `{}` is admitted by a `strict` wildcard, which requires a global attribute declaration",
                        a.local
                    )),
                    Some(_) => None,
                    None => Some(format!(
                        "attribute `{}` is not permitted on this element",
                        a.local
                    )),
                };
                if let Some(msg) = msg {
                    self.error(DiagCode::AttributeNotAllowed, line, msg);
                }
                continue;
            };

            let found = uses
                .iter()
                .find(|u| self.v.schemas[u.attribute].name == q)
                .cloned();

            match found {
                Some(u) if u.kind == AttributeUseKind::Prohibited => {
                    let shown = self.show(q);
                    self.error(
                        DiagCode::AttributeNotAllowed,
                        line,
                        format!("attribute `{shown}` is prohibited on this element"),
                    );
                }
                Some(u) => {
                    seen.push(q);
                    let ty = self.v.schemas[u.attribute].type_id;
                    let value =
                        match self
                            .v
                            .values
                            .validate_in(ty, &a.value, &Scopes(&self.namespaces))
                        {
                            Ok(v) => Some(v),
                            Err(e) => {
                                let shown = self.show(q);
                                self.error(
                                    DiagCode::InvalidValue,
                                    line,
                                    format!("attribute `{shown}`: {e}"),
                                );
                                None
                            }
                        };
                    self.record_identifiers(ty, &a.value, self.elements_seen, line);
                    // A `fixed` value is a constraint, not a default: the
                    // document may repeat it but may not differ from it.
                    // Compared in the value space, as for an element, so
                    // `1.0` satisfies a decimal fixed at `1.00`.
                    let constraint = u
                        .value_constraint
                        .clone()
                        .or_else(|| self.v.schemas[u.attribute].value_constraint.clone());
                    if let (Some(vc), Some(v)) = (&constraint, &value) {
                        if vc.is_fixed() {
                            let want = self
                                .v
                                .values
                                .validate_in(ty, vc.value(), &Scopes(&self.namespaces))
                                .ok();
                            if want.as_ref() != Some(v) {
                                let shown = self.show(q);
                                self.error(
                                    DiagCode::InvalidValue,
                                    line,
                                    format!(
                                        "attribute `{shown}` is fixed at `{}`, not `{}`",
                                        vc.value(),
                                        a.value
                                    ),
                                );
                            }
                        }
                    }
                    out.push(AttributePsvi {
                        name: q,
                        declaration: Some(u.attribute),
                        value,
                        lexical: a.value.clone(),
                        from_schema: false,
                    });
                }
                None => {
                    let shown = self.show(q);
                    let mode =
                        self.wildcard_for_attribute(a.namespace.as_deref(), Some(q), &wildcard);
                    let global = self.v.schemas.globals().attributes.get(&q).copied();
                    // A wildcard admits the *name*; `processContents` decides
                    // whether the value is looked at.
                    let declaration = match (mode, global) {
                        (None, _) => {
                            let msg =
                                format!("attribute `{shown}` is not permitted on this element");
                            self.error(DiagCode::AttributeNotAllowed, line, msg);
                            None
                        }
                        (Some(ProcessContents::Skip), _) => None,
                        (Some(_), Some(id)) => Some(id),
                        (Some(ProcessContents::Lax), None) => None,
                        (Some(ProcessContents::Strict), None) => {
                            let msg = format!(
                                "attribute `{shown}` is admitted by a `strict` wildcard, which requires a global attribute declaration"
                            );
                            self.error(DiagCode::AttributeNotAllowed, line, msg);
                            None
                        }
                    };
                    let value = match declaration {
                        Some(id) => {
                            let ty = self.v.schemas[id].type_id;
                            let v = match self.v.values.validate_in(
                                ty,
                                &a.value,
                                &Scopes(&self.namespaces),
                            ) {
                                Ok(v) => Some(v),
                                Err(e) => {
                                    let msg = format!("attribute `{shown}`: {e}");
                                    self.error(DiagCode::InvalidValue, line, msg);
                                    None
                                }
                            };
                            self.record_identifiers(ty, &a.value, self.elements_seen, line);
                            v
                        }
                        None => None,
                    };
                    out.push(AttributePsvi {
                        name: q,
                        declaration,
                        value,
                        lexical: a.value.clone(),
                        from_schema: false,
                    });
                }
            }
        }

        for u in &uses {
            let name = self.v.schemas[u.attribute].name;
            if seen.contains(&name) {
                continue;
            }
            if u.kind == AttributeUseKind::Required {
                let shown = self.show(name);
                self.error(
                    DiagCode::MissingRequiredAttribute,
                    line,
                    format!("required attribute `{shown}` is absent"),
                );
                continue;
            }
            if u.kind == AttributeUseKind::Prohibited {
                continue;
            }
            // An absent attribute with `fixed` or `default` is *supplied* by
            // the schema. That is the whole point of a schema-fixed unit:
            // `<length>3.2</length>` still has a unit, and a reader that only
            // reported attributes the document spelled out would miss it.
            let Some(vc) = u
                .value_constraint
                .as_ref()
                .or(self.v.schemas[u.attribute].value_constraint.as_ref())
            else {
                continue;
            };
            let ty = self.v.schemas[u.attribute].type_id;
            let lexical = vc.value().to_string();
            let value = self
                .v
                .values
                .validate_in(ty, &lexical, &Scopes(&self.namespaces))
                .ok();
            // A schema-supplied `xs:ID` is still an identifier in the
            // document it lands in.
            self.record_identifiers(ty, &lexical, self.elements_seen, line);
            out.push(AttributePsvi {
                name,
                declaration: Some(u.attribute),
                value,
                lexical,
                from_schema: true,
            });
        }

        out
    }

    /// How the type's wildcard says an attribute that no declaration matched
    /// should be processed, or `None` when no wildcard admits it at all.
    ///
    /// `name` is absent when the schema never interned it — the usual case
    /// for a wildcard, which exists to admit what the schema does not
    /// declare, so the namespace is matched by URI rather than by id.
    fn wildcard_for_attribute(
        &self,
        ns: Option<&str>,
        name: Option<QName>,
        wildcard: &Option<Wildcard>,
    ) -> Option<ProcessContents> {
        let w = wildcard.as_ref()?;
        let admitted = w.namespace.admits_uri(self.v.schemas.names(), ns)
            && !name.is_some_and(|q| w.not_qname.contains(&q))
            && !(w.not_defined
                && name.is_some_and(|q| self.v.schemas.globals().attributes.contains_key(&q)));
        admitted.then_some(w.process_contents)
    }

    // -- ends --------------------------------------------------------------

    fn end(&mut self, line: u32) {
        let Some(frame) = self.stack.pop() else {
            self.error(
                DiagCode::MalformedXml,
                line,
                "an end tag with no open element",
            );
            return;
        };
        self.finish(frame, line);
        // Only now: the text just checked resolved its QNames against this
        // element's own declarations.
        self.namespaces.pop();
    }

    fn finish(&mut self, frame: Frame<'a>, line: u32) {
        if frame.skipped {
            return;
        }

        let shown = self.show(frame.name);

        if frame.nil {
            if !frame.text.trim().is_empty() {
                self.error(
                    DiagCode::NilElementNotEmpty,
                    line,
                    format!("`{shown}` is `xsi:nil` but has content"),
                );
            }
        } else {
            self.check_content(&frame, &shown, line);
        }

        (self.sink)(PsviEvent::EndElement {
            name: frame.name,
            declaration: frame.declaration,
            line,
        });
    }

    fn check_content(&mut self, frame: &Frame<'a>, shown: &str, line: u32) {
        let ty = frame.type_id;

        // A simple type, or a complex type with simple content, validates its
        // character data. Everything else must have none of significance.
        let simple_target = if self.v.schemas[ty].is_simple() {
            Some(ty)
        } else {
            match self.v.schemas[ty].as_complex().map(|c| c.content) {
                Some(ContentType::Simple(t)) if !t.is_placeholder() => Some(t),
                _ => None,
            }
        };

        if let Some(target) = simple_target {
            // An element with no character content takes its declaration's
            // `default` or `fixed` value — the schema supplying what the
            // document left out, exactly as for an absent attribute. An
            // `xsi:nil` element never reaches here, which is right: nil and a
            // value constraint are alternatives, not a pair.
            let constraint = frame
                .declaration
                .and_then(|d| self.v.schemas[d].value_constraint.clone());
            let from_schema = constraint.is_some() && frame.text.is_empty();
            let lexical = match &constraint {
                Some(vc) if from_schema => vc.value().to_string(),
                _ => frame.text.clone(),
            };
            let value = match self
                .v
                .values
                .validate_in(target, &lexical, &Scopes(&self.namespaces))
            {
                Ok(v) => Some(v),
                Err(e) => {
                    self.error(DiagCode::InvalidValue, line, format!("`{shown}`: {e}"));
                    None
                }
            };
            // `fixed` is a constraint, not a default: content the document
            // did write may not differ from it. Compared in the value space,
            // so `1.0` satisfies a decimal fixed at `1.00`.
            if let (Some(vc), Some(v)) = (&constraint, &value) {
                if vc.is_fixed() && !from_schema {
                    let want = self
                        .v
                        .values
                        .validate_in(target, vc.value(), &Scopes(&self.namespaces))
                        .ok();
                    if want.as_ref() != Some(v) {
                        let msg =
                            format!("`{shown}` is fixed at `{}`, not `{lexical}`", vc.value());
                        self.error(DiagCode::InvalidValue, line, msg);
                    }
                }
            }
            self.record_identifiers(target, &lexical, frame.id_scope, line);
            (self.sink)(PsviEvent::Text {
                value,
                type_id: target,
                lexical,
                from_schema,
                line,
            });
        } else {
            let mixed = self.v.schemas.content_is_mixed(ty);
            if !mixed && !frame.text.trim().is_empty() {
                self.error(
                    DiagCode::UnexpectedText,
                    line,
                    format!("`{shown}` has element-only content, but contains character data"),
                );
            }
        }

        if let Some(m) = &frame.matcher {
            if !m.accepts_end() {
                self.error(
                    DiagCode::IncompleteContent,
                    line,
                    format!("`{shown}` ended before its content model was satisfied"),
                );
            }
        }
    }
}

/// Reads an element's attributes with their prefixes resolved.
fn read_attributes(
    reader: &mut NsReader<&[u8]>,
    e: &quick_xml::events::BytesStart<'_>,
) -> Vec<RawAttr> {
    let attrs: Vec<_> = e.attributes().filter_map(Result::ok).collect();
    attrs
        .into_iter()
        .map(|a| {
            let (ns, local) = reader.resolver_mut().resolve_attribute(a.key);
            let namespace = match ns {
                ResolveResult::Bound(n) => std::str::from_utf8(n.as_ref()).ok().map(str::to_owned),
                _ => None,
            };
            RawAttr {
                namespace,
                local: String::from_utf8_lossy(local.as_ref()).into_owned(),
                // Attribute-value normalization is required by XML 1.0
                // §3.3.3 — tab, CR and LF become spaces — and is exactly the
                // difference between an attribute and element text.
                value: a
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// The five entities XML predefines. Anything else is declared in a DTD,
/// which this reader does not process.
fn predefined_entity(name: &str) -> Option<&'static str> {
    match name {
        "amp" => Some("&"),
        "lt" => Some("<"),
        "gt" => Some(">"),
        "quot" => Some("\""),
        "apos" => Some("'"),
        _ => None,
    }
}

/// Whether values of this type can be document-scope identifiers.
///
/// A list of either plays the same role item by item, so the item type
/// decides for one. A union cannot be answered from the type alone — which
/// member matched decides — so it defers to the value.
fn id_kind(schemas: &Schemas, ty: TypeId) -> Option<IdKind> {
    let simple = schemas[ty].as_simple();
    let target = match simple {
        Some(t) if t.variety == crate::datatypes::Variety::List => t.item_type.unwrap_or(ty),
        _ => ty,
    };
    if let Some(t) = schemas[target].as_simple() {
        if t.variety == crate::datatypes::Variety::Union {
            // Only worth asking per value if some member could be one.
            let any = t
                .member_types
                .iter()
                .any(|m| id_kind(schemas, *m).is_some());
            return any.then_some(IdKind::PerValue);
        }
    }
    match nearest_builtin(schemas, target) {
        Some(crate::datatypes::Builtin::Id) => Some(IdKind::Defines),
        Some(crate::datatypes::Builtin::IdRef | crate::datatypes::Builtin::IdRefs) => {
            Some(IdKind::References)
        }
        _ => None,
    }
}

fn count_lines(xml: &str, upto: usize) -> u32 {
    let upto = upto.min(xml.len());
    (xml.as_bytes()[..upto]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1) as u32
}

impl Schemas {
    /// Builds an [`InstanceValidator`] over this schema.
    pub fn instance_validator(&self) -> InstanceValidator<'_> {
        InstanceValidator::new(self)
    }
}

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
use crate::validate::Validator;
use crate::values::Value;
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
    uri: String,
    sink: S,
}

impl<'a, S: FnMut(PsviEvent)> Run<'a, '_, S> {
    fn error(&mut self, code: DiagCode, line: u32, msg: impl Into<String>) {
        let span = Span::new(&self.uri, line);
        self.diags.push(Diagnostic::error(code, msg).at(span));
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
                    // Decode, then resolve character and predefined entity
                    // references — done explicitly so it is obvious that
                    // element text is unescaped before it becomes a value.
                    if let Ok(raw) = t.decode() {
                        if let Ok(text) = quick_xml::escape::unescape(&raw) {
                            if let Some(f) = self.stack.last_mut() {
                                f.text.push_str(&text);
                            }
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
        // Inside a skipped subtree nothing is checked, but nesting still has
        // to be tracked so the right `End` closes it.
        if self.stack.last().is_some_and(|f| f.skipped) {
            let name = qname.unwrap_or_else(|| self.stack.last().unwrap().name);
            self.stack.push(Frame {
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
            self.match_in_parent(qname, &shown, line)
        };

        if declaration.is_none() && !skipped {
            // A `lax` wildcard with nothing to match: legal, unchecked.
            skipped = true;
        }

        let declared_type = declaration
            .map(|d| self.v.schemas[d].type_id)
            .unwrap_or_else(|| self.v.schemas.builtin(crate::datatypes::Builtin::AnyType));

        let (type_id, type_from_instance) = self.resolve_xsi_type(&attrs, declared_type, line);
        let nil = attrs
            .iter()
            .any(|a| a.namespace.as_deref() == Some(XSI) && a.local == "nil" && a.value == "true");

        let attributes = if skipped {
            Vec::new()
        } else {
            self.check_attributes(type_id, &attrs, line)
        };

        let matcher = (!skipped)
            .then(|| self.v.schemas.match_content(type_id))
            .flatten();

        let name = qname.unwrap_or_else(|| {
            // An undeclared name still needs a key for the stack; interning is
            // not possible after compilation, so the parent's stands in.
            self.stack.last().map(|f| f.name).unwrap_or_else(|| {
                *self
                    .v
                    .schemas
                    .globals()
                    .elements
                    .keys()
                    .next()
                    .expect("a schema has names")
            })
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
            // A name no component in the schema carries cannot match anything.
            let owner = self.show(parent_name);
            self.error(
                DiagCode::UnexpectedElement,
                line,
                format!("`{shown}` is not permitted in `{owner}`"),
            );
            return (None, true);
        };
        if matcher.step(q) {
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
        declared: TypeId,
        line: u32,
    ) -> (TypeId, bool) {
        let Some(attr) = attrs
            .iter()
            .find(|a| a.namespace.as_deref() == Some(XSI) && a.local == "type")
        else {
            return (declared, false);
        };

        // The value is a QName, so its prefix binds in the document.
        let (prefix, local) = match attr.value.split_once(':') {
            Some((p, l)) => (Some(p), l),
            None => (None, attr.value.as_str()),
        };
        let uri = self.prefix_lookup(attrs, prefix);
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
        (chosen, true)
    }

    /// Resolves a prefix declared on this element, falling back to the
    /// schema's own namespaces for the common `xs:` case.
    fn prefix_lookup(&self, attrs: &[RawAttr], prefix: Option<&str>) -> Option<String> {
        let want = match prefix {
            Some(p) => format!("xmlns:{p}"),
            None => "xmlns".to_string(),
        };
        attrs
            .iter()
            .find(|a| {
                let key = match &a.namespace {
                    Some(_) => format!("xmlns:{}", a.local),
                    None => a.local.clone(),
                };
                key == want || (a.local == want)
            })
            .map(|a| a.value.clone())
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
                self.report_unknown_attribute(&a.local, &wildcard, line);
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
                    let value = match self.v.values.validate(ty, &a.value) {
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
                    // A `fixed` value is a constraint, not a default: the
                    // document may repeat it but may not differ from it.
                    if let Some(vc) = u
                        .value_constraint
                        .as_ref()
                        .or(self.v.schemas[u.attribute].value_constraint.as_ref())
                    {
                        if vc.is_fixed() && vc.value() != a.value {
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
                    out.push(AttributePsvi {
                        name: q,
                        declaration: Some(u.attribute),
                        value,
                        lexical: a.value.clone(),
                    });
                }
                None => {
                    let shown = self.show(q);
                    self.report_unknown_attribute(&shown, &wildcard, line);
                    out.push(AttributePsvi {
                        name: q,
                        declaration: None,
                        value: None,
                        lexical: a.value.clone(),
                    });
                }
            }
        }

        for u in &uses {
            if u.kind == AttributeUseKind::Required {
                let name = self.v.schemas[u.attribute].name;
                if !seen.contains(&name) {
                    let shown = self.show(name);
                    self.error(
                        DiagCode::MissingRequiredAttribute,
                        line,
                        format!("required attribute `{shown}` is absent"),
                    );
                }
            }
        }

        out
    }

    fn report_unknown_attribute(&mut self, shown: &str, wildcard: &Option<Wildcard>, line: u32) {
        if wildcard.is_some() {
            // An `anyAttribute` admits it; `strict` processing of attribute
            // wildcards is not implemented, so it is accepted unchecked.
            return;
        }
        self.error(
            DiagCode::AttributeNotAllowed,
            line,
            format!("attribute `{shown}` is not permitted on this element"),
        );
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
            let value = match self.v.values.validate(target, &frame.text) {
                Ok(v) => Some(v),
                Err(e) => {
                    self.error(DiagCode::InvalidValue, line, format!("`{shown}`: {e}"));
                    None
                }
            };
            (self.sink)(PsviEvent::Text {
                value,
                type_id: target,
                lexical: frame.text.clone(),
                line,
            });
        } else {
            let mixed = self.v.schemas[ty]
                .as_complex()
                .is_some_and(|c| c.content.is_mixed());
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

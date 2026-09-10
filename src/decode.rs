//! Turning a document into data.
//!
//! Validation already produces a typed PSVI — every element with the type in
//! force, every value parsed into its value space. What it does not produce
//! is a *shape*: reconstructing a nested value out of
//! `StartElement`/`Text`/`EndElement` is work every consumer would otherwise
//! redo, and redo differently.
//!
//! [`Decoded`] is that shape, and it is lossless: qualified names, the type
//! in force after any `xsi:type`, `xsi:nil`, and which values the schema
//! supplied rather than the document. The Python bindings project it onto
//! dictionaries, which is lossy on purpose; this is the layer that is not.
//!
//! ```no_run
//! use xsdkit::Schemas;
//!
//! fn read(schemas: &Schemas, xml: &str) -> Option<()> {
//!     let doc = schemas.decode(xml).into_result().ok()?;
//!     for child in doc.children() {
//!         println!("{}", schemas.display_name(child.name));
//!     }
//!     Some(())
//! }
//! ```

use crate::diagnostics::Diagnostics;
use crate::instance::PsviEvent;
use crate::model::{ElementId, Schemas, TypeId};
use crate::names::QName;
use crate::values::Value;

// Not serializable, deliberately. `Schemas` is, because compiling is the
// expensive step worth caching; a decoded tree is cheap to reproduce and
// holds `Value`, whose wire form is a real design question — is an
// `xs:decimal` a JSON number or a string? — and belongs with the converters
// the decoder does not have yet.
/// One element of a decoded document.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub name: QName,
    /// The type in force, after any `xsi:type` override.
    pub type_id: TypeId,
    /// The declaration matched, absent under a `skip` wildcard or a `lax` one
    /// with nothing to match.
    pub declaration: Option<ElementId>,
    /// `xsi:nil="true"`. A nil element has no content and that is not the
    /// same as being empty.
    pub nil: bool,
    pub attributes: Vec<DecodedAttribute>,
    pub content: DecodedContent,
}

/// One attribute, with its value in the value space of its type.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedAttribute {
    pub name: QName,
    /// `None` when the value did not validate, or when no declaration was
    /// matched to give it a type.
    pub value: Option<Value>,
    pub lexical: String,
    /// True when the document did not spell this attribute out and the schema
    /// supplied it from a `default` or `fixed` value.
    pub from_schema: bool,
}

/// What an element contains.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum DecodedContent {
    /// Neither child elements nor character data.
    #[default]
    Empty,
    /// Character data, typed where the type has a value space.
    Simple {
        /// `None` when the lexical form did not validate against the type.
        value: Option<Value>,
        lexical: String,
        /// True when the element was empty and its declaration's `default` or
        /// `fixed` supplied the content.
        from_schema: bool,
    },
    /// Child elements, and for a mixed type the character data around them.
    ///
    /// `text` is the element's character data concatenated in document order.
    /// Where each run sat *between* the children is not preserved: the
    /// validator accumulates character data per element rather than per run,
    /// and this is a decoder over what validation produces. For the
    /// data-oriented schemas this crate is built for that is a non-question;
    /// for marked-up prose it means `text` tells you what was said and not
    /// where.
    Elements {
        children: Vec<Decoded>,
        text: String,
    },
}

impl Decoded {
    /// The child elements, in document order. Empty for simple content.
    pub fn children(&self) -> &[Decoded] {
        match &self.content {
            DecodedContent::Elements { children, .. } => children,
            _ => &[],
        }
    }

    /// The typed value of simple content, if it validated.
    pub fn value(&self) -> Option<&Value> {
        match &self.content {
            DecodedContent::Simple { value, .. } => value.as_ref(),
            _ => None,
        }
    }

    /// The character data, whether this element has simple or mixed content.
    pub fn text(&self) -> &str {
        match &self.content {
            DecodedContent::Simple { lexical, .. } => lexical,
            DecodedContent::Elements { text, .. } => text,
            DecodedContent::Empty => "",
        }
    }

    /// The first child with this name, by qualified name.
    pub fn child(&self, name: QName) -> Option<&Decoded> {
        self.children().iter().find(|c| c.name == name)
    }

    /// An attribute by qualified name.
    pub fn attribute(&self, name: QName) -> Option<&DecodedAttribute> {
        self.attributes.iter().find(|a| a.name == name)
    }
}

/// What decoding produced: the tree, and every diagnostic raised on the way.
///
/// Both halves, for the same reason [`crate::Compilation`] returns both: a
/// document can be worth reading and still have something wrong with it, and
/// which of those matters is the caller's call, not this crate's.
#[derive(Clone, Debug)]
pub struct Decoding {
    /// `None` only when the document had no element at all — not merely when
    /// it was invalid.
    pub decoded: Option<Decoded>,
    pub diagnostics: Diagnostics,
}

impl Decoding {
    /// The tree, or every diagnostic if the document did not validate.
    pub fn into_result(self) -> Result<Decoded, Diagnostics> {
        match self.decoded {
            Some(d) if !self.diagnostics.has_errors() => Ok(d),
            _ => Err(self.diagnostics),
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.diagnostics.has_errors()
    }
}

/// Assembles the tree from the event stream.
///
/// The ordering the validator guarantees is what makes this a stack and not a
/// search: an element's `Text` arrives when the element ends, after every
/// child of it has already ended.
#[derive(Default)]
struct Builder {
    stack: Vec<Decoded>,
    root: Option<Decoded>,
}

impl Builder {
    fn event(&mut self, event: PsviEvent) {
        match event {
            PsviEvent::StartElement {
                name,
                declaration,
                type_id,
                nil,
                attributes,
                ..
            } => {
                self.stack.push(Decoded {
                    name,
                    type_id,
                    declaration,
                    nil,
                    attributes: attributes
                        .into_iter()
                        .map(|a| DecodedAttribute {
                            name: a.name,
                            value: a.value,
                            lexical: a.lexical,
                            from_schema: a.from_schema,
                        })
                        .collect(),
                    content: DecodedContent::Empty,
                });
            }
            PsviEvent::Text {
                value,
                lexical,
                from_schema,
                ..
            } => {
                if let Some(top) = self.stack.last_mut() {
                    match &mut top.content {
                        // Mixed content: the children arrived first, so this
                        // is the character data around them.
                        DecodedContent::Elements { text, .. } => *text = lexical,
                        _ => {
                            top.content = DecodedContent::Simple {
                                value,
                                lexical,
                                from_schema,
                            }
                        }
                    }
                }
            }
            PsviEvent::EndElement { .. } => {
                let Some(done) = self.stack.pop() else {
                    return;
                };
                match self.stack.last_mut() {
                    Some(parent) => match &mut parent.content {
                        DecodedContent::Elements { children, .. } => children.push(done),
                        _ => {
                            parent.content = DecodedContent::Elements {
                                children: vec![done],
                                text: String::new(),
                            }
                        }
                    },
                    None => self.root = Some(done),
                }
            }
        }
    }
}

impl Schemas {
    /// Validate a document and decode it into a tree of typed values.
    ///
    /// A consumer of validation rather than a second implementation of it:
    /// the same pass produces the same diagnostics, and the tree is assembled
    /// from the events it emits.
    ///
    /// ```no_run
    /// # use xsdkit::Schemas;
    /// # fn f(schemas: &Schemas, xml: &str) {
    /// let decoding = schemas.decode(xml);
    /// if decoding.is_valid() {
    ///     let tree = decoding.decoded.unwrap();
    ///     println!("{} attributes", tree.attributes.len());
    /// }
    /// # }
    /// ```
    pub fn decode(&self, xml: &str) -> Decoding {
        self.decode_named(xml, "<instance>")
    }

    /// [`Self::decode`], naming the document for diagnostics.
    pub fn decode_named(&self, xml: &str, uri: &str) -> Decoding {
        let mut builder = Builder::default();
        let report = self
            .document_validator()
            .validate_named(xml, uri, |e| builder.event(e));
        Decoding {
            decoded: builder.root,
            diagnostics: report.diagnostics,
        }
    }
}

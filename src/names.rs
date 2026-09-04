//! String interning and qualified names.
//!
//! Every name in a compiled schema is a `QName`: a pair of interned ids that
//! is `Copy`, 8 bytes, and comparable without touching a string. Resolution
//! and lookup happen on these, never on `&str` — the same reason
//! `xml2arrow` indexes paths by `PathNodeId`.

use fxhash::FxHashMap;

/// The XML Schema namespace. Built-in types live here.
pub const XS: &str = "http://www.w3.org/2001/XMLSchema";
/// The XML Schema instance namespace (`xsi:type`, `xsi:nil`, …).
pub const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";
/// The `xml:` namespace, implicitly available to every schema.
pub const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// An interned string: either a namespace URI or a local name.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Symbol(u32);

/// An interned namespace URI.
///
/// Absent means "no namespace" — an unqualified name, which is a distinct
/// thing from a name in some default namespace.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Namespace(Symbol);

/// A namespace-qualified name.
///
/// `{ns, local}` is a key for *global* declarations only. Local element and
/// attribute declarations are scoped to their containing type and are not
/// addressable this way — see [`crate::model::Scope`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct QName {
    pub ns: Option<Namespace>,
    pub local: Symbol,
}

impl QName {
    pub fn new(ns: Option<Namespace>, local: Symbol) -> Self {
        Self { ns, local }
    }
}

impl Namespace {
    /// Wraps an already-interned symbol as a namespace.
    pub fn from_symbol(s: Symbol) -> Self {
        Self(s)
    }

    pub fn symbol(self) -> Symbol {
        self.0
    }
}

/// Interns strings so names compare and hash as integers.
#[derive(Clone, Debug, Default)]
pub struct Interner {
    map: FxHashMap<Box<str>, u32>,
    vec: Vec<Box<str>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&i) = self.map.get(s) {
            return Symbol(i);
        }
        let i = self.vec.len() as u32;
        let boxed: Box<str> = s.into();
        self.vec.push(boxed.clone());
        self.map.insert(boxed, i);
        Symbol(i)
    }

    pub fn namespace(&mut self, uri: &str) -> Namespace {
        Namespace(self.intern(uri))
    }

    /// Interns a namespace, mapping the empty URI to "no namespace".
    ///
    /// XSD spells "no target namespace" as an absent `targetNamespace`
    /// attribute; some tools write `targetNamespace=""`, which is invalid but
    /// common enough to accept here.
    pub fn opt_namespace(&mut self, uri: &str) -> Option<Namespace> {
        if uri.is_empty() {
            None
        } else {
            Some(self.namespace(uri))
        }
    }

    pub fn qname(&mut self, ns: Option<&str>, local: &str) -> QName {
        QName {
            ns: ns.and_then(|u| self.opt_namespace(u)),
            local: self.intern(local),
        }
    }

    /// Looks a string up without interning it.
    ///
    /// Returns `None` when the string was never interned, which for a
    /// compiled schema means no component can carry that name.
    pub fn lookup(&self, s: &str) -> Option<Symbol> {
        self.map.get(s).copied().map(Symbol)
    }

    pub fn resolve(&self, s: Symbol) -> &str {
        &self.vec[s.0 as usize]
    }

    pub fn resolve_ns(&self, ns: Namespace) -> &str {
        self.resolve(ns.0)
    }

    /// Renders a `QName` in James Clark notation: `{ns}local`, or `local`
    /// when the name has no namespace.
    pub fn display(&self, q: QName) -> String {
        match q.ns {
            Some(ns) => format!("{{{}}}{}", self.resolve_ns(ns), self.resolve(q.local)),
            None => self.resolve(q.local).to_string(),
        }
    }

    pub fn len(&self) -> usize {
        self.vec.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vec.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_stable_and_deduplicating() {
        let mut i = Interner::new();
        let a = i.intern("well");
        let b = i.intern("well");
        let c = i.intern("wellbore");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(i.resolve(a), "well");
        assert_eq!(i.len(), 2);
    }

    #[test]
    fn empty_target_namespace_is_no_namespace() {
        let mut i = Interner::new();
        assert_eq!(i.opt_namespace(""), None);
        assert!(i.opt_namespace("urn:x").is_some());
    }

    #[test]
    fn qnames_render_in_clark_notation() {
        let mut i = Interner::new();
        let q = i.qname(Some("urn:x"), "well");
        let u = i.qname(None, "well");
        assert_eq!(i.display(q), "{urn:x}well");
        assert_eq!(i.display(u), "well");
        assert_ne!(q, u);
    }
}

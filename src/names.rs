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
/// The namespace of namespace declarations themselves.
pub const XMLNS: &str = "http://www.w3.org/2000/xmlns/";

/// An interned string: either a namespace URI or a local name.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Symbol(u32);

/// An interned namespace URI.
///
/// Absent means "no namespace" — an unqualified name, which is a distinct
/// thing from a name in some default namespace.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Namespace(Symbol);

/// A namespace-qualified name.
///
/// `{ns, local}` is a key for *global* declarations only. Local element and
/// attribute declarations are scoped to their containing type and are not
/// addressable this way — see [`crate::model::Scope`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct QName {
    pub ns: Option<Namespace>,
    pub local: Symbol,
}

impl Symbol {
    /// The empty string, present in every [`Interner`] from construction.
    ///
    /// A `Symbol` cannot be made without an interner, and an interner cannot
    /// be written to after compilation — so code that meets a name the schema
    /// never declared has nothing to fall back on. This is that fallback.
    pub const EMPTY: Symbol = Symbol(0);
}

impl QName {
    pub fn new(ns: Option<Namespace>, local: Symbol) -> Self {
        Self { ns, local }
    }

    /// A stand-in for a name the schema does not declare.
    ///
    /// The instance validator has to key its element stack on *something* even
    /// when a document uses a name no declaration mentions, and it cannot
    /// intern the real one. Naming it after some unrelated global — which is
    /// what this replaced — was worse than admitting the name is unknown, and
    /// panicked outright on a schema with no global elements to borrow from.
    pub const UNKNOWN: QName = QName {
        ns: None,
        local: Symbol::EMPTY,
    };
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
///
/// Every interner holds the empty string at [`Symbol::EMPTY`] from the moment
/// it is created, so a `Symbol` can always be produced without interning —
/// which matters because interning is impossible once a schema is compiled.
#[derive(Clone, Debug)]
pub struct Interner {
    map: FxHashMap<Box<str>, u32>,
    vec: Vec<Box<str>>,
}

/// The interner serializes as its table alone.
///
/// A [`Symbol`] *is* an index into that table, and those indices are spread
/// through every component in the model — so the table has to come back in
/// exactly the order it went out, and the lookup map is rebuilt from it
/// rather than stored. Serializing the map instead would round-trip the
/// strings and silently renumber every name in the schema.
#[cfg(feature = "serde")]
impl serde::Serialize for Interner {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&self.vec, s)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Interner {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let vec: Vec<Box<str>> = serde::Deserialize::deserialize(d)?;
        let map = vec
            .iter()
            .enumerate()
            .map(|(i, s)| (s.clone(), i as u32))
            .collect();
        Ok(Self { map, vec })
    }
}

impl Default for Interner {
    fn default() -> Self {
        let mut i = Self {
            map: FxHashMap::default(),
            vec: Vec::new(),
        };
        // Reserve slot 0 so `Symbol::EMPTY` resolves in every interner.
        i.intern("");
        i
    }
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
        // Two distinct strings, plus the empty string every interner reserves
        // at slot 0 so `Symbol::EMPTY` always resolves.
        assert_eq!(i.len(), 3);
    }

    /// `Symbol::EMPTY` has to resolve in *any* interner, however it was made,
    /// because it is the fallback for a name that cannot be interned any more.
    #[test]
    fn the_empty_symbol_resolves_in_every_interner() {
        for i in [Interner::new(), Interner::default()] {
            assert_eq!(i.resolve(Symbol::EMPTY), "");
        }
        let mut i = Interner::new();
        i.intern("well");
        assert_eq!(i.resolve(Symbol::EMPTY), "", "interning must not move it");
        assert_eq!(i.display(QName::UNKNOWN), "");
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

/// Serializing a map whose key is not a string, as a sequence of pairs.
///
/// `QName` is a struct, and a JSON object's key has to be a string — so the
/// derived impl would make the model serializable to `postcard` and not to
/// `serde_json`. Writing pairs costs nothing in a binary format and keeps
/// every format open.
#[cfg(feature = "serde")]
pub(crate) mod map_as_seq {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::hash::{BuildHasher, Hash};

    pub fn serialize<K, V, H, S>(
        map: &std::collections::HashMap<K, V, H>,
        s: S,
    ) -> Result<S::Ok, S::Error>
    where
        K: Serialize,
        V: Serialize,
        S: Serializer,
    {
        s.collect_seq(map.iter())
    }

    pub fn deserialize<'de, K, V, H, D>(
        d: D,
    ) -> Result<std::collections::HashMap<K, V, H>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash,
        V: Deserialize<'de>,
        H: BuildHasher + Default,
        D: Deserializer<'de>,
    {
        let pairs: Vec<(K, V)> = Deserialize::deserialize(d)?;
        Ok(pairs.into_iter().collect())
    }
}

/// The same, for a set.
#[cfg(feature = "serde")]
pub(crate) mod set_as_seq {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::hash::{BuildHasher, Hash};

    pub fn serialize<T, H, S>(
        set: &std::collections::HashSet<T, H>,
        s: S,
    ) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        s.collect_seq(set.iter())
    }

    pub fn deserialize<'de, T, H, D>(d: D) -> Result<std::collections::HashSet<T, H>, D::Error>
    where
        T: Deserialize<'de> + Eq + Hash,
        H: BuildHasher + Default,
        D: Deserializer<'de>,
    {
        let items: Vec<T> = Deserialize::deserialize(d)?;
        Ok(items.into_iter().collect())
    }
}

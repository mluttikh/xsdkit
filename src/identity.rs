//! `xs:key`, `xs:keyref` and `xs:unique` — the restricted XPath they use, and
//! matching it against a document as it streams past.
//!
//! # Why this XPath is not XPath
//!
//! Identity constraints do not take expressions. XSD Part 1 Appendix I defines
//! a deliberately tiny subset so that a validator needs no XPath engine: an
//! optional `.//` prefix, a sequence of child steps, and — for a field only —
//! an attribute as the final step. No predicates, no functions, no axes beyond
//! `child` and `attribute`. That is the whole grammar, and it is why this
//! module is a few hundred lines rather than a dependency.
//!
//! # Matching without a tree
//!
//! The validator never holds a document, so a path is matched against the
//! *stack* of open elements rather than against nodes. A selector opens a
//! target when the stack below the constraint's scope element spells one of
//! its paths; the target's fields then fill in as the subtree passes, and the
//! tuple is settled when the target closes. `.//` is the only wildcard over
//! depth, which is what keeps that a single index comparison rather than a
//! search.

use crate::names::QName;

/// One step of a path: a name to match, or anything.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Step {
    /// A named element, or `*` for any.
    Element(Option<QName>),
    /// `ns:*` — any name in one namespace.
    AnyIn(Option<crate::names::Namespace>),
}

/// One alternative of a selector or field.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Path {
    /// `.//` — the steps may start at any descendant depth rather than
    /// directly below the context node.
    pub descendant_or_self: bool,
    pub steps: Vec<Step>,
    /// A field may end at an attribute; a selector may not.
    pub attribute: Option<Option<QName>>,
}

/// A parsed selector or field: alternatives, tried in turn.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Paths(pub Vec<Path>);

/// What went wrong, for a diagnostic that quotes the offending text.
#[derive(Debug)]
pub(crate) struct PathError {
    pub reason: String,
}

fn err(reason: impl Into<String>) -> PathError {
    PathError {
        reason: reason.into(),
    }
}

/// How a path's names reach the schema's own tables.
///
/// One object rather than two closures because resolving a prefix reads the
/// interner and interning a local name writes it, and the borrow checker is
/// right to object to holding both at once.
pub(crate) trait PathNames {
    /// The namespace a prefix names in the schema document.
    ///
    /// An unprefixed name in one of these paths is in **no** namespace,
    /// whatever the default declaration says — the specification is explicit,
    /// and it trips people up often enough that XSD 1.1 added
    /// `xpathDefaultNamespace` to override it. So this is only ever asked
    /// about a prefix that was actually written.
    fn namespace(&mut self, prefix: &str) -> Option<crate::names::Namespace>;
    fn intern(&mut self, local: &str) -> crate::names::Symbol;
}

/// Parses a selector or field, resolving prefixes as the schema bound them.
pub(crate) fn parse(
    text: &str,
    is_field: bool,
    names: &mut dyn PathNames,
) -> Result<Paths, PathError> {
    let mut out = Vec::new();
    for alt in text.split('|') {
        out.push(parse_one(alt.trim(), is_field, names)?);
    }
    if out.is_empty() {
        return Err(err("empty path"));
    }
    Ok(Paths(out))
}

fn parse_one(text: &str, is_field: bool, names: &mut dyn PathNames) -> Result<Path, PathError> {
    if text.is_empty() {
        return Err(err("empty path"));
    }
    let (rest, descendant_or_self) = match text.strip_prefix(".//") {
        Some(r) => (r, true),
        None => (text, false),
    };
    if rest.is_empty() {
        return Err(err("`.//` needs a step after it"));
    }

    let mut steps = Vec::new();
    let mut attribute = None;
    let parts: Vec<&str> = rest.split('/').map(str::trim).collect();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        if part.is_empty() {
            return Err(err("an empty step"));
        }
        // `.` is the context node, meaningful only as the whole path or as a
        // leading step.
        if *part == "." {
            if !last || !steps.is_empty() || i > 0 {
                continue;
            }
            continue;
        }
        let attr = part
            .strip_prefix('@')
            .or_else(|| part.strip_prefix("attribute::"));
        if let Some(name) = attr {
            if !is_field {
                return Err(err("a selector may not select an attribute"));
            }
            if !last {
                return Err(err("an attribute can only be the last step"));
            }
            attribute = Some(name_test(name, names)?.0);
            continue;
        }
        let body = part.strip_prefix("child::").unwrap_or(part);
        let (name, any_in) = name_test(body, names)?;
        steps.push(match any_in {
            Some(ns) => Step::AnyIn(ns),
            None => Step::Element(name),
        });
    }
    if is_field && attribute.is_none() && steps.is_empty() {
        // `.` alone: the field is the target node's own content.
    }
    Ok(Path {
        descendant_or_self,
        steps,
        attribute,
    })
}

/// A name test: `*`, `ns:*`, or a QName. Returns the name, and — for `ns:*` —
/// the namespace it wildcards over.
#[allow(clippy::type_complexity)]
fn name_test(
    text: &str,
    names: &mut dyn PathNames,
) -> Result<(Option<QName>, Option<Option<crate::names::Namespace>>), PathError> {
    let text = text.trim();
    if text == "*" {
        return Ok((None, None));
    }
    let (prefix, local) = match text.split_once(':') {
        Some((p, l)) => (Some(p), l),
        None => (None, text),
    };
    if local == "*" {
        let ns = match prefix {
            Some(p) => match names.namespace(p) {
                Some(n) => Some(n),
                None => return Err(err(format!("undeclared prefix `{p}`"))),
            },
            None => None,
        };
        return Ok((None, Some(ns)));
    }
    if !crate::values::is_ncname(local) {
        return Err(err(format!("`{text}` is not a name")));
    }
    let ns = match prefix {
        Some(p) => match names.namespace(p) {
            Some(n) => Some(n),
            None => return Err(err(format!("undeclared prefix `{p}`"))),
        },
        None => None,
    };
    Ok((
        Some(QName {
            ns,
            local: names.intern(local),
        }),
        None,
    ))
}

impl Step {
    fn matches(&self, name: QName) -> bool {
        match self {
            Step::Element(None) => true,
            Step::Element(Some(q)) => *q == name,
            Step::AnyIn(ns) => name.ns == *ns,
        }
    }
}

impl Path {
    /// Whether `names`, the element names below the context node, spell this
    /// path exactly.
    ///
    /// With `.//` the steps may begin at any depth, so this asks whether they
    /// match the *tail*; without it they must start immediately below.
    pub(crate) fn matches(&self, names: &[QName]) -> bool {
        if self.steps.len() > names.len() {
            return false;
        }
        let start = if self.descendant_or_self {
            names.len() - self.steps.len()
        } else if self.steps.len() == names.len() {
            0
        } else {
            return false;
        };
        self.steps
            .iter()
            .zip(&names[start..])
            .all(|(s, n)| s.matches(*n))
    }
}

impl Paths {
    /// Whether any alternative matches.
    pub(crate) fn matches(&self, names: &[QName]) -> bool {
        self.0.iter().any(|p| p.matches(names))
    }

    /// The attribute an alternative would select, when its element steps
    /// match. A field ending in `@name` selects that attribute of the node
    /// its steps reached.
    pub(crate) fn attribute_for(&self, names: &[QName]) -> Option<Option<QName>> {
        self.0
            .iter()
            .find(|p| p.attribute.is_some() && p.matches(names))
            .map(|p| p.attribute.unwrap())
    }
}

// ---------------------------------------------------------------------------
// Matching a document as it streams past
// ---------------------------------------------------------------------------

use crate::model::IdcId;

/// One key tuple.
///
/// Typed values rather than text, because keys compare in the *value* space
/// and text cannot see that: `1.0` and `1.00` are one decimal, and
/// `07:00:00Z` and `12:00:00+05:00` are one instant written from two
/// timezones.
pub(crate) type Key = Vec<Option<crate::values::Value>>;

/// Whether two field values are the same key component.
///
/// Ordered types answer in the value space; for the rest — strings, booleans,
/// QNames, the binaries — structural equality *is* value equality.
pub(crate) fn value_eq(a: &crate::values::Value, b: &crate::values::Value) -> bool {
    use crate::values::Value::List;
    match (a, b) {
        (List(x), List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| value_eq(p, q)),
        // A one-item list and the value it holds are the same key. That is
        // what lets a `keyref` whose field is a list refer to a key whose
        // field is atomic, which the specification allows and real schemas
        // rely on.
        (List(x), y) if x.len() == 1 => value_eq(&x[0], y),
        (x, List(y)) if y.len() == 1 => value_eq(x, &y[0]),
        _ => match a.partial_cmp_value(b) {
            Some(o) => o == std::cmp::Ordering::Equal,
            None => a == b,
        },
    }
}

/// Whether two tuples are the same key.
pub(crate) fn key_eq(a: &Key, b: &Key) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| match (x, y) {
            (Some(x), Some(y)) => value_eq(x, y),
            (None, None) => true,
            _ => false,
        })
}

/// A constraint in force over one element and everything under it.
pub(crate) struct Scope {
    pub constraint: IdcId,
    /// Path length at the element carrying the constraint; a descendant's
    /// path relative to it starts here.
    pub depth: usize,
    /// Tuples seen, for `key` and `unique`.
    pub keys: Vec<Key>,
    /// Tuples that must match some key, for `keyref`.
    pub refs: Vec<(Key, u32)>,
}

/// A node the selector matched, whose fields are still filling in.
pub(crate) struct Target {
    /// Which open scope selected it.
    pub scope: usize,
    /// Path length at the node itself.
    pub depth: usize,
    pub fields: Key,
}

//! The XSD Part 2 datatype system: the built-in type hierarchy and facets.
//!
//! Two rules here are load-bearing and routinely implemented wrong:
//!
//! - **`whiteSpace` applies before lexical parsing, not after.** It is the
//!   only thing distinguishing `xs:token` from `xs:string`, and it is fixed
//!   at `collapse` for every type not descended from
//!   `string`/`normalizedString`. Apply it late and `<v> 42 </v>` fails
//!   against `xs:int`.
//! - **Patterns OR within a restriction step and AND across steps.** See
//!   [`FacetSet::restrict`].

use std::borrow::Cow;
use std::fmt;

/// The `whiteSpace` facet, which normalises the lexical form before parsing.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum WhiteSpace {
    /// Leave the lexical form exactly as written.
    Preserve,
    /// Replace every tab, line feed and carriage return with a space.
    Replace,
    /// `Replace`, then collapse runs of spaces and trim the ends.
    Collapse,
}

impl WhiteSpace {
    /// Applies the facet to a lexical form.
    ///
    /// Borrows unchanged input, so `Preserve` and already-normal values cost
    /// nothing.
    pub fn normalize<'a>(self, s: &'a str) -> Cow<'a, str> {
        match self {
            WhiteSpace::Preserve => Cow::Borrowed(s),
            WhiteSpace::Replace => {
                if s.bytes().any(|b| matches!(b, b'\t' | b'\n' | b'\r')) {
                    Cow::Owned(
                        s.chars()
                            .map(|c| {
                                if matches!(c, '\t' | '\n' | '\r') {
                                    ' '
                                } else {
                                    c
                                }
                            })
                            .collect(),
                    )
                } else {
                    Cow::Borrowed(s)
                }
            }
            WhiteSpace::Collapse => {
                let is_ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r');
                let trimmed = s.trim_matches(is_ws);
                let needs_work = trimmed.len() != s.len()
                    || trimmed.chars().any(|c| matches!(c, '\t' | '\n' | '\r'))
                    || trimmed.contains("  ");
                if !needs_work {
                    return Cow::Borrowed(trimmed);
                }
                let mut out = String::with_capacity(trimmed.len());
                let mut pending_space = false;
                for c in trimmed.chars() {
                    if is_ws(c) {
                        pending_space = true;
                    } else {
                        if pending_space && !out.is_empty() {
                            out.push(' ');
                        }
                        pending_space = false;
                        out.push(c);
                    }
                }
                Cow::Owned(out)
            }
        }
    }
}

impl fmt::Display for WhiteSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            WhiteSpace::Preserve => "preserve",
            WhiteSpace::Replace => "replace",
            WhiteSpace::Collapse => "collapse",
        })
    }
}

/// The `explicitTimezone` facet, new in XSD 1.1.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ExplicitTimezone {
    Optional,
    Required,
    Prohibited,
}

/// How a simple type's value space is constructed.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Variety {
    /// A single value drawn from a primitive's value space.
    Atomic,
    /// A whitespace-separated sequence of an item type's values.
    ///
    /// `length`, `minLength` and `maxLength` then count *items*, not
    /// characters.
    List,
    /// A value drawn from the first member type that accepts it, tried in
    /// declaration order.
    Union,
}

/// The 14 constraining facets of XSD 1.1, as declared at one restriction step.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Facet {
    Length(u64),
    MinLength(u64),
    MaxLength(u64),
    /// One alternative. Several at the same step are ORed together.
    Pattern(String),
    /// One permitted lexical form.
    Enumeration(String),
    WhiteSpace(WhiteSpace),
    MaxInclusive(String),
    MaxExclusive(String),
    MinInclusive(String),
    MinExclusive(String),
    TotalDigits(u32),
    FractionDigits(u32),
    ExplicitTimezone(ExplicitTimezone),
    /// XSD 1.1 `minScale`, for `xs:precisionDecimal`. Signed: a scale of -2
    /// means the value is a multiple of a hundred.
    MinScale(i32),
    /// XSD 1.1 `maxScale`, for `xs:precisionDecimal`.
    MaxScale(i32),
    /// An XPath 2.0 expression, stored unevaluated until XSD 1.1 lands.
    Assertion(String),
}

/// Which facet, without its value.
///
/// `Facet` carries a value and so cannot be compared or tabulated; the rules
/// about *which* facets a datatype admits are about the kind alone.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum FacetKind {
    Length,
    MinLength,
    MaxLength,
    Pattern,
    Enumeration,
    WhiteSpace,
    MaxInclusive,
    MaxExclusive,
    MinInclusive,
    MinExclusive,
    TotalDigits,
    FractionDigits,
    ExplicitTimezone,
    MinScale,
    MaxScale,
    Assertion,
}

impl FacetKind {
    /// The facet's XSD element name, e.g. `minInclusive`.
    pub fn name(self) -> &'static str {
        match self {
            FacetKind::Length => "length",
            FacetKind::MinLength => "minLength",
            FacetKind::MaxLength => "maxLength",
            FacetKind::Pattern => "pattern",
            FacetKind::Enumeration => "enumeration",
            FacetKind::WhiteSpace => "whiteSpace",
            FacetKind::MaxInclusive => "maxInclusive",
            FacetKind::MaxExclusive => "maxExclusive",
            FacetKind::MinInclusive => "minInclusive",
            FacetKind::MinExclusive => "minExclusive",
            FacetKind::TotalDigits => "totalDigits",
            FacetKind::FractionDigits => "fractionDigits",
            FacetKind::ExplicitTimezone => "explicitTimezone",
            FacetKind::MinScale => "minScale",
            FacetKind::MaxScale => "maxScale",
            FacetKind::Assertion => "assertion",
        }
    }

    /// Whether this facet bounds the value space rather than the lexical one.
    ///
    /// The four of them share a rule: the facet's own value has to be a legal
    /// value of the type being restricted.
    pub fn is_bound(self) -> bool {
        matches!(
            self,
            FacetKind::MaxInclusive
                | FacetKind::MaxExclusive
                | FacetKind::MinInclusive
                | FacetKind::MinExclusive
        )
    }
}

impl fmt::Display for FacetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Facet {
    /// The facet's XSD element name, e.g. `minInclusive`.
    pub fn name(&self) -> &'static str {
        self.kind().name()
    }

    /// Which facet this is, discarding the value.
    pub fn kind(&self) -> FacetKind {
        match self {
            Facet::Length(_) => FacetKind::Length,
            Facet::MinLength(_) => FacetKind::MinLength,
            Facet::MaxLength(_) => FacetKind::MaxLength,
            Facet::Pattern(_) => FacetKind::Pattern,
            Facet::Enumeration(_) => FacetKind::Enumeration,
            Facet::WhiteSpace(_) => FacetKind::WhiteSpace,
            Facet::MaxInclusive(_) => FacetKind::MaxInclusive,
            Facet::MaxExclusive(_) => FacetKind::MaxExclusive,
            Facet::MinInclusive(_) => FacetKind::MinInclusive,
            Facet::MinExclusive(_) => FacetKind::MinExclusive,
            Facet::TotalDigits(_) => FacetKind::TotalDigits,
            Facet::FractionDigits(_) => FacetKind::FractionDigits,
            Facet::ExplicitTimezone(_) => FacetKind::ExplicitTimezone,
            Facet::MinScale(_) => FacetKind::MinScale,
            Facet::MaxScale(_) => FacetKind::MaxScale,
            Facet::Assertion(_) => FacetKind::Assertion,
        }
    }
}

/// The facets in force on a simple type, after composing its whole
/// restriction chain.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct FacetSet {
    pub length: Option<u64>,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    /// Outer: one entry per restriction step, **ANDed** — a value must match
    /// every step. Inner: alternatives declared at that step, **ORed**.
    pub patterns: Vec<Vec<String>>,
    /// The most-derived enumeration. A restriction may only narrow, so the
    /// innermost set is the effective one.
    pub enumeration: Option<Vec<String>>,
    /// The namespace bindings in scope where [`Self::enumeration`] was
    /// written, for the prefixes its literals actually use.
    ///
    /// `xs:QName` and `xs:NOTATION` are the only datatypes whose value is not
    /// a function of the lexical form alone, and an enumeration literal binds
    /// its prefix in the **schema** document — not in the instance being
    /// validated, which may spell the same namespace differently or not
    /// declare it at all. Nothing downstream can recover that, so the loader
    /// captures it here. Empty for every other datatype.
    pub namespaces: Vec<(Option<String>, String)>,
    pub white_space: Option<WhiteSpace>,
    pub max_inclusive: Option<String>,
    pub max_exclusive: Option<String>,
    pub min_inclusive: Option<String>,
    pub min_exclusive: Option<String>,
    pub total_digits: Option<u32>,
    pub fraction_digits: Option<u32>,
    pub explicit_timezone: Option<ExplicitTimezone>,
    /// XSD 1.1 scale bounds, for `xs:precisionDecimal`.
    pub min_scale: Option<i32>,
    pub max_scale: Option<i32>,
    pub assertions: Vec<String>,
}

impl FacetSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Composes one restriction step onto the inherited set, following the
    /// XSD facet-composition rules.
    ///
    /// Patterns declared at this step are ORed with each other and ANDed with
    /// every inherited step. Every other facet overrides the inherited value,
    /// which is what "a restriction may only narrow" reduces to once the
    /// schema has been checked.
    pub fn restrict(&self, step: &[Facet]) -> FacetSet {
        let mut out = self.clone();
        let mut step_patterns = Vec::new();

        for f in step {
            match f {
                Facet::Length(v) => out.length = Some(*v),
                Facet::MinLength(v) => out.min_length = Some(*v),
                Facet::MaxLength(v) => out.max_length = Some(*v),
                Facet::Pattern(p) => step_patterns.push(p.clone()),
                Facet::Enumeration(_) => {}
                Facet::WhiteSpace(w) => out.white_space = Some(*w),
                Facet::MaxInclusive(v) => {
                    out.max_inclusive = Some(v.clone());
                    out.max_exclusive = None;
                }
                Facet::MaxExclusive(v) => {
                    out.max_exclusive = Some(v.clone());
                    out.max_inclusive = None;
                }
                Facet::MinInclusive(v) => {
                    out.min_inclusive = Some(v.clone());
                    out.min_exclusive = None;
                }
                Facet::MinExclusive(v) => {
                    out.min_exclusive = Some(v.clone());
                    out.min_inclusive = None;
                }
                Facet::TotalDigits(v) => out.total_digits = Some(*v),
                Facet::FractionDigits(v) => out.fraction_digits = Some(*v),
                Facet::ExplicitTimezone(v) => out.explicit_timezone = Some(*v),
                Facet::MinScale(v) => out.min_scale = Some(*v),
                Facet::MaxScale(v) => out.max_scale = Some(*v),
                Facet::Assertion(a) => out.assertions.push(a.clone()),
            }
        }

        if !step_patterns.is_empty() {
            out.patterns.push(step_patterns);
        }

        let enums: Vec<String> = step
            .iter()
            .filter_map(|f| match f {
                Facet::Enumeration(v) => Some(v.clone()),
                _ => None,
            })
            .collect();
        if !enums.is_empty() {
            out.enumeration = Some(enums);
        }

        out
    }

    /// The effective `whiteSpace`, defaulting to the value the type's
    /// built-in ancestry fixes.
    pub fn effective_white_space(&self, builtin_default: WhiteSpace) -> WhiteSpace {
        self.white_space.unwrap_or(builtin_default)
    }
}

/// Every built-in type defined by XSD 1.1 Part 2.
///
/// 19 primitives, 2 special types, the complex ur-type, and the ordinary
/// derived types — including the three that are new in 1.1
/// (`yearMonthDuration`, `dayTimeDuration`, `dateTimeStamp`).
///
/// `precisionDecimal` is deliberately absent: it appeared in drafts and did
/// not make the final Recommendation.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Builtin {
    // ur-types
    AnyType,
    AnySimpleType,
    AnyAtomicType,

    // 19 primitives
    String,
    Boolean,
    Decimal,
    Float,
    Double,
    Duration,
    DateTime,
    Time,
    Date,
    GYearMonth,
    GYear,
    GMonthDay,
    GDay,
    GMonth,
    HexBinary,
    Base64Binary,
    AnyUri,
    QName,
    Notation,

    // derived — string branch
    NormalizedString,
    Token,
    Language,
    NmToken,
    NmTokens,
    Name,
    NcName,
    Id,
    IdRef,
    IdRefs,
    Entity,
    Entities,

    // derived — decimal branch
    Integer,
    NonPositiveInteger,
    NegativeInteger,
    Long,
    Int,
    Short,
    Byte,
    NonNegativeInteger,
    UnsignedLong,
    UnsignedInt,
    UnsignedShort,
    UnsignedByte,
    PositiveInteger,

    // derived — new in XSD 1.1
    YearMonthDuration,
    DayTimeDuration,
    DateTimeStamp,

    /// XSD 1.1's optional decimal that remembers its scale. A primitive in its
    /// own right, not a decimal: it has infinities, a NaN and a signed zero,
    /// none of which `xs:decimal` has.
    ///
    /// Last, because `BUILTINS` is indexed by `Builtin as usize` and the two
    /// orders have to agree — `table_and_enum_agree` pins that.
    PrecisionDecimal,
}

/// What kind of built-in a [`Builtin`] is.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum BuiltinKind {
    /// `xs:anyType` — the complex ur-type.
    Complex,
    /// `xs:anySimpleType` — has no variety of its own.
    AnySimple,
    Atomic,
    /// A list, with its item type.
    List(Builtin),
}

impl Builtin {
    /// The local name in the XML Schema namespace, e.g. `nonNegativeInteger`.
    pub fn local_name(self) -> &'static str {
        BUILTINS[self as usize].0
    }

    /// The type this one is derived from, absent only for `xs:anyType`.
    pub fn base(self) -> Option<Builtin> {
        BUILTINS[self as usize].1
    }

    pub fn kind(self) -> BuiltinKind {
        BUILTINS[self as usize].2
    }

    /// The `whiteSpace` value fixed by this type's built-in ancestry.
    pub fn white_space(self) -> WhiteSpace {
        BUILTINS[self as usize].3
    }

    /// Whether this is one of the 19 primitive datatypes.
    pub fn is_primitive(self) -> bool {
        matches!(self.base(), Some(Builtin::AnyAtomicType))
    }

    /// The primitive this type reduces to, if any.
    ///
    /// Returns `None` for the ur-types and for list types, which have no
    /// primitive of their own.
    pub fn primitive(self) -> Option<Builtin> {
        let mut cur = self;
        loop {
            if cur.is_primitive() {
                return Some(cur);
            }
            match cur.kind() {
                BuiltinKind::Atomic => cur = cur.base()?,
                _ => return None,
            }
        }
    }

    /// The variety of this type's value space.
    pub fn variety(self) -> Option<Variety> {
        match self.kind() {
            BuiltinKind::Complex | BuiltinKind::AnySimple => None,
            BuiltinKind::Atomic => Some(Variety::Atomic),
            BuiltinKind::List(_) => Some(Variety::List),
        }
    }

    /// Whether `facet` may constrain this datatype.
    ///
    /// From the per-datatype "Constraining facets" lists in the datatypes
    /// specification. The rule is about the *primitive*, not the type itself:
    /// `xs:yearMonthDuration` derives from `xs:duration`, so `length` is no
    /// more applicable to it than to a duration, even though it reads like a
    /// measure of one.
    ///
    /// The ur-types admit everything. Nothing may be derived from them by
    /// restriction anyway, so the question is moot there, and answering
    /// `false` would turn a schema this crate cannot place into a schema it
    /// rejects.
    pub fn allows_facet(self, facet: FacetKind) -> bool {
        use FacetKind as F;
        // Applies to every simple type that admits any facet at all.
        let universal = matches!(facet, F::Pattern | F::WhiteSpace | F::Assertion);
        // Counts characters, or items for a list.
        let sized = matches!(facet, F::Length | F::MinLength | F::MaxLength);

        match self.kind() {
            BuiltinKind::Complex | BuiltinKind::AnySimple => true,
            BuiltinKind::List(_) => universal || sized || facet == F::Enumeration,
            BuiltinKind::Atomic => {
                let Some(p) = self.primitive() else {
                    // xs:anyAtomicType, which is not restrictable either.
                    return true;
                };
                if universal {
                    return true;
                }
                match p {
                    // Boolean has no enumeration: with two values, enumerating
                    // them either says nothing or contradicts the type.
                    B::Boolean => false,
                    B::String
                    | B::HexBinary
                    | B::Base64Binary
                    | B::AnyUri
                    | B::QName
                    | B::Notation => sized || facet == F::Enumeration,
                    B::Decimal => {
                        facet.is_bound()
                            || matches!(facet, F::TotalDigits | F::FractionDigits | F::Enumeration)
                    }
                    // Scale in place of `fractionDigits`: the scale may be
                    // negative, which a count of fractional digits cannot be.
                    B::PrecisionDecimal => {
                        facet.is_bound()
                            || matches!(
                                facet,
                                F::TotalDigits | F::MinScale | F::MaxScale | F::Enumeration
                            )
                    }
                    B::Float | B::Double | B::Duration => {
                        facet.is_bound() || facet == F::Enumeration
                    }
                    B::DateTime
                    | B::Time
                    | B::Date
                    | B::GYearMonth
                    | B::GYear
                    | B::GMonthDay
                    | B::GDay
                    | B::GMonth => {
                        facet.is_bound() || matches!(facet, F::Enumeration | F::ExplicitTimezone)
                    }
                    _ => unreachable!("primitive() returned a non-primitive: {p:?}"),
                }
            }
        }
    }

    /// Whether `self` is `other`, or is derived from it.
    pub fn derives_from(self, other: Builtin) -> bool {
        let mut cur = Some(self);
        while let Some(c) = cur {
            if c == other {
                return true;
            }
            cur = c.base();
        }
        false
    }

    /// Looks a built-in up by its local name in the XML Schema namespace.
    pub fn from_local_name(name: &str) -> Option<Builtin> {
        BUILTINS
            .iter()
            .position(|e| e.0 == name)
            .map(|i| ALL_BUILTINS[i])
    }

    /// Every built-in, in declaration order.
    pub fn all() -> &'static [Builtin] {
        &ALL_BUILTINS
    }
}

impl fmt::Display for Builtin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "xs:{}", self.local_name())
    }
}

type Entry = (&'static str, Option<Builtin>, BuiltinKind, WhiteSpace);

use Builtin as B;
use BuiltinKind as K;
use WhiteSpace as W;

/// The built-in table, indexed by `Builtin as usize`.
///
/// Order must match the `Builtin` enum exactly; `ALL_BUILTINS` below pins
/// that with a test.
static BUILTINS: &[Entry] = &[
    ("anyType", None, K::Complex, W::Preserve),
    ("anySimpleType", Some(B::AnyType), K::AnySimple, W::Preserve),
    (
        "anyAtomicType",
        Some(B::AnySimpleType),
        K::Atomic,
        W::Preserve,
    ),
    ("string", Some(B::AnyAtomicType), K::Atomic, W::Preserve),
    ("boolean", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("decimal", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("float", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("double", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("duration", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("dateTime", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("time", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("date", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("gYearMonth", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("gYear", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("gMonthDay", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("gDay", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("gMonth", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("hexBinary", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    (
        "base64Binary",
        Some(B::AnyAtomicType),
        K::Atomic,
        W::Collapse,
    ),
    ("anyURI", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("QName", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("NOTATION", Some(B::AnyAtomicType), K::Atomic, W::Collapse),
    ("normalizedString", Some(B::String), K::Atomic, W::Replace),
    ("token", Some(B::NormalizedString), K::Atomic, W::Collapse),
    ("language", Some(B::Token), K::Atomic, W::Collapse),
    ("NMTOKEN", Some(B::Token), K::Atomic, W::Collapse),
    (
        "NMTOKENS",
        Some(B::AnySimpleType),
        K::List(B::NmToken),
        W::Collapse,
    ),
    ("Name", Some(B::Token), K::Atomic, W::Collapse),
    ("NCName", Some(B::Name), K::Atomic, W::Collapse),
    ("ID", Some(B::NcName), K::Atomic, W::Collapse),
    ("IDREF", Some(B::NcName), K::Atomic, W::Collapse),
    (
        "IDREFS",
        Some(B::AnySimpleType),
        K::List(B::IdRef),
        W::Collapse,
    ),
    ("ENTITY", Some(B::NcName), K::Atomic, W::Collapse),
    (
        "ENTITIES",
        Some(B::AnySimpleType),
        K::List(B::Entity),
        W::Collapse,
    ),
    ("integer", Some(B::Decimal), K::Atomic, W::Collapse),
    (
        "nonPositiveInteger",
        Some(B::Integer),
        K::Atomic,
        W::Collapse,
    ),
    (
        "negativeInteger",
        Some(B::NonPositiveInteger),
        K::Atomic,
        W::Collapse,
    ),
    ("long", Some(B::Integer), K::Atomic, W::Collapse),
    ("int", Some(B::Long), K::Atomic, W::Collapse),
    ("short", Some(B::Int), K::Atomic, W::Collapse),
    ("byte", Some(B::Short), K::Atomic, W::Collapse),
    (
        "nonNegativeInteger",
        Some(B::Integer),
        K::Atomic,
        W::Collapse,
    ),
    (
        "unsignedLong",
        Some(B::NonNegativeInteger),
        K::Atomic,
        W::Collapse,
    ),
    ("unsignedInt", Some(B::UnsignedLong), K::Atomic, W::Collapse),
    (
        "unsignedShort",
        Some(B::UnsignedInt),
        K::Atomic,
        W::Collapse,
    ),
    (
        "unsignedByte",
        Some(B::UnsignedShort),
        K::Atomic,
        W::Collapse,
    ),
    (
        "positiveInteger",
        Some(B::NonNegativeInteger),
        K::Atomic,
        W::Collapse,
    ),
    (
        "yearMonthDuration",
        Some(B::Duration),
        K::Atomic,
        W::Collapse,
    ),
    ("dayTimeDuration", Some(B::Duration), K::Atomic, W::Collapse),
    ("dateTimeStamp", Some(B::DateTime), K::Atomic, W::Collapse),
    (
        "precisionDecimal",
        Some(B::AnyAtomicType),
        K::Atomic,
        W::Collapse,
    ),
];

static ALL_BUILTINS: [Builtin; 51] = [
    B::AnyType,
    B::AnySimpleType,
    B::AnyAtomicType,
    B::String,
    B::Boolean,
    B::Decimal,
    B::Float,
    B::Double,
    B::Duration,
    B::DateTime,
    B::Time,
    B::Date,
    B::GYearMonth,
    B::GYear,
    B::GMonthDay,
    B::GDay,
    B::GMonth,
    B::HexBinary,
    B::Base64Binary,
    B::AnyUri,
    B::QName,
    B::Notation,
    B::NormalizedString,
    B::Token,
    B::Language,
    B::NmToken,
    B::NmTokens,
    B::Name,
    B::NcName,
    B::Id,
    B::IdRef,
    B::IdRefs,
    B::Entity,
    B::Entities,
    B::Integer,
    B::NonPositiveInteger,
    B::NegativeInteger,
    B::Long,
    B::Int,
    B::Short,
    B::Byte,
    B::NonNegativeInteger,
    B::UnsignedLong,
    B::UnsignedInt,
    B::UnsignedShort,
    B::UnsignedByte,
    B::PositiveInteger,
    B::YearMonthDuration,
    B::DayTimeDuration,
    B::DateTimeStamp,
    B::PrecisionDecimal,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_and_enum_agree() {
        assert_eq!(BUILTINS.len(), ALL_BUILTINS.len());
        for (i, &b) in ALL_BUILTINS.iter().enumerate() {
            assert_eq!(b as usize, i, "{b:?} is out of order in ALL_BUILTINS");
        }
    }

    #[test]
    fn there_are_nineteen_primitives_plus_the_optional_one() {
        let n = Builtin::all().iter().filter(|b| b.is_primitive()).count();
        assert_eq!(
            n, 20,
            "XSD 1.1 Part 2 defines 19 required primitives, and `precisionDecimal` \
             is the optional twentieth"
        );
        assert!(Builtin::PrecisionDecimal.is_primitive());
    }

    /// `precisionDecimal` is optional in XSD 1.1 — a conforming processor need
    /// not have it, and the suite's `vc:typeAvailable` exists partly to ask.
    /// This one does, so the answer is yes.
    #[test]
    fn precision_decimal_is_available() {
        assert_eq!(
            Builtin::from_local_name("precisionDecimal"),
            Some(Builtin::PrecisionDecimal)
        );
        // A primitive in its own right, not a decimal: it has infinities, a
        // NaN and a signed zero, none of which `xs:decimal` has.
        assert_eq!(
            Builtin::PrecisionDecimal.primitive(),
            Some(Builtin::PrecisionDecimal)
        );
        assert!(!Builtin::PrecisionDecimal.derives_from(Builtin::Decimal));
    }

    #[test]
    fn lookup_round_trips() {
        for &b in Builtin::all() {
            assert_eq!(Builtin::from_local_name(b.local_name()), Some(b));
        }
    }

    #[test]
    fn derivation_chains_reach_the_ur_type() {
        for &b in Builtin::all() {
            assert!(
                b.derives_from(Builtin::AnyType),
                "{b:?} does not reach xs:anyType"
            );
        }
        assert!(Builtin::PositiveInteger.derives_from(Builtin::Decimal));
        assert!(Builtin::PositiveInteger.derives_from(Builtin::Integer));
        assert!(!Builtin::PositiveInteger.derives_from(Builtin::NegativeInteger));
    }

    #[test]
    fn primitives_resolve_through_long_chains() {
        assert_eq!(Builtin::UnsignedByte.primitive(), Some(Builtin::Decimal));
        assert_eq!(Builtin::Id.primitive(), Some(Builtin::String));
        assert_eq!(Builtin::DateTimeStamp.primitive(), Some(Builtin::DateTime));
        // list types have no primitive of their own
        assert_eq!(Builtin::NmTokens.primitive(), None);
    }

    #[test]
    fn list_types_are_lists_of_the_right_item() {
        assert_eq!(
            Builtin::NmTokens.kind(),
            BuiltinKind::List(Builtin::NmToken)
        );
        assert_eq!(Builtin::IdRefs.kind(), BuiltinKind::List(Builtin::IdRef));
        assert_eq!(Builtin::Entities.kind(), BuiltinKind::List(Builtin::Entity));
        assert_eq!(Builtin::NmTokens.variety(), Some(Variety::List));
    }

    #[test]
    fn whitespace_is_fixed_by_ancestry() {
        assert_eq!(Builtin::String.white_space(), WhiteSpace::Preserve);
        assert_eq!(Builtin::NormalizedString.white_space(), WhiteSpace::Replace);
        assert_eq!(Builtin::Token.white_space(), WhiteSpace::Collapse);
        assert_eq!(Builtin::Language.white_space(), WhiteSpace::Collapse);
        assert_eq!(Builtin::Int.white_space(), WhiteSpace::Collapse);
    }

    #[test]
    fn whitespace_normalization() {
        assert_eq!(WhiteSpace::Preserve.normalize(" a\tb "), " a\tb ");
        assert_eq!(WhiteSpace::Replace.normalize(" a\tb\n"), " a b ");
        assert_eq!(WhiteSpace::Collapse.normalize(" a\t\n b  c "), "a b c");
        assert_eq!(WhiteSpace::Collapse.normalize("  "), "");
        assert_eq!(WhiteSpace::Collapse.normalize("plain"), "plain");
    }

    /// The reason `<v> 42 </v>` must parse as `xs:int`: collapse runs first.
    #[test]
    fn collapse_runs_before_lexical_parsing() {
        let raw = " 42 ";
        let ws = Builtin::Int.white_space();
        assert_eq!(ws.normalize(raw), "42");
        assert_eq!(Builtin::String.white_space().normalize(raw), " 42 ");
    }

    #[test]
    fn patterns_or_within_a_step_and_and_across_steps() {
        let base = FacetSet::new();
        let step1 = base.restrict(&[
            Facet::Pattern("[a-z]+".into()),
            Facet::Pattern("[0-9]+".into()),
        ]);
        assert_eq!(
            step1.patterns,
            vec![vec!["[a-z]+".to_string(), "[0-9]+".to_string()]]
        );

        let step2 = step1.restrict(&[Facet::Pattern(".{3}".into())]);
        assert_eq!(
            step2.patterns.len(),
            2,
            "each step is a separate ANDed group"
        );
        assert_eq!(step2.patterns[1], vec![".{3}".to_string()]);
    }

    #[test]
    fn a_step_without_patterns_adds_no_group() {
        let f = FacetSet::new().restrict(&[Facet::MaxLength(4)]);
        assert!(f.patterns.is_empty());
        assert_eq!(f.max_length, Some(4));
    }

    #[test]
    fn inclusive_and_exclusive_bounds_displace_each_other() {
        let f = FacetSet::new()
            .restrict(&[Facet::MinInclusive("0".into())])
            .restrict(&[Facet::MinExclusive("5".into())]);
        assert_eq!(f.min_exclusive.as_deref(), Some("5"));
        assert_eq!(
            f.min_inclusive, None,
            "a bound of the other kind must not linger"
        );
    }

    #[test]
    fn the_innermost_enumeration_wins() {
        let f = FacetSet::new()
            .restrict(&[
                Facet::Enumeration("a".into()),
                Facet::Enumeration("b".into()),
            ])
            .restrict(&[Facet::Enumeration("a".into())]);
        assert_eq!(f.enumeration, Some(vec!["a".to_string()]));
    }

    #[test]
    fn effective_whitespace_falls_back_to_the_builtin() {
        let f = FacetSet::new();
        assert_eq!(
            f.effective_white_space(WhiteSpace::Collapse),
            WhiteSpace::Collapse
        );
        let f = f.restrict(&[Facet::WhiteSpace(WhiteSpace::Preserve)]);
        assert_eq!(
            f.effective_white_space(WhiteSpace::Collapse),
            WhiteSpace::Preserve
        );
    }
}

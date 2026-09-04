//! The XSD value space: lexical forms in, typed values out.
//!
//! Two rules govern every entry point here, and both are easy to get subtly
//! wrong:
//!
//! - **`whiteSpace` is applied before parsing, never after.** [`parse`] does
//!   it itself, from the built-in's own facet, so a caller cannot get the
//!   order wrong. It is the whole reason `<v> 42 </v>` is a valid `xs:int`.
//! - **Facets constrain the *value* space, not the lexical one.** `1.0` and
//!   `1.00` are the same `xs:decimal`, so an `enumeration` listing one admits
//!   the other. Comparing strings would reject it.
//!
//! The numeric and temporal families come from `oxsdatatypes`, which already
//! carries the awkward parts — arbitrary-scale decimals, timezone-aware
//! comparison, the two duration subtypes — and is exercised hard inside
//! Oxigraph.

use crate::datatypes::{Builtin, BuiltinKind, FacetSet};
use crate::names::QName;
use oxsdatatypes::{
    Date, DateTime, DayTimeDuration, Decimal, Double, Duration, Float, GDay, GMonth, GMonthDay,
    GYear, GYearMonth, Time, YearMonthDuration,
};
use std::fmt;
use std::str::FromStr;

/// A value in an XSD value space.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// `xs:string` and every type derived from it.
    String(String),
    Boolean(bool),
    Decimal(Decimal),
    /// The `xs:integer` chain, held as `i128`.
    ///
    /// That covers every *bounded* XSD integer type exactly, `unsignedLong`
    /// included. `xs:integer` itself is unbounded in the specification;
    /// values beyond `i128` are rejected rather than silently truncated.
    Integer(i128),
    Float(Float),
    Double(Double),
    Duration(Duration),
    YearMonthDuration(YearMonthDuration),
    DayTimeDuration(DayTimeDuration),
    DateTime(DateTime),
    Time(Time),
    Date(Date),
    GYearMonth(GYearMonth),
    GYear(GYear),
    GMonthDay(GMonthDay),
    GDay(GDay),
    GMonth(GMonth),
    HexBinary(Vec<u8>),
    Base64Binary(Vec<u8>),
    AnyUri(String),
    QName(QName),
    /// A list variety's items.
    List(Vec<Value>),
}

impl Value {
    /// The value's length as the `length` family of facets counts it:
    /// characters for strings, octets for binary, items for a list.
    ///
    /// `None` for types the facets do not apply to.
    pub fn facet_length(&self) -> Option<usize> {
        match self {
            Value::String(s) | Value::AnyUri(s) => Some(s.chars().count()),
            Value::HexBinary(b) | Value::Base64Binary(b) => Some(b.len()),
            Value::List(items) => Some(items.len()),
            Value::QName(_) => None,
            _ => None,
        }
    }

    /// Whether two values are ordered relative to one another.
    ///
    /// Partial because `xs:duration` genuinely is: `P1M` and `P30D` cannot be
    /// ordered without knowing which month.
    pub fn partial_cmp_value(&self, other: &Value) -> Option<std::cmp::Ordering> {
        use Value::*;
        match (self, other) {
            (Decimal(a), Decimal(b)) => a.partial_cmp(b),
            (Integer(a), Integer(b)) => a.partial_cmp(b),
            (Decimal(a), Integer(b)) => a.partial_cmp(&(*b).try_into().ok()?),
            (Integer(a), Decimal(b)) => <oxsdatatypes::Decimal>::try_from(*a).ok()?.partial_cmp(b),
            (Float(a), Float(b)) => a.partial_cmp(b),
            (Double(a), Double(b)) => a.partial_cmp(b),
            (DateTime(a), DateTime(b)) => a.partial_cmp(b),
            (Date(a), Date(b)) => a.partial_cmp(b),
            (Time(a), Time(b)) => a.partial_cmp(b),
            (GYear(a), GYear(b)) => a.partial_cmp(b),
            (GYearMonth(a), GYearMonth(b)) => a.partial_cmp(b),
            (GMonthDay(a), GMonthDay(b)) => a.partial_cmp(b),
            (GDay(a), GDay(b)) => a.partial_cmp(b),
            (GMonth(a), GMonth(b)) => a.partial_cmp(b),
            (YearMonthDuration(a), YearMonthDuration(b)) => a.partial_cmp(b),
            (DayTimeDuration(a), DayTimeDuration(b)) => a.partial_cmp(b),
            (Duration(a), Duration(b)) => a.partial_cmp(b),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::String(s) | Value::AnyUri(s) => f.write_str(s),
            Value::Boolean(b) => write!(f, "{b}"),
            Value::Decimal(v) => write!(f, "{v}"),
            Value::Integer(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Double(v) => write!(f, "{v}"),
            Value::Duration(v) => write!(f, "{v}"),
            Value::YearMonthDuration(v) => write!(f, "{v}"),
            Value::DayTimeDuration(v) => write!(f, "{v}"),
            Value::DateTime(v) => write!(f, "{v}"),
            Value::Time(v) => write!(f, "{v}"),
            Value::Date(v) => write!(f, "{v}"),
            Value::GYearMonth(v) => write!(f, "{v}"),
            Value::GYear(v) => write!(f, "{v}"),
            Value::GMonthDay(v) => write!(f, "{v}"),
            Value::GDay(v) => write!(f, "{v}"),
            Value::GMonth(v) => write!(f, "{v}"),
            Value::HexBinary(b) => {
                for byte in b {
                    write!(f, "{byte:02X}")?;
                }
                Ok(())
            }
            Value::Base64Binary(b) => f.write_str(&base64_encode(b)),
            Value::QName(_) => f.write_str("<QName>"),
            Value::List(items) => {
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" ")?;
                    }
                    write!(f, "{v}")?;
                }
                Ok(())
            }
        }
    }
}

/// Why a lexical form is not a value of its type.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ValueError {
    pub builtin: Builtin,
    pub lexical: String,
    pub reason: String,
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is not a valid {}: {}",
            self.lexical, self.builtin, self.reason
        )
    }
}

impl std::error::Error for ValueError {}

fn err(builtin: Builtin, lexical: &str, reason: impl Into<String>) -> ValueError {
    ValueError {
        builtin,
        lexical: lexical.to_string(),
        reason: reason.into(),
    }
}

/// Parses a lexical form into the value space of a built-in datatype.
///
/// Applies the type's `whiteSpace` facet first — that ordering is the whole
/// difference between `xs:string` and `xs:token`, and between a valid and an
/// invalid `xs:int`.
pub fn parse(builtin: Builtin, lexical: &str) -> Result<Value, ValueError> {
    let normalized = builtin.white_space().normalize(lexical);
    let s = normalized.as_ref();
    parse_normalized(builtin, s, lexical)
}

/// Parses an already-whitespace-normalised lexical form.
///
/// `raw` is carried only so errors quote what the document actually said.
fn parse_normalized(builtin: Builtin, s: &str, raw: &str) -> Result<Value, ValueError> {
    use Builtin as B;

    // A list type's value is its items', so the item type does the work.
    if let BuiltinKind::List(item) = builtin.kind() {
        let items = s
            .split_whitespace()
            .map(|tok| parse(item, tok))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Value::List(items));
    }

    let bad = |reason: &str| err(builtin, raw, reason);

    Ok(match builtin {
        B::String | B::NormalizedString | B::Token => Value::String(s.to_string()),

        B::Language => {
            if !is_language(s) {
                return Err(bad("expected a language tag such as `en-GB`"));
            }
            Value::String(s.to_string())
        }
        B::NmToken => {
            if s.is_empty() || !s.chars().all(is_name_char) {
                return Err(bad("expected one or more XML name characters"));
            }
            Value::String(s.to_string())
        }
        B::Name => {
            if !is_xml_name(s) {
                return Err(bad("expected an XML Name"));
            }
            Value::String(s.to_string())
        }
        B::NcName | B::Id | B::IdRef | B::Entity => {
            if !is_ncname(s) {
                return Err(bad("expected an XML NCName (a Name with no colon)"));
            }
            Value::String(s.to_string())
        }

        B::Boolean => match s {
            "true" | "1" => Value::Boolean(true),
            "false" | "0" => Value::Boolean(false),
            // Deliberately strict: XSD admits exactly these four spellings.
            _ => return Err(bad("expected `true`, `false`, `1` or `0`")),
        },

        B::Decimal => Value::Decimal(Decimal::from_str(s).map_err(|e| bad(&e.to_string()))?),

        B::Integer
        | B::NonPositiveInteger
        | B::NegativeInteger
        | B::Long
        | B::Int
        | B::Short
        | B::Byte
        | B::NonNegativeInteger
        | B::UnsignedLong
        | B::UnsignedInt
        | B::UnsignedShort
        | B::UnsignedByte
        | B::PositiveInteger => {
            let n = parse_integer(s).ok_or_else(|| bad("expected an integer"))?;
            let (lo, hi) = integer_bounds(builtin);
            if n < lo || n > hi {
                return Err(bad(&format!(
                    "outside the range of {builtin} ({lo}..={hi})"
                )));
            }
            Value::Integer(n)
        }

        B::Float => Value::Float(Float::from_str(s).map_err(|e| bad(&e.to_string()))?),
        B::Double => Value::Double(Double::from_str(s).map_err(|e| bad(&e.to_string()))?),

        B::Duration => Value::Duration(Duration::from_str(s).map_err(|e| bad(&e.to_string()))?),
        B::YearMonthDuration => Value::YearMonthDuration(
            YearMonthDuration::from_str(s).map_err(|e| bad(&e.to_string()))?,
        ),
        B::DayTimeDuration => {
            Value::DayTimeDuration(DayTimeDuration::from_str(s).map_err(|e| bad(&e.to_string()))?)
        }

        B::DateTime => Value::DateTime(DateTime::from_str(s).map_err(|e| bad(&e.to_string()))?),
        B::DateTimeStamp => {
            let v = DateTime::from_str(s).map_err(|e| bad(&e.to_string()))?;
            if v.timezone_offset().is_none() {
                // The one thing dateTimeStamp adds over dateTime.
                return Err(bad("a timezone is required by xs:dateTimeStamp"));
            }
            Value::DateTime(v)
        }
        B::Time => Value::Time(Time::from_str(s).map_err(|e| bad(&e.to_string()))?),
        B::Date => Value::Date(Date::from_str(s).map_err(|e| bad(&e.to_string()))?),
        B::GYearMonth => {
            Value::GYearMonth(GYearMonth::from_str(s).map_err(|e| bad(&e.to_string()))?)
        }
        B::GYear => Value::GYear(GYear::from_str(s).map_err(|e| bad(&e.to_string()))?),
        B::GMonthDay => Value::GMonthDay(GMonthDay::from_str(s).map_err(|e| bad(&e.to_string()))?),
        B::GDay => Value::GDay(GDay::from_str(s).map_err(|e| bad(&e.to_string()))?),
        B::GMonth => Value::GMonth(GMonth::from_str(s).map_err(|e| bad(&e.to_string()))?),

        B::HexBinary => Value::HexBinary(
            hex_decode(s).ok_or_else(|| bad("expected an even number of hexadecimal digits"))?,
        ),
        B::Base64Binary => Value::Base64Binary(
            base64_decode(s).ok_or_else(|| bad("expected base64-encoded data"))?,
        ),

        B::AnyUri => Value::AnyUri(s.to_string()),

        // A QName needs the in-scope namespace bindings, which live in the
        // document rather than the type. The validator resolves these.
        B::QName | B::Notation => {
            return Err(bad(
                "QName values must be resolved against the document's namespaces",
            ));
        }

        // The ur-types accept anything; `anyType` is not a simple type at all.
        B::AnyType | B::AnySimpleType | B::AnyAtomicType => Value::String(s.to_string()),

        // Reached only via `BuiltinKind::List`, handled above.
        B::NmTokens | B::IdRefs | B::Entities => unreachable!("list varieties handled above"),
    })
}

/// The closed range of an XSD integer type.
///
/// `xs:integer` and its unbounded subtypes are clamped to `i128`, which is
/// far beyond any real document and is documented on [`Value::Integer`].
fn integer_bounds(b: Builtin) -> (i128, i128) {
    use Builtin as B;
    match b {
        B::Byte => (-128, 127),
        B::Short => (-32_768, 32_767),
        B::Int => (i32::MIN as i128, i32::MAX as i128),
        B::Long => (i64::MIN as i128, i64::MAX as i128),
        B::UnsignedByte => (0, 255),
        B::UnsignedShort => (0, 65_535),
        B::UnsignedInt => (0, u32::MAX as i128),
        B::UnsignedLong => (0, u64::MAX as i128),
        B::NonNegativeInteger => (0, i128::MAX),
        B::PositiveInteger => (1, i128::MAX),
        B::NonPositiveInteger => (i128::MIN, 0),
        B::NegativeInteger => (i128::MIN, -1),
        _ => (i128::MIN, i128::MAX),
    }
}

/// Parses the XSD `integer` lexical form, which permits a leading sign and
/// leading zeroes but nothing else.
fn parse_integer(s: &str) -> Option<i128> {
    let (neg, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut n: i128 = 0;
    for b in digits.bytes() {
        n = n.checked_mul(10)?.checked_add(i128::from(b - b'0'))?;
    }
    Some(if neg { -n } else { n })
}

// ---------------------------------------------------------------------------
// XML name productions
// ---------------------------------------------------------------------------

/// XML 1.0 `NameStartChar`.
fn is_name_start_char(c: char) -> bool {
    matches!(c,
        ':' | '_' | 'A'..='Z' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

/// XML 1.0 `NameChar`.
fn is_name_char(c: char) -> bool {
    is_name_start_char(c)
        || matches!(c,
            '-' | '.' | '0'..='9' | '\u{B7}'
            | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

fn is_xml_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_name_start_char) && chars.all(is_name_char)
}

fn is_ncname(s: &str) -> bool {
    is_xml_name(s) && !s.contains(':')
}

/// RFC 3066 / BCP 47 shape, which is what `xs:language` constrains.
fn is_language(s: &str) -> bool {
    let mut parts = s.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    if first.is_empty() || first.len() > 8 || !first.bytes().all(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    parts.all(|p| !p.is_empty() && p.len() <= 8 && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

// ---------------------------------------------------------------------------
// Binary encodings
// ---------------------------------------------------------------------------

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let b = s.as_bytes();
    (0..b.len() / 2)
        .map(|i| {
            let hi = (b[2 * i] as char).to_digit(16)?;
            let lo = (b[2 * i + 1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    // XSD permits whitespace inside base64Binary; it is not part of the value.
    let clean: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !clean.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    for chunk in clean.chunks(4) {
        let mut buf = [0u32; 4];
        let mut pad = 0;
        for (i, &c) in chunk.iter().enumerate() {
            if c == b'=' {
                // Padding is only legal in the last two positions.
                if i < 2 {
                    return None;
                }
                pad += 1;
                buf[i] = 0;
            } else {
                if pad > 0 {
                    return None;
                }
                buf[i] = B64.iter().position(|&x| x == c)? as u32;
            }
        }
        let n = (buf[0] << 18) | (buf[1] << 12) | (buf[2] << 6) | buf[3];
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Facets
// ---------------------------------------------------------------------------

/// A facet the value does not satisfy.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FacetViolation {
    /// The facet's XSD element name, e.g. `maxInclusive`.
    pub facet: &'static str,
    pub message: String,
}

impl fmt::Display for FacetViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.facet, self.message)
    }
}

fn violation(facet: &'static str, message: impl Into<String>) -> FacetViolation {
    FacetViolation {
        facet,
        message: message.into(),
    }
}

/// Checks a value against a composed facet set.
///
/// `pattern` is **not** checked here. Patterns constrain the *lexical* form,
/// not the value, and compiling them is expensive enough to want doing once
/// per type rather than once per value — see [`check_patterns`].
pub fn check_facets(
    value: &Value,
    facets: &FacetSet,
    builtin: Builtin,
) -> Result<(), FacetViolation> {
    if let Some(len) = value.facet_length() {
        if let Some(want) = facets.length {
            if len as u64 != want {
                return Err(violation(
                    "length",
                    format!("length is {len}, must be {want}"),
                ));
            }
        }
        if let Some(min) = facets.min_length {
            if (len as u64) < min {
                return Err(violation(
                    "minLength",
                    format!("length is {len}, minimum is {min}"),
                ));
            }
        }
        if let Some(max) = facets.max_length {
            if (len as u64) > max {
                return Err(violation(
                    "maxLength",
                    format!("length is {len}, maximum is {max}"),
                ));
            }
        }
    }

    // Enumeration compares in the *value* space: `1.0` satisfies an
    // enumeration listing `1.00`, which a string comparison would reject.
    if let Some(allowed) = &facets.enumeration {
        let matched = allowed
            .iter()
            .any(|lex| parse(builtin, lex).map(|v| &v == value).unwrap_or(false));
        if !matched {
            return Err(violation(
                "enumeration",
                format!(
                    "`{value}` is not one of the {} permitted values",
                    allowed.len()
                ),
            ));
        }
    }

    let bound = |name: &'static str, lex: &str, ok: fn(std::cmp::Ordering) -> bool| {
        let limit = parse(builtin, lex).ok()?;
        let ord = value.partial_cmp_value(&limit)?;
        (!ok(ord)).then(|| violation(name, format!("`{value}` violates {name} `{lex}`")))
    };
    use std::cmp::Ordering::*;
    if let Some(l) = &facets.min_inclusive {
        if let Some(v) = bound("minInclusive", l, |o| matches!(o, Greater | Equal)) {
            return Err(v);
        }
    }
    if let Some(l) = &facets.min_exclusive {
        if let Some(v) = bound("minExclusive", l, |o| o == Greater) {
            return Err(v);
        }
    }
    if let Some(l) = &facets.max_inclusive {
        if let Some(v) = bound("maxInclusive", l, |o| matches!(o, Less | Equal)) {
            return Err(v);
        }
    }
    if let Some(l) = &facets.max_exclusive {
        if let Some(v) = bound("maxExclusive", l, |o| o == Less) {
            return Err(v);
        }
    }

    if facets.total_digits.is_some() || facets.fraction_digits.is_some() {
        if let Some((total, fraction)) = digit_counts(value) {
            if let Some(max) = facets.total_digits {
                if total > max {
                    return Err(violation(
                        "totalDigits",
                        format!("`{value}` has {total} digits, maximum is {max}"),
                    ));
                }
            }
            if let Some(max) = facets.fraction_digits {
                if fraction > max {
                    return Err(violation(
                        "fractionDigits",
                        format!("`{value}` has {fraction} fraction digits, maximum is {max}"),
                    ));
                }
            }
        }
    }

    Ok(())
}

/// `(totalDigits, fractionDigits)` of a decimal-derived value, counted from
/// its canonical form so trailing zeroes do not inflate the total.
fn digit_counts(value: &Value) -> Option<(u32, u32)> {
    let text = match value {
        Value::Decimal(d) => d.to_string(),
        Value::Integer(n) => n.to_string(),
        _ => return None,
    };
    let text = text.trim_start_matches(['-', '+']);
    let (int, frac) = match text.split_once('.') {
        Some((i, f)) => (i, f.trim_end_matches('0')),
        None => (text, ""),
    };
    let int_digits = int.trim_start_matches('0').len() as u32;
    Some((int_digits + frac.len() as u32, frac.len() as u32))
}

/// Checks the lexical form against compiled patterns.
///
/// Separate from [`check_facets`] for two reasons. Patterns constrain the
/// lexical form rather than the value — `1.0` and `1.00` are one decimal but
/// two strings, and a pattern can tell them apart. And compiling is expensive,
/// so a caller compiles once per type and matches many values.
///
/// The form given must already be whitespace-normalised, since that is what
/// the schema's own `whiteSpace` facet produced.
pub fn check_patterns(
    normalized_lexical: &str,
    patterns: &crate::regex::Patterns,
) -> Result<(), FacetViolation> {
    match patterns.first_failure(normalized_lexical) {
        None => Ok(()),
        Some(step) => Err(violation(
            "pattern",
            format!("`{normalized_lexical}` does not match {}", step.as_str()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datatypes::Facet;

    fn v(b: Builtin, s: &str) -> Value {
        parse(b, s).unwrap_or_else(|e| panic!("{e}"))
    }

    /// The ordering rule: collapse runs before the lexical parse, which is
    /// what makes `<v> 42 </v>` a valid `xs:int`.
    #[test]
    fn whitespace_is_applied_before_parsing() {
        assert_eq!(v(Builtin::Int, " 42 "), Value::Integer(42));
        assert_eq!(v(Builtin::Boolean, "\n true \t"), Value::Boolean(true));
        // xs:string preserves, so it keeps what the document said.
        assert_eq!(v(Builtin::String, " hi "), Value::String(" hi ".into()));
        assert_eq!(v(Builtin::Token, " a  b "), Value::String("a b".into()));
    }

    #[test]
    fn boolean_accepts_exactly_four_spellings() {
        for (s, want) in [("true", true), ("1", true), ("false", false), ("0", false)] {
            assert_eq!(v(Builtin::Boolean, s), Value::Boolean(want));
        }
        // Unlike some lenient readers, XSD does not admit these.
        for s in ["yes", "no", "TRUE", "on", ""] {
            assert!(parse(Builtin::Boolean, s).is_err(), "{s} must not parse");
        }
    }

    #[test]
    fn integer_bounds_are_enforced_per_type() {
        assert_eq!(v(Builtin::Byte, "127"), Value::Integer(127));
        assert!(parse(Builtin::Byte, "128").is_err());
        assert_eq!(v(Builtin::UnsignedByte, "255"), Value::Integer(255));
        assert!(parse(Builtin::UnsignedByte, "-1").is_err());
        // unsignedLong's maximum does not fit in i64, which is why values are
        // held as i128.
        assert_eq!(
            v(Builtin::UnsignedLong, "18446744073709551615"),
            Value::Integer(18_446_744_073_709_551_615)
        );
        assert!(parse(Builtin::PositiveInteger, "0").is_err());
        assert!(parse(Builtin::NegativeInteger, "0").is_err());
        assert_eq!(v(Builtin::Integer, "+007"), Value::Integer(7));
        assert!(
            parse(Builtin::Integer, "1.0").is_err(),
            "no fraction in an integer"
        );
        assert!(
            parse(Builtin::Integer, "1e3").is_err(),
            "no exponent in an integer"
        );
    }

    #[test]
    fn temporal_types_round_trip() {
        assert_eq!(v(Builtin::Date, "2024-03-31").to_string(), "2024-03-31");
        assert_eq!(
            v(Builtin::DateTime, "2024-12-30T12:39:15Z").to_string(),
            "2024-12-30T12:39:15Z"
        );
        assert_eq!(
            v(Builtin::Duration, "P1Y2M3DT4H5M6S").to_string(),
            "P1Y2M3DT4H5M6S"
        );
        assert_eq!(v(Builtin::GYear, "2024").to_string(), "2024");
        assert!(
            parse(Builtin::Date, "2024-02-30").is_err(),
            "February has no 30th"
        );
    }

    /// The one thing `dateTimeStamp` adds over `dateTime`.
    #[test]
    fn date_time_stamp_requires_a_timezone() {
        assert!(parse(Builtin::DateTime, "2024-01-01T00:00:00").is_ok());
        assert!(parse(Builtin::DateTimeStamp, "2024-01-01T00:00:00").is_err());
        assert!(parse(Builtin::DateTimeStamp, "2024-01-01T00:00:00Z").is_ok());
    }

    #[test]
    fn name_productions_are_enforced() {
        assert!(parse(Builtin::NcName, "well_1").is_ok());
        assert!(
            parse(Builtin::NcName, "ns:well").is_err(),
            "an NCName has no colon"
        );
        assert!(parse(Builtin::Name, "ns:well").is_ok());
        assert!(
            parse(Builtin::Name, "1well").is_err(),
            "a Name cannot start with a digit"
        );
        assert!(parse(Builtin::NmToken, "1well").is_ok(), "an NMTOKEN can");
        assert!(
            parse(Builtin::NmToken, "a b").is_err(),
            "an NMTOKEN has no space"
        );
        assert!(parse(Builtin::Language, "en-GB").is_ok());
        assert!(parse(Builtin::Language, "en-").is_err());
        assert!(parse(Builtin::Language, "123").is_err());
    }

    #[test]
    fn list_types_parse_each_item() {
        let Value::List(items) = v(Builtin::NmTokens, " a  b c ") else {
            panic!("expected a list")
        };
        assert_eq!(items.len(), 3);
        assert!(
            parse(Builtin::IdRefs, "ok 1bad").is_err(),
            "items are validated too"
        );
    }

    #[test]
    fn binary_types_decode() {
        assert_eq!(
            v(Builtin::HexBinary, "0FB7"),
            Value::HexBinary(vec![0x0F, 0xB7])
        );
        assert!(parse(Builtin::HexBinary, "0FB").is_err(), "odd digit count");
        assert!(parse(Builtin::HexBinary, "0FGZ").is_err());

        assert_eq!(
            v(Builtin::Base64Binary, "aGVsbG8="),
            Value::Base64Binary(b"hello".to_vec())
        );
        // Whitespace inside base64 is permitted and is not part of the value.
        assert_eq!(
            v(Builtin::Base64Binary, "aGVs bG8="),
            Value::Base64Binary(b"hello".to_vec())
        );
        assert!(
            parse(Builtin::Base64Binary, "aGVsbG8").is_err(),
            "length must be a multiple of 4"
        );
    }

    #[test]
    fn base64_round_trips() {
        for raw in [&b""[..], b"a", b"ab", b"abc", b"abcd", b"\x00\xFF\x10"] {
            let encoded = base64_encode(raw);
            assert_eq!(base64_decode(&encoded).as_deref(), Some(raw), "{encoded}");
        }
    }

    // -- facets ------------------------------------------------------------

    fn facets(list: &[Facet]) -> FacetSet {
        FacetSet::new().restrict(list)
    }

    #[test]
    fn length_facets_count_the_right_thing() {
        let f = facets(&[Facet::MaxLength(3)]);
        assert!(check_facets(&v(Builtin::String, "abc"), &f, Builtin::String).is_ok());
        assert!(check_facets(&v(Builtin::String, "abcd"), &f, Builtin::String).is_err());
        // A list counts items, not characters.
        assert!(check_facets(&v(Builtin::NmTokens, "a b c"), &f, Builtin::NmTokens).is_ok());
        assert!(check_facets(&v(Builtin::NmTokens, "a b c d"), &f, Builtin::NmTokens).is_err());
        // Binary counts octets.
        assert!(check_facets(&v(Builtin::HexBinary, "0F1E2D"), &f, Builtin::HexBinary).is_ok());
    }

    /// Facets constrain the value space, so `1.0` satisfies an enumeration
    /// that lists `1.00`. Comparing lexical forms would reject it.
    #[test]
    fn enumeration_compares_values_not_strings() {
        let f = facets(&[
            Facet::Enumeration("1.00".into()),
            Facet::Enumeration("2".into()),
        ]);
        assert!(check_facets(&v(Builtin::Decimal, "1.0"), &f, Builtin::Decimal).is_ok());
        assert!(check_facets(&v(Builtin::Decimal, "2.000"), &f, Builtin::Decimal).is_ok());
        assert!(check_facets(&v(Builtin::Decimal, "3"), &f, Builtin::Decimal).is_err());
    }

    #[test]
    fn bounds_are_checked_in_the_value_space() {
        let f = facets(&[
            Facet::MinInclusive("0".into()),
            Facet::MaxExclusive("10".into()),
        ]);
        for ok in ["0", "9.999", "0.0"] {
            assert!(
                check_facets(&v(Builtin::Decimal, ok), &f, Builtin::Decimal).is_ok(),
                "{ok}"
            );
        }
        for bad in ["-0.1", "10", "11"] {
            assert!(
                check_facets(&v(Builtin::Decimal, bad), &f, Builtin::Decimal).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn temporal_bounds_work_too() {
        let f = facets(&[Facet::MinInclusive("2024-01-01".into())]);
        assert!(check_facets(&v(Builtin::Date, "2024-06-01"), &f, Builtin::Date).is_ok());
        assert!(check_facets(&v(Builtin::Date, "2023-12-31"), &f, Builtin::Date).is_err());
    }

    #[test]
    fn digit_facets_count_canonically() {
        let f = facets(&[Facet::TotalDigits(4), Facet::FractionDigits(2)]);
        assert!(check_facets(&v(Builtin::Decimal, "12.34"), &f, Builtin::Decimal).is_ok());
        // Trailing zeroes are not significant digits.
        assert!(check_facets(&v(Builtin::Decimal, "12.3400"), &f, Builtin::Decimal).is_ok());
        assert!(check_facets(&v(Builtin::Decimal, "12.345"), &f, Builtin::Decimal).is_err());
        assert!(check_facets(&v(Builtin::Decimal, "123.45"), &f, Builtin::Decimal).is_err());
    }

    #[test]
    fn a_violation_names_its_facet() {
        let f = facets(&[Facet::MaxInclusive("5".into())]);
        let e = check_facets(&v(Builtin::Int, "6"), &f, Builtin::Int).unwrap_err();
        assert_eq!(e.facet, "maxInclusive");
        assert!(e.to_string().contains("maxInclusive"), "{e}");
    }

    #[test]
    fn patterns_constrain_the_lexical_form_not_the_value() {
        let p = crate::regex::Patterns::compile(&[vec!["[0-9]\\.[0-9]".into()]]).unwrap();
        // Both are the same decimal value; only one has the lexical shape.
        assert!(check_patterns("1.0", &p).is_ok());
        assert!(check_patterns("1.00", &p).is_err());
        assert_eq!(check_patterns("1.00", &p).unwrap_err().facet, "pattern");
    }

    #[test]
    fn errors_quote_what_the_document_said() {
        let e = parse(Builtin::Int, " nope ").unwrap_err();
        assert_eq!(e.lexical, " nope ", "the raw form, not the normalised one");
        assert!(e.to_string().contains("xs:int"), "{e}");
    }
}

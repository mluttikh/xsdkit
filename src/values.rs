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

use crate::atomic::{
    Date, DateTime, DayTimeDuration, Decimal, Double, Duration, Float, GDay, GMonth, GMonthDay,
    GYear, GYearMonth, Time, YearMonthDuration,
};
use crate::datatypes::{Builtin, BuiltinKind, FacetSet};
use crate::load::Version;
use crate::names::QName;
use std::fmt;

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
    /// Whether this is a list value, whose facets count and compare items.
    pub fn is_list(&self) -> bool {
        matches!(self, Value::List(_))
    }

    pub fn partial_cmp_value(&self, other: &Value) -> Option<std::cmp::Ordering> {
        use Value::*;
        match (self, other) {
            (Decimal(a), Decimal(b)) => a.partial_cmp(b),
            (Integer(a), Integer(b)) => a.partial_cmp(b),
            (Decimal(a), Integer(b)) => a.partial_cmp(&crate::atomic::Decimal::from_integer(*b)?),
            (Integer(a), Decimal(b)) => crate::atomic::Decimal::from_integer(*a)?.partial_cmp(b),
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
            (Duration(a), Duration(b)) => duration_cmp(a, b),
            _ => None,
        }
    }
}

/// Orders two `xs:duration` values without walking the calendar.
///
/// The specification orders durations by adding both to four reference
/// dateTimes chosen to cover every combination of month length and leap year;
/// if all four comparisons agree the durations are ordered, and otherwise they
/// are genuinely incomparable — `P1M` against `P30D` has no answer until you
/// say which month.
///
/// `oxsdatatypes` implements that rule, but its date normalization walks the
/// calendar one month at a time. `PT424206887777785H` is a perfectly
/// well-formed duration that an untrusted instance document can carry into a
/// `maxInclusive` check, and normalizing it takes on the order of 10^11
/// iterations — a hang, not a slow answer. Found by fuzzing.
///
/// Both operands are added to the *same* reference date, so all the calendar
/// work reduces to turning a year and month into a day number, which the civil
/// calendar formula does in constant time.
fn duration_cmp(
    a: &crate::atomic::Duration,
    b: &crate::atomic::Duration,
) -> Option<std::cmp::Ordering> {
    // From the datatypes specification. Note the first is 1696, not the 1969
    // `oxsdatatypes` uses: 1696 is a leap year and 1969 is not, so the two
    // disagree on durations reaching back across February.
    const REFERENCES: [(i128, i128); 4] = [(1696, 9), (1697, 2), (1903, 3), (1903, 7)];

    let mut agreed = None;
    for r in REFERENCES {
        let ord = instant(r, a)?.cmp(&instant(r, b)?);
        match agreed {
            None => agreed = Some(ord),
            Some(prev) if prev == ord => {}
            Some(_) => return None,
        }
    }
    agreed
}

/// The instant reached by adding `d` to a reference date, as whole seconds
/// since the civil epoch and a remainder in units of 10^-18 seconds.
///
/// Two integers rather than one because the whole-second count runs to about
/// 10^25 and scaling that by the decimal's 10^18 would overflow. Splitting at
/// the second keeps the remainder non-negative, which is what makes comparing
/// the pair lexicographically the same as comparing the instants.
fn instant(reference: (i128, i128), d: &crate::atomic::Duration) -> Option<(i128, i128)> {
    // `years`/`months` and `days`/`hours`/`minutes`/`seconds` are the
    // normalized components: everything below the leading one is bounded, so
    // recombining them recovers the totals exactly.
    let months = i128::from(d.years())
        .checked_mul(12)?
        .checked_add(d.months().into())?;
    let total = reference
        .0
        .checked_mul(12)?
        .checked_add(reference.1 - 1)?
        .checked_add(months)?;
    let day = days_from_civil(total.div_euclid(12), total.rem_euclid(12) + 1)?;

    // The decimal is a fixed-point i128 scaled by 10^18, and holds less than a
    // minute, so splitting it at the second is exact.
    const SCALE: i128 = crate::atomic::Decimal::SCALE;
    let scaled = d.seconds().to_i128_scaled();

    let seconds = day
        .checked_mul(86_400)?
        .checked_add(i128::from(d.days()).checked_mul(86_400)?)?
        .checked_add(i128::from(d.hours()).checked_mul(3_600)?)?
        .checked_add(i128::from(d.minutes()).checked_mul(60)?)?
        .checked_add(scaled.div_euclid(SCALE))?;
    Some((seconds, scaled.rem_euclid(SCALE)))
}

/// Days from 1970-01-01 to the first of the given month, proleptic Gregorian.
///
/// Hinnant's civil-calendar formula, with the day fixed at 1 — every reference
/// date is the first of a month, so there is no end-of-month clamping to do.
fn days_from_civil(year: i128, month: i128) -> Option<i128> {
    let y = year - i128::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 }.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era.checked_mul(146_097)?.checked_add(doe - 719_468)
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
///
/// Reads the **XSD 1.1** lexical spaces, which are a superset of 1.0's. With
/// no schema in hand there is nothing to say which language applies, and
/// refusing a form that some schema somewhere admits is the more surprising
/// answer. Use [`parse_in`] where the version is known — validating against a
/// schema does, and [`crate::validate::Validator`] passes it through.
pub fn parse(builtin: Builtin, lexical: &str) -> Result<Value, ValueError> {
    parse_in(builtin, lexical, Version::Xsd11)
}

/// Parses a lexical form in a particular version of XSD.
///
/// The two languages differ in exactly two places among the built-ins, both
/// widenings that 1.1 made and 1.0 forbids:
///
/// - the year `0000`, which 1.1 admits as 1 BCE and 1.0 prohibits outright;
/// - `+INF`, which 1.1 added to the special values and 1.0 does not have.
pub fn parse_in(builtin: Builtin, lexical: &str, version: Version) -> Result<Value, ValueError> {
    let normalized = builtin.white_space().normalize(lexical);
    let s = normalized.as_ref();
    parse_normalized(builtin, s, lexical, version)
}

/// Parses an already-whitespace-normalised lexical form.
///
/// `raw` is carried only so errors quote what the document actually said.
fn parse_normalized(
    builtin: Builtin,
    s: &str,
    raw: &str,
    version: Version,
) -> Result<Value, ValueError> {
    use Builtin as B;

    // A list type's value is its items', so the item type does the work.
    if let BuiltinKind::List(item) = builtin.kind() {
        let items = s
            .split_whitespace()
            .map(|tok| parse_in(item, tok, version))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Value::List(items));
    }

    let bad = |reason: &str| err(builtin, raw, reason);

    // The 1.0/1.1 divergences. Both are forms 1.1 added, so 1.0 refuses them
    // and the underlying parser — which implements 1.1 — would not.
    if version == Version::Xsd10 {
        if matches!(builtin, B::Float | B::Double) && s == "+INF" {
            return Err(bad("`+INF` is XSD 1.1; XSD 1.0 spells it `INF`"));
        }
        if is_temporal(builtin) && has_year_zero(s) {
            return Err(bad(
                "the year 0000 is prohibited in XSD 1.0; 1 BCE is `-0001`",
            ));
        }
    }

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

        B::Decimal => Value::Decimal(Decimal::parse_lexical(s).map_err(|e| bad(&e))?),

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

        B::Float => Value::Float(Float::parse_lexical(s).map_err(|e| bad(&e))?),
        B::Double => Value::Double(Double::parse_lexical(s).map_err(|e| bad(&e))?),

        B::Duration => Value::Duration(Duration::parse_lexical(s).map_err(|e| bad(&e))?),
        B::YearMonthDuration => {
            Value::YearMonthDuration(YearMonthDuration::parse_lexical(s).map_err(|e| bad(&e))?)
        }
        B::DayTimeDuration => {
            Value::DayTimeDuration(DayTimeDuration::parse_lexical(s).map_err(|e| bad(&e))?)
        }

        B::DateTime => Value::DateTime(DateTime::parse_lexical(s).map_err(|e| bad(&e))?),
        B::DateTimeStamp => {
            let v = DateTime::parse_lexical(s).map_err(|e| bad(&e))?;
            if v.timezone_offset().is_none() {
                // The one thing dateTimeStamp adds over dateTime.
                return Err(bad("a timezone is required by xs:dateTimeStamp"));
            }
            Value::DateTime(v)
        }
        B::Time => Value::Time(Time::parse_lexical(s).map_err(|e| bad(&e))?),
        B::Date => Value::Date(Date::parse_lexical(s).map_err(|e| bad(&e))?),
        B::GYearMonth => Value::GYearMonth(GYearMonth::parse_lexical(s).map_err(|e| bad(&e))?),
        B::GYear => Value::GYear(GYear::parse_lexical(s).map_err(|e| bad(&e))?),
        B::GMonthDay => Value::GMonthDay(GMonthDay::parse_lexical(s).map_err(|e| bad(&e))?),
        B::GDay => Value::GDay(GDay::parse_lexical(s).map_err(|e| bad(&e))?),
        B::GMonth => Value::GMonth(GMonth::parse_lexical(s).map_err(|e| bad(&e))?),

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
/// Whether this built-in's lexical form starts with a year.
///
/// `xs:time`, `xs:gMonth`, `xs:gDay` and `xs:gMonthDay` carry no year, so the
/// year-zero rule cannot apply to them.
fn is_temporal(builtin: Builtin) -> bool {
    use Builtin as B;
    matches!(
        builtin,
        B::DateTime | B::DateTimeStamp | B::Date | B::GYearMonth | B::GYear
    )
}

/// Whether the lexical form names the year zero.
///
/// The year is the leading field, optionally signed, and at least four digits.
/// `-0000` is not a way to write it either: the sign is what distinguishes
/// 1 BCE from 1 CE, and zero has no sign.
fn has_year_zero(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    let year: String = digits.chars().take_while(char::is_ascii_digit).collect();
    year.len() >= 4 && year.bytes().all(|b| b == b'0')
}

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
    //
    // A list is the exception. Its enumeration literals are lists too, and
    // comparing them means parsing each one against the *item* type, which is
    // not reachable from here — so `Validator::list` does that itself and this
    // would only ever compare a list against a string and reject everything.
    if let Some(allowed) = facets.enumeration.as_ref().filter(|_| !value.is_list()) {
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

    fn dur_cmp(a: &str, b: &str) -> Option<std::cmp::Ordering> {
        v(Builtin::Duration, a).partial_cmp_value(&v(Builtin::Duration, b))
    }

    /// The ordering examples from the datatypes specification. A duration is
    /// ordered against another only when adding both to all four reference
    /// dateTimes agrees; `P1M` against `P30D` genuinely has no answer.
    #[test]
    fn durations_are_ordered_by_the_four_reference_dates() {
        use std::cmp::Ordering::*;

        assert_eq!(dur_cmp("P1Y", "P364D"), Some(Greater));
        assert_eq!(dur_cmp("P1Y", "P367D"), Some(Less));
        assert_eq!(dur_cmp("P1Y", "P365D"), None, "leap years disagree");

        assert_eq!(dur_cmp("P1M", "P27D"), Some(Greater));
        assert_eq!(dur_cmp("P1M", "P32D"), Some(Less));
        for d in ["P28D", "P29D", "P30D", "P31D"] {
            assert_eq!(dur_cmp("P1M", d), None, "P1M vs {d}");
        }

        assert_eq!(dur_cmp("P5M", "P149D"), Some(Greater));
        assert_eq!(dur_cmp("P5M", "P154D"), Some(Less));
        assert_eq!(dur_cmp("P5M", "P150D"), None);

        // Equality, and the sub-second end where the seconds field is a
        // fixed-point decimal rather than an integer.
        assert_eq!(dur_cmp("P1Y", "P12M"), Some(Equal));
        assert_eq!(dur_cmp("PT1S", "PT0.999999999999999999S"), Some(Greater));
        assert_eq!(dur_cmp("PT0.5S", "PT0.5S"), Some(Equal));

        // Negatives, including the case that breaks a naive split of the
        // seconds at the minute: the sub-minute remainder carries a sign, so
        // the whole-second part alone orders these wrongly.
        assert_eq!(dur_cmp("-P1Y", "P1Y"), Some(Less));
        assert_eq!(dur_cmp("-PT0.1S", "PT0.1S"), Some(Less));
        assert_eq!(dur_cmp("PT1M", "PT59S"), Some(Greater));
        assert_eq!(dur_cmp("-PT1M", "-PT59S"), Some(Less));
        assert_eq!(dur_cmp("-P1M", "-P30D"), None);
    }

    /// `oxsdatatypes` compares durations by normalising a dateTime one month at
    /// a time, so a large but perfectly legal duration — the kind an untrusted
    /// instance can carry into a `maxInclusive` check — takes ~10^11 iterations.
    /// Ours is constant time. Found by fuzzing.
    #[test]
    fn comparing_a_huge_duration_terminates() {
        let big = v(Builtin::Duration, "P12MT424206887777785H506M");
        let start = std::time::Instant::now();
        assert_eq!(big.partial_cmp_value(&big), Some(std::cmp::Ordering::Equal));
        assert_eq!(
            big.partial_cmp_value(&v(Builtin::Duration, "P1D")),
            Some(std::cmp::Ordering::Greater)
        );
        assert!(start.elapsed().as_secs() < 5, "{:?}", start.elapsed());
    }

    /// The year is unbounded in the lexical space, so the arithmetic has to
    /// answer rather than overflow. Every step is checked, and an unanswerable
    /// comparison degrades to "incomparable".
    #[test]
    fn an_unrepresentable_duration_is_incomparable_not_a_panic() {
        let huge = format!("P{}Y", "9".repeat(30));
        let a = parse(Builtin::Duration, &huge);
        if let Ok(a) = a {
            let _ = a.partial_cmp_value(&v(Builtin::Duration, "P1D"));
        }
    }
}

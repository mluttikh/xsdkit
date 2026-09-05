//! The atomic value types a [`Value`](crate::values::Value) can hold.
//!
//! All of them are implemented here. They were wrappers around `oxsdatatypes`
//! until that library was found to reject `--02-29` — a valid `xs:gMonthDay`,
//! since the type names no year — and owning them turned out to be a few
//! hundred lines, most of it shared. `DESIGN.md` §3.12.4 records how that
//! decision was reached and reversed.
//!
//! Three rules run through everything below, and they are where the subtlety
//! is:
//!
//! - **Equality is the order relation, never the fields.**
//!   `2010-09-20T13:00:00+01:00` and `2010-09-20T12:00:00Z` are one instant
//!   written two ways, so an `xs:enumeration` listing either must admit both.
//! - **A value with no timezone names a 28-hour window**, not an instant, so
//!   it is ordered against a value that has one only when the whole window
//!   falls to one side. Being incomparable is a real answer.
//! - **Range is refused and precision is dropped.** A number too large to
//!   represent is an error; digits below 10^-18 are not, because the document
//!   is still valid and the specification only asks for eighteen.
//!
//! What each type offers is what a consumer actually does with a parsed value:
//! render it canonically ([`std::fmt::Display`]), order it, and take it apart
//! to build something of their own — a `chrono::DateTime`, an Arrow column, a
//! Python `datetime`.

use std::fmt;

/// An offset from UTC, in minutes east of it.
///
/// Absent when the value carries no timezone, which is what makes two
/// otherwise ordered values incomparable: without one, a `dateTime` names a
/// 28-hour-wide window rather than an instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimezoneOffset(i16);

impl TimezoneOffset {
    /// Minutes east of UTC, from -840 to 840.
    pub fn minutes(self) -> i16 {
        self.0
    }
}

/// `xs:decimal` and the types derived from it that are not integers.
///
/// Exact, not floating point: fixed point in an `i128` scaled by 10^18, which
/// is about 20 digits either side of the point. The specification's value space
/// is unbounded, so a literal whose *integer* part does not fit is rejected —
/// that is range. Fractional digits below 10^-18 are dropped instead, because
/// that is precision, and the specification only requires eighteen digits of
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Decimal(i128);

impl Decimal {
    /// The scale the fixed-point representation uses: 10^18.
    pub const SCALE: i128 = 1_000_000_000_000_000_000;

    /// The nearest decimal to an integer, absent when it does not fit.
    pub(crate) fn from_integer(n: i128) -> Option<Self> {
        n.checked_mul(Self::SCALE).map(Self)
    }

    /// Builds one from a value already scaled by [`Self::SCALE`].
    pub(crate) fn from_scaled(scaled: i128) -> Self {
        Self(scaled)
    }

    /// The value scaled by 10^18, which is how it is held.
    ///
    /// Exact, and the way to convert into a decimal type of your own without
    /// going through a string.
    pub fn to_i128_scaled(self) -> i128 {
        self.0
    }

    pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
        let (negative, rest) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        let (int, frac) = match rest.split_once('.') {
            Some((i, f)) => (i, f),
            None => (rest, ""),
        };
        // The production allows `1.` and `.5` but not `.` alone, and no
        // exponent — that spelling belongs to float and double.
        if int.is_empty() && frac.is_empty() {
            return Err("expected a decimal number".into());
        }
        if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
            return Err("a decimal is digits, with at most one point".into());
        }

        let too_big = || "the value has more digits than an xs:decimal can hold".to_string();
        let mut scaled: i128 = 0;
        for b in int.bytes() {
            scaled = scaled
                .checked_mul(10)
                .and_then(|v| v.checked_add(i128::from(b - b'0')))
                .ok_or_else(too_big)?;
        }
        scaled = scaled.checked_mul(Self::SCALE).ok_or_else(too_big)?;

        // Digits below 10^-18 are dropped rather than refused. The value space
        // is unbounded, so a literal with thirty fractional digits *is* a
        // decimal and a document carrying one is valid — refusing it would be
        // a conformance failure, where losing precision the specification only
        // requires 18 digits of is a documented limit. Overflow of the integer
        // part above is different: that is range, not precision.
        let mut unit = Self::SCALE;
        for b in frac.bytes() {
            if unit == 1 {
                break;
            }
            unit /= 10;
            scaled = scaled
                .checked_add(i128::from(b - b'0') * unit)
                .ok_or_else(too_big)?;
        }
        Ok(Self(if negative { -scaled } else { scaled }))
    }
}

impl fmt::Display for Decimal {
    /// The canonical form: no leading `+`, no insignificant zeroes, and no
    /// decimal point at all when the value is integral.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 < 0 {
            f.write_str("-")?;
        }
        let magnitude = self.0.unsigned_abs();
        let scale = Self::SCALE as u128;
        write!(f, "{}", magnitude / scale)?;
        let mut frac = magnitude % scale;
        if frac == 0 {
            return Ok(());
        }
        let mut digits = String::new();
        let mut unit = scale;
        while frac != 0 {
            unit /= 10;
            digits.push((b'0' + (frac / unit) as u8) as char);
            frac %= unit;
        }
        write!(f, ".{digits}")
    }
}

// ---------------------------------------------------------------------------
// float and double
// ---------------------------------------------------------------------------

/// Whether `s` matches XSD's numeral production, which is narrower than
/// Rust's.
///
/// Rust accepts `inf`, `infinity`, `1e5` with no digits before the point in
/// some forms, and is case-insensitive about all of it. XSD accepts none of
/// that, so the shape is checked here and only the digits are handed over.
fn is_xsd_numeral(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (mantissa, exponent) = match body.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e)),
        None => (body, None),
    };
    let (int, frac) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };
    // `1`, `1.`, `.5` and `1.5` are numerals; `.` and the empty string are not.
    let mantissa_ok = (!int.is_empty() || !frac.is_empty())
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit());
    let exponent_ok = match exponent {
        None => true,
        Some(e) => {
            let digits = e.strip_prefix(['+', '-']).unwrap_or(e);
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        }
    };
    mantissa_ok && exponent_ok
}

/// Parses one of XSD's special float values, which are spelled exactly.
///
/// `+INF` is XSD 1.1 only; the version rule in [`crate::values`] refuses it
/// for 1.0 rather than this, so both are accepted here.
fn special_value(s: &str) -> Option<f64> {
    match s {
        "INF" | "+INF" => Some(f64::INFINITY),
        "-INF" => Some(f64::NEG_INFINITY),
        "NaN" => Some(f64::NAN),
        _ => None,
    }
}

/// Writes a float or double the way XSD spells it.
fn write_float(f: &mut fmt::Formatter<'_>, v: f64, plain: &dyn fmt::Display) -> fmt::Result {
    if v.is_nan() {
        f.write_str("NaN")
    } else if v == f64::INFINITY {
        f.write_str("INF")
    } else if v == f64::NEG_INFINITY {
        f.write_str("-INF")
    } else {
        write!(f, "{plain}")
    }
}

/// `xs:float`, IEEE 754 single precision.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Float(f32);

/// `xs:double`, IEEE 754 double precision.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Double(f64);

impl Float {
    pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
        if let Some(v) = special_value(s) {
            return Ok(Self(v as f32));
        }
        if !is_xsd_numeral(s) {
            return Err("expected a number, `INF`, `-INF` or `NaN`".into());
        }
        s.parse().map(Self).map_err(|_| "not a float".to_string())
    }
}

impl Double {
    pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
        if let Some(v) = special_value(s) {
            return Ok(Self(v));
        }
        if !is_xsd_numeral(s) {
            return Err("expected a number, `INF`, `-INF` or `NaN`".into());
        }
        s.parse().map(Self).map_err(|_| "not a double".to_string())
    }
}

impl fmt::Display for Float {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_float(f, f64::from(self.0), &self.0)
    }
}

impl fmt::Display for Double {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_float(f, self.0, &self.0)
    }
}

impl From<Float> for f32 {
    fn from(v: Float) -> f32 {
        v.0
    }
}

impl From<Double> for f64 {
    fn from(v: Double) -> f64 {
        v.0
    }
}

// ---------------------------------------------------------------------------
// Shared temporal machinery
// ---------------------------------------------------------------------------

/// Days from 1970-01-01 to `year-month-01`, proleptic Gregorian.
///
/// Hinnant's civil-calendar formula. Constant time, which is what keeps
/// comparison from walking the calendar — a legal duration can reach far
/// enough that a month-at-a-time loop never returns.
pub(crate) fn days_from_civil(year: i128, month: i128) -> Option<i128> {
    let y = year - i128::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 }.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era.checked_mul(146_097)?.checked_add(doe - 719_468)
}

/// Whether `year` has a 29th of February.
fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// The number of days in a month of a particular year.
fn days_in_month(year: i64, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Orders two instants given as minutes from a common epoch, applying the rule
/// that makes the temporal types only *partially* ordered.
///
/// A value carrying no timezone does not name an instant; it names every
/// instant its 28-hour window could stand for. So it is ordered against a
/// value that does carry one only when the whole window falls to one side, and
/// otherwise the two are incomparable — which is a real answer, not a failure.
fn compare_instants(a: i128, a_zoned: bool, b: i128, b_zoned: bool) -> Option<std::cmp::Ordering> {
    const FOURTEEN_HOURS: i128 = 14 * 60;
    match (a_zoned, b_zoned) {
        (true, true) | (false, false) => Some(a.cmp(&b)),
        (false, true) => {
            if a + FOURTEEN_HOURS < b {
                Some(std::cmp::Ordering::Less)
            } else if a - FOURTEEN_HOURS > b {
                Some(std::cmp::Ordering::Greater)
            } else {
                None
            }
        }
        (true, false) => compare_instants(b, b_zoned, a, a_zoned).map(std::cmp::Ordering::reverse),
    }
}

/// A date-and-or-time value reduced to minutes from the epoch, in UTC.
fn utc_minutes(year: i64, month: u8, day: u8, minute_of_day: i64, offset: Option<i16>) -> i128 {
    let days = days_from_civil(i128::from(year), i128::from(month)).unwrap_or(0)
        + i128::from(day.saturating_sub(1));
    days * 1440 + i128::from(minute_of_day) - i128::from(offset.unwrap_or(0))
}

/// Reads the year field, which is signed, at least four digits, and unbounded
/// in the lexical space.
///
/// Returns the year and the rest of the input.
///
/// `-0000` is accepted. It looks like it should not be — zero has no sign —
/// but the lexical production allows it and the W3C suite has it inside a
/// schema it expects to be valid (`saxonData/Zone/zone202`). Year zero is
/// prohibited in XSD 1.0 rather than here, by the version rule in
/// [`crate::values`], which strips the sign before looking.
fn take_year(s: &str) -> Result<(i64, &str), String> {
    let (negative, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits < 4 || (digits > 4 && rest.as_bytes()[0] == b'0') {
        return Err("the year needs at least four digits, and no leading zero beyond that".into());
    }
    let year: i64 = rest[..digits]
        .parse()
        .map_err(|_| "the year is out of range".to_string())?;
    Ok((if negative { -year } else { year }, &rest[digits..]))
}

/// Renders a year in the canonical form: signed, at least four digits.
fn write_year(f: &mut fmt::Formatter<'_>, year: i64) -> fmt::Result {
    if year < 0 {
        write!(f, "-{:04}", year.unsigned_abs())
    } else {
        write!(f, "{year:04}")
    }
}

/// Reads two ASCII digits at the start of `s`, then the rest.
fn take_two<'a>(s: &'a str, what: &str) -> Result<(u8, &'a str), String> {
    let b = s.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_digit() || !b[1].is_ascii_digit() {
        return Err(format!("{what} must be two digits"));
    }
    Ok(((b[0] - b'0') * 10 + (b[1] - b'0'), &s[2..]))
}

/// Reads a literal separator.
fn take<'a>(s: &'a str, sep: u8, what: &str) -> Result<&'a str, String> {
    match s.as_bytes().first() {
        Some(&c) if c == sep => Ok(&s[1..]),
        _ => Err(format!("expected `{}` {what}", sep as char)),
    }
}

// ---------------------------------------------------------------------------
// gMonthDay
// ---------------------------------------------------------------------------

/// `xs:gMonthDay`, a day of a month recurring every year.
///
/// Ours rather than the backend's, because the backend rejects `--02-29`. A
/// gMonthDay names no year, so February has 29 days in it — the day exists,
/// just not every year — and the specification says so explicitly. The W3C
/// suite agrees (`msData/datatypes/gMonthDay004`).
///
/// It is also the smallest of the temporal types, which makes it the natural
/// first one to own outright: two small integers and an offset, with the
/// ordering rule the only subtle part.
#[derive(Clone, Copy, Debug)]
pub struct GMonthDay {
    month: u8,
    day: u8,
    offset: Option<i16>,
}

impl GMonthDay {
    pub fn month(self) -> u8 {
        self.month
    }

    pub fn day(self) -> u8 {
        self.day
    }

    pub fn timezone_offset(self) -> Option<TimezoneOffset> {
        self.offset.map(TimezoneOffset)
    }

    /// The most days the month can have, over any year.
    fn max_day(month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            // The whole point: no year, so the leap day is in.
            2 => 29,
            _ => 0,
        }
    }

    pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
        let rest = s
            .strip_prefix("--")
            .ok_or_else(|| "expected `--MM-DD`".to_string())?;
        // On bytes, and the shape is checked before anything is sliced: the
        // input is arbitrary text, so `&rest[..5]` would panic on a multi-byte
        // character rather than reject it.
        let b = rest.as_bytes();
        let shaped = b.len() >= 5
            && b[0].is_ascii_digit()
            && b[1].is_ascii_digit()
            && b[2] == b'-'
            && b[3].is_ascii_digit()
            && b[4].is_ascii_digit();
        if !shaped {
            return Err("expected `--MM-DD`".into());
        }
        let two = |hi: u8, lo: u8| (hi - b'0') * 10 + (lo - b'0');
        let month = two(b[0], b[1]);
        let day = two(b[3], b[4]);
        if !(1..=12).contains(&month) {
            return Err(format!("{month} is not a month"));
        }
        if day < 1 || day > Self::max_day(month) {
            return Err(format!("{day} is not a day of month {month}"));
        }
        Ok(Self {
            month,
            day,
            // Byte 5 is a character boundary: the five before it are ASCII.
            offset: parse_offset(&rest[5..])?,
        })
    }

    /// Minutes from the start of the reference year, in UTC.
    ///
    /// The specification orders these by mapping them into a leap year, so
    /// that `--02-29` has somewhere to land.
    fn utc_minutes(self) -> i32 {
        const BEFORE: [i32; 12] = [0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335];
        let day_of_year = BEFORE[usize::from(self.month) - 1] + i32::from(self.day) - 1;
        day_of_year * 1440 - i32::from(self.offset.unwrap_or(0))
    }
}

impl fmt::Display for GMonthDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--{:02}-{:02}", self.month, self.day)?;
        write_offset(f, self.offset)
    }
}

impl PartialOrd for GMonthDay {
    /// The timezone rule, same as every other temporal type: a value without
    /// one names a 28-hour-wide window, so it is ordered against a value with
    /// one only when the whole window falls to one side.
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        const FOURTEEN_HOURS: i32 = 14 * 60;
        let (a, b) = (self.utc_minutes(), other.utc_minutes());
        match (self.offset, other.offset) {
            (Some(_), Some(_)) | (None, None) => Some(a.cmp(&b)),
            (None, Some(_)) => {
                if a + FOURTEEN_HOURS < b {
                    Some(std::cmp::Ordering::Less)
                } else if a - FOURTEEN_HOURS > b {
                    Some(std::cmp::Ordering::Greater)
                } else {
                    None
                }
            }
            (Some(_), None) => other.partial_cmp(self).map(std::cmp::Ordering::reverse),
        }
    }
}

/// Parses the optional timezone that may follow any temporal lexical form.
fn parse_offset(s: &str) -> Result<Option<i16>, String> {
    if s.is_empty() {
        return Ok(None);
    }
    if s == "Z" {
        return Ok(Some(0));
    }
    // Bytes again, and shape before value: `s` is whatever the document said.
    let b = s.as_bytes();
    let sign: i16 = match b[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return Err("expected a timezone such as `Z` or `+02:00`".into()),
    };
    let shaped = b.len() == 6
        && b[1].is_ascii_digit()
        && b[2].is_ascii_digit()
        && b[3] == b':'
        && b[4].is_ascii_digit()
        && b[5].is_ascii_digit();
    if !shaped {
        return Err("expected a timezone such as `Z` or `+02:00`".into());
    }
    let two = |hi: u8, lo: u8| i16::from(hi - b'0') * 10 + i16::from(lo - b'0');
    let (hh, mm) = (two(b[1], b[2]), two(b[4], b[5]));
    let total = sign * (hh * 60 + mm);
    if mm > 59 || !(-840..=840).contains(&total) {
        return Err("a timezone must be between -14:00 and +14:00".into());
    }
    Ok(Some(total))
}

fn write_offset(f: &mut fmt::Formatter<'_>, offset: Option<i16>) -> fmt::Result {
    match offset {
        None => Ok(()),
        Some(0) => f.write_str("Z"),
        Some(m) => {
            let sign = if m < 0 { '-' } else { '+' };
            write!(f, "{sign}{:02}:{:02}", m.abs() / 60, m.abs() % 60)
        }
    }
}

// ---------------------------------------------------------------------------
// The other Gregorian fragments
// ---------------------------------------------------------------------------

/// Defines one of the `xs:g*` types: a few calendar fields and a timezone.
///
/// They differ only in which fields they carry and how they are written, so
/// the ordering — map to an instant, then apply the timezone rule — is shared.
macro_rules! gregorian {
    (
        $(#[$m:meta])*
        $name:ident { $($field:ident : $ty:ty),* $(,)? },
        parse: |$ps:ident| $parse:block,
        write: |$this:ident, $f:ident| $write:block,
        instant: |$iv:ident| $instant:expr,
    ) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug)]
        pub struct $name {
            $($field: $ty,)*
            offset: Option<i16>,
        }

        impl $name {
            $(
                pub fn $field(self) -> $ty {
                    self.$field
                }
            )*

            /// The offset from UTC, if the value carries one.
            pub fn timezone_offset(self) -> Option<TimezoneOffset> {
                self.offset.map(TimezoneOffset)
            }

            pub(crate) fn parse_lexical($ps: &str) -> Result<Self, String> $parse
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let $this = *self;
                let $f = &mut *f;
                $write?;
                write_offset(f, self.offset)
            }
        }

        impl PartialOrd for $name {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                let f = |$iv: &Self| $instant;
                compare_instants(
                    f(self),
                    self.offset.is_some(),
                    f(other),
                    other.offset.is_some(),
                )
            }
        }
    };
}

gregorian!(
    /// `xs:gYear`.
    GYear { year: i64 },
    parse: |s| {
        let (year, rest) = take_year(s)?;
        Ok(Self { year, offset: parse_offset(rest)? })
    },
    write: |v, f| { write_year(f, v.year) },
    instant: |v| utc_minutes(v.year, 1, 1, 0, v.offset),
);

gregorian!(
    /// `xs:gYearMonth`, a month of a particular year.
    GYearMonth { year: i64, month: u8 },
    parse: |s| {
        let (year, rest) = take_year(s)?;
        let rest = take(rest, b'-', "between the year and the month")?;
        let (month, rest) = take_two(rest, "the month")?;
        if !(1..=12).contains(&month) {
            return Err(format!("{month} is not a month"));
        }
        Ok(Self { year, month, offset: parse_offset(rest)? })
    },
    write: |v, f| { write_year(f, v.year).and_then(|()| write!(f, "-{:02}", v.month)) },
    instant: |v| utc_minutes(v.year, v.month, 1, 0, v.offset),
);

gregorian!(
    /// `xs:gMonth`, a month recurring every year.
    GMonth { month: u8 },
    parse: |s| {
        let rest = s.strip_prefix("--").ok_or_else(|| "expected `--MM`".to_string())?;
        let (month, rest) = take_two(rest, "the month")?;
        if !(1..=12).contains(&month) {
            return Err(format!("{month} is not a month"));
        }
        Ok(Self { month, offset: parse_offset(rest)? })
    },
    write: |v, f| { write!(f, "--{:02}", v.month) },
    // The reference year is a leap year, so every month has its full length.
    instant: |v| utc_minutes(1972, v.month, 1, 0, v.offset),
);

gregorian!(
    /// `xs:gDay`, a day recurring every month.
    GDay { day: u8 },
    parse: |s| {
        let rest = s.strip_prefix("---").ok_or_else(|| "expected `---DD`".to_string())?;
        let (day, rest) = take_two(rest, "the day")?;
        // Up to 31: a gDay names no month, so the longest one decides.
        if !(1..=31).contains(&day) {
            return Err(format!("{day} is not a day of any month"));
        }
        Ok(Self { day, offset: parse_offset(rest)? })
    },
    write: |v, f| { write!(f, "---{:02}", v.day) },
    instant: |v| utc_minutes(1972, 1, v.day, 0, v.offset),
);

// ---------------------------------------------------------------------------
// Date, Time and DateTime
// ---------------------------------------------------------------------------

/// Seconds within a minute, as a count of 10^-18 seconds.
///
/// XSD allows a fractional part of unbounded precision; this holds as much of
/// it as `Decimal` does, which is what the rest of the value layer uses.
type Subsecond = i128;

/// Reads `SS[.fff…]`, returning it scaled the way [`Decimal`] scales.
fn take_seconds(s: &str) -> Result<(Subsecond, &str), String> {
    let (whole, rest) = take_two(s, "the seconds")?;
    if whole > 59 {
        // XSD has no leap seconds: the value space stops at 59.
        return Err(format!("{whole} is not a number of seconds"));
    }
    let mut scaled = Subsecond::from(whole) * Decimal::SCALE;
    let rest = match rest.strip_prefix('.') {
        None => rest,
        Some(frac) => {
            let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return Err("the fractional seconds need at least one digit".into());
            }
            let mut unit = Decimal::SCALE;
            for b in frac[..digits].bytes() {
                unit /= 10;
                scaled += Subsecond::from(b - b'0') * unit;
                if unit == 0 {
                    break;
                }
            }
            &frac[digits..]
        }
    };
    Ok((scaled, rest))
}

/// Writes seconds in the canonical form: two digits, and a fractional part
/// only when there is one, with no trailing zeroes.
fn write_seconds(f: &mut fmt::Formatter<'_>, scaled: Subsecond) -> fmt::Result {
    write!(f, "{:02}", scaled / Decimal::SCALE)?;
    let mut frac = scaled % Decimal::SCALE;
    if frac == 0 {
        return Ok(());
    }
    let mut digits = String::new();
    let mut unit = Decimal::SCALE;
    while frac != 0 {
        unit /= 10;
        digits.push((b'0' + (frac / unit) as u8) as char);
        frac %= unit;
    }
    write!(f, ".{digits}")
}

/// `xs:time`, and the time half of `xs:dateTime`.
#[derive(Clone, Copy, Debug)]
pub struct Time {
    hour: u8,
    minute: u8,
    second: Subsecond,
    offset: Option<i16>,
}

/// The clock fields, and whether they named the midnight that ends the day.
fn take_clock(s: &str) -> Result<(u8, u8, Subsecond, bool, &str), String> {
    let (hour, rest) = take_two(s, "the hour")?;
    let rest = take(rest, b':', "after the hour")?;
    let (minute, rest) = take_two(rest, "the minute")?;
    let rest = take(rest, b':', "after the minute")?;
    let (second, rest) = take_seconds(rest)?;
    if minute > 59 {
        return Err(format!("{minute} is not a minute"));
    }
    // `24:00:00` is the midnight that *ends* a day, and it is the only use of
    // hour 24 the lexical space allows.
    let end_of_day = hour == 24;
    if end_of_day && (minute != 0 || second != 0) {
        return Err("`24` is only an hour in `24:00:00`".into());
    }
    if hour > 24 {
        return Err(format!("{hour} is not an hour"));
    }
    Ok((hour, minute, second, end_of_day, rest))
}

impl Time {
    pub fn hour(self) -> u8 {
        self.hour
    }

    pub fn minute(self) -> u8 {
        self.minute
    }

    /// Seconds within the minute, which XSD allows a fractional part.
    pub fn second(self) -> Decimal {
        Decimal::from_scaled(self.second)
    }

    pub fn timezone_offset(self) -> Option<TimezoneOffset> {
        self.offset.map(TimezoneOffset)
    }

    pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
        let (hour, minute, second, end_of_day, rest) = take_clock(s)?;
        Ok(Self {
            // A time has no day to roll into, so the end of one is the start
            // of the next.
            hour: if end_of_day { 0 } else { hour },
            minute,
            second,
            offset: parse_offset(rest)?,
        })
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:", self.hour, self.minute)?;
        write_seconds(f, self.second)?;
        write_offset(f, self.offset)
    }
}

impl PartialOrd for Time {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // A time spans one day, so nothing here can overflow; it uses the pair
        // form anyway so both types answer with the same code.
        let split = |v: &Self| {
            (
                i128::from(v.hour) * 60 + i128::from(v.minute) - i128::from(v.offset.unwrap_or(0)),
                v.second,
            )
        };
        compare_split(
            split(self),
            self.offset.is_some(),
            split(other),
            other.offset.is_some(),
        )
    }
}

/// `xs:date`.
#[derive(Clone, Copy, Debug)]
pub struct Date {
    year: i64,
    month: u8,
    day: u8,
    offset: Option<i16>,
}

/// The calendar fields, checked against the real length of the month.
fn take_calendar(s: &str) -> Result<(i64, u8, u8, &str), String> {
    let (year, rest) = take_year(s)?;
    let rest = take(rest, b'-', "after the year")?;
    let (month, rest) = take_two(rest, "the month")?;
    let rest = take(rest, b'-', "after the month")?;
    let (day, rest) = take_two(rest, "the day")?;
    if !(1..=12).contains(&month) {
        return Err(format!("{month} is not a month"));
    }
    if day < 1 || day > days_in_month(year, month) {
        return Err(format!("{day} is not a day of {month} in {year}"));
    }
    Ok((year, month, day, rest))
}

impl Date {
    pub fn year(self) -> i64 {
        self.year
    }

    pub fn month(self) -> u8 {
        self.month
    }

    pub fn day(self) -> u8 {
        self.day
    }

    pub fn timezone_offset(self) -> Option<TimezoneOffset> {
        self.offset.map(TimezoneOffset)
    }

    pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
        let (year, month, day, rest) = take_calendar(s)?;
        Ok(Self {
            year,
            month,
            day,
            offset: parse_offset(rest)?,
        })
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_year(f, self.year)?;
        write!(f, "-{:02}-{:02}", self.month, self.day)?;
        write_offset(f, self.offset)
    }
}

impl PartialOrd for Date {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let m = |v: &Self| utc_minutes(v.year, v.month, v.day, 0, v.offset);
        compare_instants(
            m(self),
            self.offset.is_some(),
            m(other),
            other.offset.is_some(),
        )
    }
}

/// `xs:dateTime`, and `xs:dateTimeStamp` which additionally requires the
/// timezone to be present.
#[derive(Clone, Copy, Debug)]
pub struct DateTime {
    date: Date,
    time: Time,
}

impl DateTime {
    pub fn year(self) -> i64 {
        self.date.year
    }

    pub fn month(self) -> u8 {
        self.date.month
    }

    pub fn day(self) -> u8 {
        self.date.day
    }

    pub fn hour(self) -> u8 {
        self.time.hour
    }

    pub fn minute(self) -> u8 {
        self.time.minute
    }

    /// Seconds within the minute, which XSD allows a fractional part.
    pub fn second(self) -> Decimal {
        Decimal::from_scaled(self.time.second)
    }

    pub fn timezone_offset(self) -> Option<TimezoneOffset> {
        self.date.offset.map(TimezoneOffset)
    }

    pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
        let (year, month, day, rest) = take_calendar(s)?;
        let rest = take(rest, b'T', "between the date and the time")?;
        let (hour, minute, second, end_of_day, rest) = take_clock(rest)?;
        let offset = parse_offset(rest)?;

        let mut date = Date {
            year,
            month,
            day,
            offset,
        };
        // `24:00:00` is the midnight that ends this day, which is the midnight
        // that starts the next one — so the date moves.
        if end_of_day {
            date = date.next_day();
        }
        Ok(Self {
            date,
            time: Time {
                hour: if end_of_day { 0 } else { hour },
                minute,
                second,
                offset,
            },
        })
    }
}

impl Date {
    /// The day after this one, carrying through the month and the year.
    fn next_day(self) -> Self {
        let mut d = self;
        if d.day < days_in_month(d.year, d.month) {
            d.day += 1;
        } else if d.month < 12 {
            d.month += 1;
            d.day = 1;
        } else {
            d.year += 1;
            d.month = 1;
            d.day = 1;
        }
        d
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_year(f, self.date.year)?;
        write!(
            f,
            "-{:02}-{:02}T{:02}:{:02}:",
            self.date.month, self.date.day, self.time.hour, self.time.minute
        )?;
        write_seconds(f, self.time.second)?;
        write_offset(f, self.date.offset)
    }
}

impl PartialOrd for DateTime {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // Whole minutes and the remainder within the minute, rather than one
        // count of 10^-18 seconds. The year is unbounded in the lexical space,
        // and `6044555555555-06-06T22:13:00` is a perfectly good dateTime
        // whose attosecond count does not fit in an `i128`. Splitting keeps
        // each half small, and comparing the pair lexicographically is the
        // same comparison, because the remainder is non-negative and below a
        // minute.
        let split = |v: &Self| {
            (
                utc_minutes(
                    v.date.year,
                    v.date.month,
                    v.date.day,
                    i64::from(v.time.hour) * 60 + i64::from(v.time.minute),
                    v.date.offset,
                ),
                v.time.second,
            )
        };
        compare_split(
            split(self),
            self.date.offset.is_some(),
            split(other),
            other.date.offset.is_some(),
        )
    }
}

/// The order rule for an instant given as (whole minutes, the remainder within
/// the minute).
fn compare_split(
    a: (i128, i128),
    a_zoned: bool,
    b: (i128, i128),
    b_zoned: bool,
) -> Option<std::cmp::Ordering> {
    const FOURTEEN_HOURS: i128 = 14 * 60;
    match (a_zoned, b_zoned) {
        (true, true) | (false, false) => Some(a.cmp(&b)),
        (false, true) => {
            if (a.0 + FOURTEEN_HOURS, a.1) < b {
                Some(std::cmp::Ordering::Less)
            } else if (a.0 - FOURTEEN_HOURS, a.1) > b {
                Some(std::cmp::Ordering::Greater)
            } else {
                None
            }
        }
        (true, false) => compare_split(b, b_zoned, a, a_zoned).map(std::cmp::Ordering::reverse),
    }
}

/// Defines `PartialEq` as "the order relation says equal".
///
/// Structural equality is wrong for every temporal type:
/// `2010-09-20T13:00:00+01:00` and `2010-09-20T12:00:00Z` are the same
/// instant, written two ways, and an `xs:enumeration` listing one must admit
/// the other. Two values that are merely *incomparable* — one with a timezone,
/// one without, inside the 28-hour window — are not equal either, which falls
/// out of the same rule.
///
/// `Eq` and `Hash` are deliberately not derived alongside: they would have to
/// agree with this, and a structural hash cannot.
macro_rules! eq_from_order {
    ($($name:ident),* $(,)?) => {
        $(
            impl PartialEq for $name {
                fn eq(&self, other: &Self) -> bool {
                    self.partial_cmp(other) == Some(std::cmp::Ordering::Equal)
                }
            }
        )*
    };
}

eq_from_order!(
    DateTime, Date, Time, GMonthDay, GYearMonth, GYear, GMonth, GDay
);

// ---------------------------------------------------------------------------
// The duration family
// ---------------------------------------------------------------------------

/// The two independent halves of a duration: whole months, and seconds.
///
/// They stay separate because no fixed number of seconds is a month. That is
/// also why two durations are only *partially* ordered — see
/// [`crate::values`], which does the comparing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DurationParts {
    months: i64,
    /// Seconds, scaled the way [`Decimal`] scales.
    seconds: i128,
}

/// Reads `PnYnMnDTnHnMnS`, with every field optional but at least one present.
fn take_duration(s: &str, allow: &str) -> Result<DurationParts, String> {
    let (negative, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let mut rest = rest
        .strip_prefix('P')
        .ok_or_else(|| "a duration starts with `P`".to_string())?;

    let mut months: i128 = 0;
    let mut seconds: i128 = 0;
    let mut seen = 0usize;
    let mut in_time = false;
    // The designators in order, so `P1M1Y` is refused as readily as `P1X`.
    let order = ["Y", "M", "D", "T", "H", "M", "S"];
    let mut next = 0usize;

    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('T') {
            let at = order.iter().position(|d| *d == "T").expect("T is listed");
            if in_time || next > at || after.is_empty() {
                return Err("`T` must come once, before the time fields".into());
            }
            in_time = true;
            next = at + 1;
            rest = after;
            continue;
        }
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return Err("expected a number in the duration".into());
        }
        let (number, tail) = rest.split_at(digits);
        // Only the seconds field may carry a fraction.
        let (frac, tail) = match tail.strip_prefix('.') {
            Some(f) => {
                let n = f.bytes().take_while(u8::is_ascii_digit).count();
                if n == 0 {
                    return Err("the fractional seconds need at least one digit".into());
                }
                (&f[..n], &f[n..])
            }
            None => ("", tail),
        };
        let designator = tail.get(..1).unwrap_or("");
        // `T` separates the date fields from the time ones; it is never a
        // field itself, so a number in front of it is not a duration —
        // `P8TH` names no quantity of anything.
        let at = order
            .iter()
            .skip(next)
            .position(|d| *d == designator && *d != "T")
            .map(|i| i + next)
            .filter(|i| (*i > 3) == in_time)
            .ok_or_else(|| format!("`{designator}` is not a duration field here"))?;
        if !allow.contains(designator) || (!frac.is_empty() && designator != "S") {
            return Err(format!("`{designator}` is not allowed in this duration"));
        }
        let value: i128 = number
            .parse()
            .map_err(|_| "the duration field is too large".to_string())?;

        let scale = |unit: i128| value.checked_mul(unit).ok_or("the duration is too large");
        match at {
            0 => months += scale(12)?,
            1 => months += value,
            2 => seconds += scale(86_400 * Decimal::SCALE)?,
            4 => seconds += scale(3_600 * Decimal::SCALE)?,
            5 => seconds += scale(60 * Decimal::SCALE)?,
            6 => {
                seconds += scale(Decimal::SCALE)?;
                let mut unit = Decimal::SCALE;
                for b in frac.bytes() {
                    if unit == 1 {
                        break;
                    }
                    unit /= 10;
                    seconds += i128::from(b - b'0') * unit;
                }
            }
            _ => unreachable!("`T` is excluded above"),
        }
        seen += 1;
        next = at + 1;
        rest = &tail[1..];
    }

    if seen == 0 {
        return Err("a duration needs at least one field".into());
    }
    if in_time && next <= 4 {
        return Err("`T` must be followed by a time field".into());
    }
    let sign = if negative { -1 } else { 1 };
    Ok(DurationParts {
        months: i64::try_from(months * sign)
            .map_err(|_| "the duration is too large".to_string())?,
        seconds: seconds * sign,
    })
}

/// Writes the canonical form, omitting every field that is zero.
fn write_duration(f: &mut fmt::Formatter<'_>, p: DurationParts) -> fmt::Result {
    if p.months == 0 && p.seconds == 0 {
        // Something has to be written, and the specification picks the
        // seconds.
        return f.write_str("PT0S");
    }
    if p.months < 0 || p.seconds < 0 {
        f.write_str("-")?;
    }
    f.write_str("P")?;
    let (years, months) = (p.months.unsigned_abs() / 12, p.months.unsigned_abs() % 12);
    if years > 0 {
        write!(f, "{years}Y")?;
    }
    if months > 0 {
        write!(f, "{months}M")?;
    }

    let total = p.seconds.unsigned_abs();
    let scale = Decimal::SCALE as u128;
    let whole = total / scale;
    let days = whole / 86_400;
    if days > 0 {
        write!(f, "{days}D")?;
    }
    let rest = whole % 86_400;
    if rest == 0 && total.is_multiple_of(scale) {
        return Ok(());
    }
    f.write_str("T")?;
    let (hours, minutes) = (rest / 3_600, rest % 3_600 / 60);
    if hours > 0 {
        write!(f, "{hours}H")?;
    }
    if minutes > 0 {
        write!(f, "{minutes}M")?;
    }
    if !rest.is_multiple_of(60) || !total.is_multiple_of(scale) {
        // `rest` counts whole seconds and `total` is scaled, so the two
        // remainders are in different units and must be scaled to match.
        let scaled = i128::try_from((rest % 60) * scale + total % scale).unwrap_or(0);
        write_seconds_field(f, scaled)?;
    }
    Ok(())
}

/// The seconds of a duration: no leading zero, and a fraction only when there
/// is one.
fn write_seconds_field(f: &mut fmt::Formatter<'_>, scaled: i128) -> fmt::Result {
    write!(f, "{}", scaled / Decimal::SCALE)?;
    let mut frac = scaled % Decimal::SCALE;
    if frac != 0 {
        let mut digits = String::new();
        let mut unit = Decimal::SCALE;
        while frac != 0 {
            unit /= 10;
            digits.push((b'0' + (frac / unit) as u8) as char);
            frac %= unit;
        }
        write!(f, ".{digits}")?;
    }
    f.write_str("S")
}

/// Defines one of the three duration types over [`DurationParts`].
macro_rules! duration {
    ($(#[$m:meta])* $name:ident, allow: $allow:literal) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct $name(DurationParts);

        impl $name {
            /// Whole years, the largest unit the months split into.
            pub fn years(self) -> i64 {
                self.0.months / 12
            }

            /// Months beyond the whole years, so always below twelve.
            pub fn months(self) -> i64 {
                self.0.months % 12
            }

            pub fn days(self) -> i64 {
                (self.0.seconds / Decimal::SCALE / 86_400) as i64
            }

            pub fn hours(self) -> i64 {
                (self.0.seconds / Decimal::SCALE % 86_400 / 3_600) as i64
            }

            pub fn minutes(self) -> i64 {
                (self.0.seconds / Decimal::SCALE % 3_600 / 60) as i64
            }

            /// Seconds beyond the whole minutes, with the fraction XSD allows.
            pub fn seconds(self) -> Decimal {
                Decimal::from_scaled(
                    self.0.seconds % (60 * Decimal::SCALE),
                )
            }

            /// Whole months, for comparison.
            ///
            /// One of these two is unused on each of the halves — a
            /// yearMonthDuration has no seconds worth naming and a
            /// dayTimeDuration no months — but the macro gives all three the
            /// same surface, and `xs:duration` needs both.
            #[allow(dead_code)]
            pub(crate) fn total_months(self) -> i64 {
                self.0.months
            }

            /// Seconds scaled by 10^18, for comparison.
            #[allow(dead_code)]
            pub(crate) fn total_seconds(self) -> i128 {
                self.0.seconds
            }

            pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
                take_duration(s, $allow).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_duration(f, self.0)
            }
        }
    };
}

duration!(
    /// `xs:duration` — months and seconds, which is why two of them are not
    /// always ordered. `P1M` against `P30D` has no answer until you say which
    /// month.
    Duration,
    allow: "YMDTHS"
);
duration!(
    /// `xs:yearMonthDuration`, the months-only half of a duration.
    YearMonthDuration,
    allow: "YM"
);
duration!(
    /// `xs:dayTimeDuration`, the seconds-only half of a duration.
    DayTimeDuration,
    allow: "DTHMS"
);

// Both halves of a duration are *totally* ordered on their own; only
// `xs:duration`, which carries the two at once, is not.
impl PartialOrd for YearMonthDuration {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.total_months().cmp(&other.total_months()))
    }
}

impl PartialOrd for DayTimeDuration {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.total_seconds().cmp(&other.total_seconds()))
    }
}

// ---------------------------------------------------------------------------
// precisionDecimal
// ---------------------------------------------------------------------------

/// What an [`PrecisionDecimal`] holds.
#[derive(Clone, Copy, Debug)]
enum Pd {
    /// `coefficient × 10^exponent`, with the sign kept apart so that `-0` and
    /// `0` stay distinct values.
    Finite {
        negative: bool,
        coefficient: u128,
        exponent: i32,
    },
    Infinity {
        negative: bool,
    },
    NaN,
}

/// `xs:precisionDecimal`, the optional XSD 1.1 datatype.
///
/// A decimal that remembers how it was written. `1.0` and `1.00` are the same
/// *number* and compare equal, but they are different values: the second has a
/// scale of two, and `minScale`/`maxScale` can tell them apart. That is the
/// whole point of the type — it is IEEE 754's decimal, where the trailing
/// zeroes carry the precision of a measurement.
///
/// It also has what `xs:decimal` does not: infinities, a not-a-number, and a
/// signed zero.
#[derive(Clone, Copy, Debug)]
pub struct PrecisionDecimal(Pd);

impl PrecisionDecimal {
    /// The number of digits in the coefficient, which is what `totalDigits`
    /// counts.
    ///
    /// Written digits, not significant ones: `1.000` has four and `1.0` two,
    /// where an `xs:decimal` would say one of each.
    pub fn total_digits(self) -> Option<u32> {
        match self.0 {
            Pd::Finite { coefficient, .. } => Some(digit_count(coefficient)),
            // A special value has no digits to count, so the facet cannot
            // reject it.
            _ => None,
        }
    }

    /// How many digits sit after the point — the negated exponent.
    ///
    /// Signed: `200` written as `2e2` has a scale of -2.
    pub fn scale(self) -> Option<i32> {
        match self.0 {
            Pd::Finite { exponent, .. } => Some(-exponent),
            _ => None,
        }
    }

    pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
        match s {
            "INF" | "+INF" => return Ok(Self(Pd::Infinity { negative: false })),
            "-INF" => return Ok(Self(Pd::Infinity { negative: true })),
            "NaN" => return Ok(Self(Pd::NaN)),
            _ => {}
        }
        if !is_xsd_numeral(s) {
            return Err("expected a number, `INF`, `-INF` or `NaN`".into());
        }
        let (negative, rest) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        let (mantissa, exp) = match rest.split_once(['e', 'E']) {
            Some((m, e)) => (
                m,
                e.parse::<i32>()
                    .map_err(|_| "the exponent is out of range".to_string())?,
            ),
            None => (rest, 0),
        };
        let (int, frac) = match mantissa.split_once('.') {
            Some((i, f)) => (i, f),
            None => (mantissa, ""),
        };

        let mut coefficient: u128 = 0;
        for b in int.bytes().chain(frac.bytes()) {
            coefficient = coefficient
                .checked_mul(10)
                .and_then(|v| v.checked_add(u128::from(b - b'0')))
                .ok_or("more digits than an xs:precisionDecimal can hold")?;
        }
        // Each fractional digit is one power of ten the exponent has to
        // absorb, which is what turns `1.00` into 100 × 10^-2 and gives the
        // value its scale.
        let exponent = i32::try_from(frac.len())
            .ok()
            .and_then(|f| exp.checked_sub(f))
            .ok_or("the exponent is out of range")?;
        Ok(Self(Pd::Finite {
            negative,
            coefficient,
            exponent,
        }))
    }
}

/// How many decimal digits `n` is written with; zero is one digit.
fn digit_count(n: u128) -> u32 {
    let mut digits = 1;
    let mut rest = n / 10;
    while rest != 0 {
        digits += 1;
        rest /= 10;
    }
    digits
}

impl fmt::Display for PrecisionDecimal {
    /// Keeps the scale, because the scale is part of the value.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (negative, coefficient, exponent) = match self.0 {
            Pd::NaN => return f.write_str("NaN"),
            Pd::Infinity { negative: true } => return f.write_str("-INF"),
            Pd::Infinity { negative: false } => return f.write_str("INF"),
            Pd::Finite {
                negative,
                coefficient,
                exponent,
            } => (negative, coefficient, exponent),
        };
        if negative {
            f.write_str("-")?;
        }
        let digits = coefficient.to_string();
        match exponent {
            // No fractional part, and small enough to write out in full.
            0 => f.write_str(&digits),
            e if e > 0 && e <= 6 => write!(f, "{digits}{}", "0".repeat(e as usize)),
            e if e < 0 && (-e as usize) < digits.len() => {
                let split = digits.len() - (-e as usize);
                write!(f, "{}.{}", &digits[..split], &digits[split..])
            }
            e if e < 0 && (-e) <= 6 => {
                write!(f, "0.{}{digits}", "0".repeat((-e) as usize - digits.len()))
            }
            // Far from the point, where writing it out would be all zeroes.
            e => write!(f, "{digits}E{e}"),
        }
    }
}

impl PartialOrd for PrecisionDecimal {
    /// Numeric order, which ignores the scale: `1.0`, `1.00` and `10e-1` are
    /// one number three ways. `NaN` is ordered against nothing, and the two
    /// zeroes are equal despite their signs.
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering::*;
        let rank = |v: &Self| match v.0 {
            Pd::NaN => None,
            Pd::Infinity { negative: true } => Some(-1i8),
            Pd::Infinity { negative: false } => Some(1),
            Pd::Finite { .. } => Some(0),
        };
        let (a, b) = (rank(self)?, rank(other)?);
        if a != b {
            return Some(a.cmp(&b));
        }
        if a != 0 {
            // Both are the same infinity.
            return Some(Equal);
        }
        let (
            Pd::Finite {
                negative: an,
                coefficient: ac,
                exponent: ae,
            },
            Pd::Finite {
                negative: bn,
                coefficient: bc,
                exponent: be,
            },
        ) = (self.0, other.0)
        else {
            unreachable!("both ranked as finite");
        };
        // Zero has no sign for the purpose of ordering, so `-0` and `0` are
        // equal and both sit between the negatives and the positives.
        match (ac == 0, bc == 0) {
            (true, true) => return Some(Equal),
            (true, false) => return Some(if bn { Greater } else { Less }),
            (false, true) => return Some(if an { Less } else { Greater }),
            (false, false) => {}
        }
        if an != bn {
            return Some(if an { Less } else { Greater });
        }
        let magnitude = compare_magnitude((ac, ae), (bc, be));
        Some(if an { magnitude.reverse() } else { magnitude })
    }
}

/// Compares two non-zero coefficients scaled by their exponents.
///
/// Through the digit strings rather than by scaling one side up: the exponents
/// can differ by enough that any common scaling overflows, and comparison is
/// not hot enough to be worth the risk.
fn compare_magnitude(a: (u128, i32), b: (u128, i32)) -> std::cmp::Ordering {
    let (ad, bd) = (a.0.to_string(), b.0.to_string());
    // Where the number sits, independent of how many digits were written.
    let adjusted = |digits: &str, exponent: i32| exponent + digits.len() as i32 - 1;
    match adjusted(&ad, a.1).cmp(&adjusted(&bd, b.1)) {
        std::cmp::Ordering::Equal => {}
        other => return other,
    }
    // Same magnitude, so the digits line up from the left.
    let width = ad.len().max(bd.len());
    let pad = |s: &str| {
        let mut out = s.to_string();
        out.push_str(&"0".repeat(width - s.len()));
        out
    };
    pad(&ad).cmp(&pad(&bd))
}

impl PartialEq for PrecisionDecimal {
    /// The order relation, so `1.0` equals `1.00` and `NaN` equals nothing.
    fn eq(&self, other: &Self) -> bool {
        self.partial_cmp(other) == Some(std::cmp::Ordering::Equal)
    }
}

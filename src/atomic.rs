//! The atomic value types a [`Value`](crate::values::Value) can hold.
//!
//! Every one of these wraps an `oxsdatatypes` value and forwards to it. The
//! wrapping is the point: `Value` is public, so whatever it holds is part of
//! this crate's API. Exposing the library's types directly would make every
//! consumer add the dependency themselves and pin them to our exact version of
//! it, so a patch bump on our side would be a breaking change on theirs.
//!
//! Behind these, `oxsdatatypes` does the work and does it well — the timezone
//! partial order, `24:00` normalising to the next midnight, `2000-02-30`
//! refused. See `DESIGN.md` §3.15.4 for why keeping it is the right call and
//! what would have to change for that to stop being true.
//!
//! What each type offers is what a consumer actually does with a parsed value:
//! render it canonically ([`std::fmt::Display`]), order it, and take it apart
//! to build something of their own — a `chrono::DateTime`, an Arrow column, a
//! Python `datetime`.

use std::fmt;

/// Defines a wrapper that forwards ordering and canonical rendering.
macro_rules! wrapper {
    ($(#[$m:meta])* $name:ident, $inner:ty) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
        pub struct $name(pub(crate) $inner);

        impl fmt::Display for $name {
            /// The canonical lexical form.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl $name {
            /// Parses a lexical form, reporting the reason as a string.
            ///
            /// Not `FromStr`: that would need a public error type, and the
            /// backend's would put it right back in this crate's API. Callers
            /// want [`crate::values::parse`] anyway, which knows the type.
            pub(crate) fn parse_lexical(s: &str) -> Result<Self, String> {
                <$inner as std::str::FromStr>::from_str(s)
                    .map(Self)
                    .map_err(|e| e.to_string())
            }
        }
    };
}

wrapper!(
    /// `xs:decimal` and the types derived from it that are not integers.
    ///
    /// Exact, not floating point: fixed point with 18 fractional digits.
    Decimal,
    oxsdatatypes::Decimal
);
wrapper!(
    /// `xs:float`, IEEE 754 single precision.
    Float,
    oxsdatatypes::Float
);
wrapper!(
    /// `xs:double`, IEEE 754 double precision.
    Double,
    oxsdatatypes::Double
);
wrapper!(
    /// `xs:duration` — months and seconds, which is why two of them are not
    /// always ordered. `P1M` against `P30D` has no answer until you say which
    /// month.
    Duration,
    oxsdatatypes::Duration
);
wrapper!(
    /// `xs:yearMonthDuration`, the months-only half of a duration.
    YearMonthDuration,
    oxsdatatypes::YearMonthDuration
);
wrapper!(
    /// `xs:dayTimeDuration`, the seconds-only half of a duration.
    DayTimeDuration,
    oxsdatatypes::DayTimeDuration
);
wrapper!(
    /// `xs:dateTime`, and `xs:dateTimeStamp` which additionally requires the
    /// timezone to be present.
    DateTime,
    oxsdatatypes::DateTime
);
wrapper!(
    /// `xs:time`.
    Time,
    oxsdatatypes::Time
);
wrapper!(
    /// `xs:date`.
    Date,
    oxsdatatypes::Date
);
wrapper!(
    /// `xs:gYearMonth`, a month of a particular year.
    GYearMonth,
    oxsdatatypes::GYearMonth
);
wrapper!(
    /// `xs:gYear`.
    GYear,
    oxsdatatypes::GYear
);
wrapper!(
    /// `xs:gDay`, a day recurring every month.
    GDay,
    oxsdatatypes::GDay
);
wrapper!(
    /// `xs:gMonth`, a month recurring every year.
    GMonth,
    oxsdatatypes::GMonth
);

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

impl TimezoneOffset {
    /// Deliberately not a `From` impl: that would name the backend's type in
    /// a public trait bound, which is the leak this module exists to close.
    fn wrap(tz: oxsdatatypes::TimezoneOffset) -> Self {
        Self(i16::from_be_bytes(tz.to_be_bytes()))
    }
}

/// Generates the shared accessors of a date-and-or-time type.
macro_rules! date_parts {
    ($name:ident $(, $part:ident : $ty:ty)*) => {
        impl $name {
            $(
                pub fn $part(self) -> $ty {
                    self.0.$part()
                }
            )*

            /// The offset from UTC, if the value carries one.
            pub fn timezone_offset(self) -> Option<TimezoneOffset> {
                self.0.timezone_offset().map(TimezoneOffset::wrap)
            }
        }
    };
}

date_parts!(DateTime, year: i64, month: u8, day: u8, hour: u8, minute: u8);
date_parts!(Date, year: i64, month: u8, day: u8);
date_parts!(Time, hour: u8, minute: u8);
date_parts!(GYearMonth, year: i64, month: u8);
date_parts!(GYear, year: i64);
date_parts!(GDay, day: u8);
date_parts!(GMonth, month: u8);

impl DateTime {
    /// Seconds within the minute, which XSD allows a fractional part.
    pub fn second(self) -> Decimal {
        Decimal(self.0.second())
    }
}

impl Time {
    /// Seconds within the minute, which XSD allows a fractional part.
    pub fn second(self) -> Decimal {
        Decimal(self.0.second())
    }
}

/// Generates the accessors of a duration type.
///
/// The components are *normalised*: everything below the leading one is
/// bounded, and they share one sign, so recombining them recovers the total
/// exactly.
macro_rules! duration_parts {
    ($name:ident $(, $part:ident : $ty:ty)*) => {
        impl $name {
            $(
                pub fn $part(self) -> $ty {
                    self.0.$part()
                }
            )*
        }
    };
}

duration_parts!(
    Duration,
    years: i64,
    months: i64,
    days: i64,
    hours: i64,
    minutes: i64
);
duration_parts!(YearMonthDuration, years: i64, months: i64);
duration_parts!(DayTimeDuration, days: i64, hours: i64, minutes: i64);

impl Duration {
    /// Seconds, with the fractional part XSD allows.
    pub fn seconds(self) -> Decimal {
        Decimal(self.0.seconds())
    }
}

impl DayTimeDuration {
    /// Seconds, with the fractional part XSD allows.
    pub fn seconds(self) -> Decimal {
        Decimal(self.0.seconds())
    }
}

impl Decimal {
    /// The nearest decimal to an integer, absent when it does not fit.
    pub(crate) fn from_integer(n: i128) -> Option<Self> {
        oxsdatatypes::Decimal::try_from(n).ok().map(Self)
    }

    /// The value scaled by 10^18, which is how it is held.
    ///
    /// Exact, and the way to convert into a decimal type of your own without
    /// going through a string.
    pub fn to_i128_scaled(self) -> i128 {
        i128::from_be_bytes(self.0.to_be_bytes())
    }

    /// The scale the fixed-point representation uses: 10^18.
    pub const SCALE: i128 = 1_000_000_000_000_000_000;
}

impl From<Float> for f32 {
    fn from(v: Float) -> f32 {
        f32::from(v.0)
    }
}

impl From<Double> for f64 {
    fn from(v: Double) -> f64 {
        f64::from(v.0)
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
        if rest.len() < 5 || rest.as_bytes()[2] != b'-' {
            return Err("expected `--MM-DD`".into());
        }
        let (mm, tail) = rest.split_at(2);
        let (dd, tz) = tail[1..].split_at(2.min(tail.len() - 1));
        let two = |v: &str, what: &str| -> Result<u8, String> {
            if v.len() == 2 && v.bytes().all(|b| b.is_ascii_digit()) {
                Ok(v.parse().expect("two ascii digits"))
            } else {
                Err(format!("{what} must be two digits"))
            }
        };
        let month = two(mm, "the month")?;
        let day = two(dd, "the day")?;
        if !(1..=12).contains(&month) {
            return Err(format!("{month} is not a month"));
        }
        if day < 1 || day > Self::max_day(month) {
            return Err(format!("{day} is not a day of month {month}"));
        }
        Ok(Self {
            month,
            day,
            offset: parse_offset(tz)?,
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
    let sign = match s.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return Err("expected a timezone such as `Z` or `+02:00`".into()),
    };
    let body = &s[1..];
    if body.len() != 5 || body.as_bytes()[2] != b':' {
        return Err("expected a timezone such as `Z` or `+02:00`".into());
    }
    let hh: i16 = body[..2]
        .parse()
        .map_err(|_| "the timezone hour must be two digits".to_string())?;
    let mm: i16 = body[3..]
        .parse()
        .map_err(|_| "the timezone minute must be two digits".to_string())?;
    let total = sign * (hh * 60 + mm);
    if !(-840..=840).contains(&total) || mm > 59 {
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

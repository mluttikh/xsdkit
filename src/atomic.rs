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
    /// `xs:gMonthDay`, a day of a month recurring every year.
    GMonthDay,
    oxsdatatypes::GMonthDay
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
date_parts!(GMonthDay, month: u8, day: u8);
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

//! Canonical forms and orderings, for every built-in that has them.
//!
//! This is the half of the value layer fuzzing cannot judge. Ten million runs
//! establish that nothing panics; none of them can say whether `--02-29` is a
//! date or whether two durations are ordered the right way round. Every
//! expectation here is from the datatypes specification, not from recording
//! what the code happened to print.

use xsdkit::Version;
use xsdkit::datatypes::Builtin as B;
use xsdkit::values::{parse, parse_in};

#[track_caller]
fn canonical(b: B, lexical: &str) -> String {
    parse(b, lexical)
        .unwrap_or_else(|e| panic!("{b} rejected {lexical:?}: {}", e.reason))
        .to_string()
}

#[track_caller]
fn rejects(b: B, lexical: &str) {
    assert!(
        parse(b, lexical).is_err(),
        "{b} should reject {lexical:?}, got {}",
        canonical(b, lexical)
    );
}

/// Parsing the canonical form again must give the same value — otherwise the
/// canonical form is not in the type's own lexical space.
#[track_caller]
fn round_trips(b: B, lexical: &str) {
    let once = canonical(b, lexical);
    let twice = canonical(b, &once);
    assert_eq!(once, twice, "{b} does not round-trip from {lexical:?}");
}

#[test]
fn numeric_canonical_forms() {
    // Insignificant zeroes and a leading `+` are lexical, not part of the
    // value.
    assert_eq!(canonical(B::Decimal, "-0.500"), "-0.5");
    assert_eq!(canonical(B::Decimal, "+1.5"), "1.5");
    assert_eq!(canonical(B::Decimal, ".5"), "0.5");
    assert_eq!(canonical(B::Integer, "+007"), "7");
    assert_eq!(canonical(B::Int, "-0"), "0");
    // A decimal has no exponent; that spelling belongs to float and double.
    rejects(B::Decimal, "1E3");
    rejects(B::Integer, "1.0");

    assert_eq!(canonical(B::Double, "1.5E3"), canonical(B::Double, "1500"));
    assert_eq!(canonical(B::Double, "NaN"), "NaN");
    assert_eq!(canonical(B::Double, "-INF"), "-INF");
    assert_eq!(canonical(B::Boolean, "1"), "true");
    assert_eq!(canonical(B::Boolean, "0"), "false");

    for (b, s) in [
        (B::Decimal, "-0.5"),
        (B::Double, "1.5E3"),
        (B::Float, "0.1"),
        (B::Integer, "-42"),
    ] {
        round_trips(b, s);
    }
}

#[test]
fn temporal_canonical_forms() {
    // `24:00` is midnight ending the day, so it normalises to the next one.
    assert_eq!(
        canonical(B::DateTime, "2024-01-01T24:00:00"),
        "2024-01-02T00:00:00"
    );
    assert_eq!(canonical(B::Time, "24:00:00"), "00:00:00");
    // Trailing zeroes in the seconds are not part of the value.
    assert_eq!(
        canonical(B::DateTime, "2024-01-01T00:00:00.000"),
        "2024-01-01T00:00:00"
    );
    // The timezone is part of the value and survives.
    assert_eq!(
        canonical(B::DateTime, "2024-02-29T13:45:06.5+02:00"),
        "2024-02-29T13:45:06.5+02:00"
    );
    assert_eq!(canonical(B::Date, "2024-03-01+01:00"), "2024-03-01+01:00");
    // A duration's components are normalised, carrying up where they can.
    assert_eq!(canonical(B::YearMonthDuration, "P14M"), "P1Y2M");
    assert_eq!(canonical(B::DayTimeDuration, "PT36H"), "P1DT12H");
    assert_eq!(canonical(B::Duration, "-P1D"), "-P1D");

    for (b, s) in [
        (B::DateTime, "2024-02-29T13:45:06.5+02:00"),
        (B::Date, "-0001-01-01"),
        (B::Time, "13:45:06.5"),
        (B::GYearMonth, "2024-02"),
        (B::GYear, "-0044"),
        (B::GDay, "---05"),
        (B::GMonth, "--02"),
        (B::Duration, "P1Y2M3DT4H5M6.5S"),
    ] {
        round_trips(b, s);
    }

    // The calendar is checked, and 2024 is a leap year while 2023 is not.
    assert_eq!(canonical(B::Date, "2024-02-29"), "2024-02-29");
    rejects(B::Date, "2023-02-29");
    rejects(B::Date, "2024-02-30");
    // XSD has no leap seconds.
    rejects(B::DateTime, "2024-01-01T00:00:60");
}

/// A `gMonthDay` names no year, so February has 29 days in it — the day
/// exists, just not every year. The backend rejected `--02-29`, which is why
/// this type is implemented here.
#[test]
fn a_gmonthday_admits_the_leap_day() {
    assert_eq!(canonical(B::GMonthDay, "--02-29"), "--02-29");
    round_trips(B::GMonthDay, "--02-29");
    // But the months that never have 30 or 31 days still say so.
    rejects(B::GMonthDay, "--02-30");
    rejects(B::GMonthDay, "--04-31");
    assert_eq!(canonical(B::GMonthDay, "--01-31"), "--01-31");

    // Timezones, and the bounds on them.
    assert_eq!(canonical(B::GMonthDay, "--12-25Z"), "--12-25Z");
    assert_eq!(canonical(B::GMonthDay, "--12-25+02:00"), "--12-25+02:00");
    assert_eq!(canonical(B::GMonthDay, "--12-25-05:00"), "--12-25-05:00");
    rejects(B::GMonthDay, "--12-25+15:00");

    // Shape.
    rejects(B::GMonthDay, "--00-10");
    rejects(B::GMonthDay, "--13-01");
    rejects(B::GMonthDay, "--1-1");
    rejects(B::GMonthDay, "02-29");
}

/// Order is a property of the value space, so it ignores how the value was
/// written — and a value with no timezone names a 28-hour-wide window, which
/// is why it is sometimes ordered against nothing at all.
#[test]
fn orderings_are_in_the_value_space() {
    use std::cmp::Ordering::*;

    let cmp = |b: B, x: &str, y: &str| {
        parse(b, x)
            .unwrap()
            .partial_cmp_value(&parse(b, y).unwrap())
    };

    // Lexically different, the same value.
    assert_eq!(cmp(B::Decimal, "1.0", "1.00"), Some(Equal));
    assert_eq!(cmp(B::Integer, "+7", "7"), Some(Equal));
    assert_eq!(cmp(B::Decimal, "-0.5", "0.5"), Some(Less));
    // Across the decimal and integer families.
    assert_eq!(cmp(B::Double, "1.5E3", "1500"), Some(Equal));
    // NaN is ordered against nothing, itself included.
    assert_eq!(cmp(B::Double, "NaN", "NaN"), None);
    assert_eq!(cmp(B::Double, "NaN", "1"), None);
    assert_eq!(cmp(B::Double, "-INF", "INF"), Some(Less));

    // Temporal, in and out of a timezone.
    assert_eq!(
        cmp(
            B::DateTime,
            "2024-01-01T12:00:00Z",
            "2024-01-01T13:00:00+01:00"
        ),
        Some(Equal),
        "the same instant, written two ways"
    );
    assert_eq!(cmp(B::Date, "2024-01-01", "2024-01-02"), Some(Less));
    assert_eq!(
        cmp(B::DateTime, "2024-01-01T12:00:00", "2024-01-01T12:00:00Z"),
        None,
        "no timezone: the window straddles the other value"
    );
    assert_eq!(
        cmp(B::DateTime, "2024-01-01T12:00:00", "2025-01-01T12:00:00Z"),
        Some(Less),
        "a year apart is outside the window"
    );

    assert_eq!(cmp(B::GMonthDay, "--01-01", "--01-02"), Some(Less));
    assert_eq!(cmp(B::GMonthDay, "--12-25Z", "--12-26Z"), Some(Less));
    assert_eq!(cmp(B::GMonthDay, "--12-25", "--12-25Z"), None);
}

/// The two lexical spaces XSD 1.1 widened, checked at the value layer rather
/// than through a schema.
#[test]
fn the_version_decides_two_lexical_spaces() {
    assert!(parse_in(B::Double, "+INF", Version::Xsd11).is_ok());
    assert!(parse_in(B::Double, "+INF", Version::Xsd10).is_err());
    assert!(parse_in(B::Date, "0000-01-01", Version::Xsd11).is_ok());
    assert!(parse_in(B::Date, "0000-01-01", Version::Xsd10).is_err());
    // The bare `parse` reads the superset.
    assert!(parse(B::Double, "+INF").is_ok());
}

/// Every built-in must render something that parses back as the same type.
/// This is the check that catches a canonical form leaking a debug shape.
#[test]
fn every_builtin_round_trips_its_canonical_form() {
    let samples: &[(B, &str)] = &[
        (B::String, "hello"),
        (B::NormalizedString, "a b"),
        (B::Token, "a b"),
        (B::Language, "en-GB"),
        (B::NmToken, "abc"),
        (B::Name, "a.b-c"),
        (B::NcName, "abc"),
        (B::Id, "x1"),
        (B::IdRef, "x1"),
        (B::Entity, "e1"),
        (B::Boolean, "true"),
        (B::Decimal, "1.5"),
        (B::Integer, "1"),
        (B::Long, "1"),
        (B::Int, "1"),
        (B::Short, "1"),
        (B::Byte, "1"),
        (B::NonNegativeInteger, "1"),
        (B::UnsignedLong, "1"),
        (B::PositiveInteger, "1"),
        (B::Float, "1.5"),
        (B::Double, "1.5"),
        (B::Duration, "P1D"),
        (B::YearMonthDuration, "P1Y"),
        (B::DayTimeDuration, "PT1H"),
        (B::DateTime, "2024-01-01T00:00:00"),
        (B::DateTimeStamp, "2024-01-01T00:00:00Z"),
        (B::Time, "00:00:00"),
        (B::Date, "2024-01-01"),
        (B::GYearMonth, "2024-01"),
        (B::GYear, "2024"),
        (B::GMonthDay, "--02-29"),
        (B::GDay, "---05"),
        (B::GMonth, "--02"),
        (B::HexBinary, "0FB7"),
        (B::Base64Binary, "aGVsbG8="),
        (B::AnyUri, "http://x/y?a=1"),
        (B::NmTokens, "a b"),
        (B::IdRefs, "a b"),
        (B::Entities, "a b"),
    ];
    for (b, s) in samples {
        round_trips(*b, s);
        assert_eq!(
            canonical(*b, s),
            *s,
            "{b} changed a value already canonical"
        );
    }
}

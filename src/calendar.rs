//! Calendar dates for the v0.4 `date` type (D-062): days since
//! 1970-01-01 in the proleptic Gregorian calendar, from 0001-01-01 to
//! 9999-12-31.
//!
//! Depends on: nothing (leaf module).
//!
//! Invariants:
//! - `parse` accepts exactly `YYYY-MM-DD` with a real day of that month
//!   (leap years included) inside the range; `format` is its inverse;
//! - conversions use Howard Hinnant's `days_from_civil` and
//!   `civil_from_days`, exact for every date in the range.

/// Day number of 0001-01-01.
pub const MIN: i64 = -719_162;
/// Day number of 9999-12-31.
pub const MAX: i64 = 2_932_896;

/// Day number of a civil date (any year; no range check).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date `(year, month, day)` of a day number.
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        2 => 28,
        _ => 0,
    }
}

pub fn in_range(days: i64) -> bool {
    (MIN..=MAX).contains(&days)
}

/// Parse `YYYY-MM-DD`; `None` if malformed, impossible, or out of range.
pub fn parse(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = &s[r];
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let (y, m, d) = (i64::from(num(0..4)?), num(5..7)?, num(8..10)?);
    if y < 1 || !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// `YYYY-MM-DD`.
pub fn format(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 1 = Monday … 7 = Sunday (1970-01-01 was a Thursday).
pub fn weekday(days: i64) -> u32 {
    ((days + 3).rem_euclid(7) + 1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_ends_and_round_trip() {
        assert_eq!(parse("0001-01-01"), Some(MIN));
        assert_eq!(parse("9999-12-31"), Some(MAX));
        assert_eq!(parse("1970-01-01"), Some(0));
        for d in [MIN, -1, 0, 1, 19_000, 20_759, MAX] {
            assert_eq!(parse(&format(d)), Some(d));
        }
    }

    #[test]
    fn invalid_dates_are_rejected() {
        for s in [
            "2026-02-29",
            "2026-02-30",
            "2026-13-01",
            "0000-01-01",
            "2026-1-01",
            "2026/01/01",
            "2026-01-0a",
            "",
        ] {
            assert_eq!(parse(s), None, "{s}");
        }
        assert!(parse("2024-02-29").is_some());
        assert!(parse("2000-02-29").is_some());
        assert!(parse("1900-02-29").is_none());
    }

    #[test]
    fn weekdays_and_differences_match_the_hand_worked_examples() {
        let d = |s| parse(s).expect("valid");
        assert_eq!(weekday(d("2026-11-02")), 1);
        assert_eq!(weekday(d("2026-01-01")), 4);
        assert_eq!(weekday(d("2026-10-31")), 6);
        assert_eq!(weekday(d("2024-03-01")), 5);
        assert_eq!(d("2026-11-02") - d("2026-01-01"), 305);
        assert_eq!(d("2026-10-10") - d("2026-08-01"), 70);
        assert_eq!(d("2024-03-01") - d("2024-02-28"), 2);
    }
}

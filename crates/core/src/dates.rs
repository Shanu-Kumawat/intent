use jiff::civil::Weekday;
use jiff::ToSpan;
use std::fmt;

use crate::error::{bad_value, CoreError, Result};

/// A local calendar date. Newtype over `jiff::civil::Date` so the serde
/// representation is exactly `YYYY-MM-DD` (jiff's default) and so intent-core
/// callers never depend on jiff directly except through this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date(pub jiff::civil::Date);

impl Date {
    pub fn to_string(self) -> String {
        self.0.to_string()
    }

    pub fn add_days(self, days: i64) -> Date {
        Date(self.0.checked_add(days.days()).expect("date arithmetic overflow"))
    }

    pub fn add_months(self, months: i64) -> Date {
        Date(self.0.checked_add(months.months()).expect("date arithmetic overflow"))
    }

    pub fn add_years(self, years: i64) -> Date {
        Date(self.0.checked_add(years.years()).expect("date arithmetic overflow"))
    }

    pub fn days_until(self, other: Date) -> i64 {
        (other.0 - self.0).get_days() as i64
    }

    pub fn weekday(self) -> Weekday {
        self.0.weekday()
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for Date {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for Date {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        parse_iso(&s).map_err(serde::de::Error::custom)
    }
}

/// Current local calendar date.
pub fn today() -> Date {
    Date(jiff::Zoned::now().date())
}

fn parse_iso(s: &str) -> std::result::Result<Date, jiff::Error> {
    // Strictly YYYY-MM-DD (SPEC §1.4) — reject jiff's lenient/basic forms.
    let s = s.trim();
    let bytes = s.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..].iter().all(u8::is_ascii_digit);
    if !shaped {
        return Err(any_jiff_error());
    }
    s.parse::<jiff::civil::Date>().map(Date)
}

fn any_jiff_error() -> jiff::Error {
    "x".parse::<jiff::civil::Date>().unwrap_err()
}

fn next_weekday(from: Date, target: Weekday) -> Date {
    let mut d = from;
    for _ in 0..7 {
        if d.weekday() == target {
            return d;
        }
        d = d.add_days(1);
    }
    unreachable!("every weekday occurs within 7 days")
}

fn weekday_from_name(name: &str) -> Option<Weekday> {
    Some(match name.to_ascii_lowercase().as_str() {
        "mon" | "monday" => Weekday::Monday,
        "tue" | "tues" | "tuesday" => Weekday::Tuesday,
        "wed" | "weds" | "wednesday" => Weekday::Wednesday,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thursday,
        "fri" | "friday" => Weekday::Friday,
        "sat" | "saturday" => Weekday::Saturday,
        "sun" | "sunday" => Weekday::Sunday,
        _ => return None,
    })
}

/// Parse a date in any of the forms listed in SPEC §1.4:
/// `YYYY-MM-DD`, `today`/`tod`, `tomorrow`/`tom`/`tmr`, `yesterday`,
/// `±N[d|w|m|y]`, weekday names (next occurrence, today included).
pub fn parse_date(input: &str, today: Date) -> Result<Date> {
    let s = input.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err(CoreError::BadDate("empty date".into()));
    }

    if let Ok(d) = parse_iso(&s) {
        return Ok(d);
    }

    match s.as_str() {
        "today" | "tod" => return Ok(today),
        "tomorrow" | "tom" | "tmr" => return Ok(today.add_days(1)),
        "yesterday" => return Ok(today.add_days(-1)),
        _ => {}
    }

    // Relative ±N[d|w|m|y]. Bare sign = 1. Empty rest means no digits.
    if let Some(rest) = s.strip_prefix('+').or_else(|| s.strip_prefix('-')) {
        let negative = s.starts_with('-');
        let (n, unit) = match rest.chars().next() {
            None => (1i64, 'd'),
            Some(u @ ('d' | 'w' | 'm' | 'y')) if rest.len() == 1 => (1i64, u),
            _ => {
                let split = rest
                    .find(|c: char| c.is_ascii_alphabetic())
                    .unwrap_or(rest.len());
                let (digits, unit) = rest.split_at(split);
                let unit = if unit.is_empty() { "d" } else { unit };
                match (digits.parse::<i64>(), unit) {
                    (Ok(n), "d") => (n, 'd'),
                    (Ok(n), "w") => (n, 'w'),
                    (Ok(n), "m") => (n, 'm'),
                    (Ok(n), "y") => (n, 'y'),
                    _ => return Err(CoreError::BadDate(format!("unrecognized date: {input}"))),
                }
            }
        };
        let n = if negative { -n } else { n };
        return Ok(match unit {
            'd' => today.add_days(n),
            'w' => today.add_days(n * 7),
            'm' => today.add_months(n),
            'y' => today.add_years(n),
            _ => unreachable!(),
        });
    }

    if let Some(wd) = weekday_from_name(&s) {
        return Ok(next_weekday(today, wd));
    }

    Err(CoreError::BadDate(format!(
        "unrecognized date: {input} (expected YYYY-MM-DD, today/tomorrow, ±N[d|w|m|y], or a weekday)"
    )))
}

/// True when `input` unambiguously means "clear this date" where allowed.
pub fn is_clear_marker(input: &str) -> bool {
    matches!(input.trim().to_ascii_lowercase().as_str(), "none" | "-" | "")
}

pub fn parse_priority(input: &str) -> Result<crate::model::Priority> {
    crate::model::Priority::parse(input).ok_or_else(|| {
        bad_value(format!(
            "invalid priority {input:?}: expected none, low, medium, or high"
        ))
    })
}

pub fn parse_status(input: &str) -> Result<crate::model::Status> {
    crate::model::Status::parse(input).ok_or_else(|| {
        bad_value(format!(
            "invalid status {input:?}: expected active, done, or archived"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn t() -> Date {
        // 2026-10-26 is a Monday.
        Date(date(2026, 10, 26))
    }

    #[test]
    fn iso_dates() {
        assert_eq!(parse_date("2026-11-02", t()).unwrap(), Date(date(2026, 11, 2)));
        assert_eq!(parse_date(" 2026-11-02 ", t()).unwrap(), Date(date(2026, 11, 2)));
        assert!(parse_date("2026-13-02", t()).is_err());
        assert!(parse_date("20261102", t()).is_err());
    }

    #[test]
    fn words() {
        assert_eq!(parse_date("today", t()).unwrap(), t());
        assert_eq!(parse_date("TODAY", t()).unwrap(), t());
        assert_eq!(parse_date("tod", t()).unwrap(), t());
        assert_eq!(parse_date("tomorrow", t()).unwrap(), t().add_days(1));
        assert_eq!(parse_date("tom", t()).unwrap(), t().add_days(1));
        assert_eq!(parse_date("tmr", t()).unwrap(), t().add_days(1));
        assert_eq!(parse_date("yesterday", t()).unwrap(), t().add_days(-1));
    }

    #[test]
    fn relative() {
        assert_eq!(parse_date("+3", t()).unwrap(), t().add_days(3));
        assert_eq!(parse_date("+3d", t()).unwrap(), t().add_days(3));
        assert_eq!(parse_date("-2", t()).unwrap(), t().add_days(-2));
        assert_eq!(parse_date("+2w", t()).unwrap(), t().add_days(14));
        assert_eq!(parse_date("+1m", t()).unwrap(), Date(date(2026, 11, 26)));
        assert_eq!(parse_date("+1y", t()).unwrap(), Date(date(2027, 10, 26)));
        assert_eq!(parse_date("+", t()).unwrap(), t().add_days(1));
        assert!(parse_date("+3x", t()).is_err());
        assert!(parse_date("++", t()).is_err());
    }

    #[test]
    fn weekdays() {
        // 2026-10-26 is Monday: mon = today, tue = tomorrow, sun = +6.
        assert_eq!(parse_date("mon", t()).unwrap(), t());
        assert_eq!(parse_date("Monday", t()).unwrap(), t());
        assert_eq!(parse_date("tue", t()).unwrap(), t().add_days(1));
        assert_eq!(parse_date("sun", t()).unwrap(), t().add_days(6));
    }

    #[test]
    fn errors_and_clears() {
        assert!(parse_date("next fortnight", t()).is_err());
        assert!(parse_date("", t()).is_err());
        assert!(is_clear_marker("none"));
        assert!(is_clear_marker("-"));
        assert!(!is_clear_marker("tomorrow"));
    }

    #[test]
    fn serde_round_trip() {
        let d = Date(date(2026, 10, 26));
        let j = serde_json::to_string(&d).unwrap();
        assert_eq!(j, "\"2026-10-26\"");
        assert_eq!(serde_json::from_str::<Date>(&j).unwrap(), d);
    }
}

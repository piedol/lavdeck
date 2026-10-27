//! JSON and dates written the way Foundation writes them, so files Lavdeck leaves on disk
//! (`.take.json`, plugin requests, `lavdeck status --json`) read the same as the Swift app's.
//!
//! Foundation differs from serde_json in a few visible ways: pretty output uses `"key" : value`,
//! empty containers span a blank line, `/` is escaped, and a whole `Double` drops its `.0`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serializer};
use serde_json::{Map, Number, Value};

/// `JSONEncoder` with `[.prettyPrinted, .sortedKeys]`, or `JSONSerialization` with the same options.
pub fn to_pretty_sorted(value: &Value) -> String {
    let mut out = String::new();
    write_pretty(value, 0, &mut out);
    out
}

/// `JSONEncoder` with default options: no whitespace, keys in the order given.
pub fn to_compact(value: &Value) -> String {
    let mut out = String::new();
    write_compact(value, &mut out);
    out
}

fn write_pretty(value: &Value, depth: usize, out: &mut String) {
    let indent = |out: &mut String, depth: usize| out.extend(std::iter::repeat_n("  ", depth));
    match value {
        Value::Array(items) => {
            out.push_str("[\n");
            if items.is_empty() {
                out.push('\n');
            }
            for (i, item) in items.iter().enumerate() {
                indent(out, depth + 1);
                write_pretty(item, depth + 1, out);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str("{\n");
            if map.is_empty() {
                out.push('\n');
            }
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.iter().enumerate() {
                indent(out, depth + 1);
                write_string(key, out);
                out.push_str(" : ");
                write_pretty(&map[*key], depth + 1, out);
                out.push_str(if i + 1 < keys.len() { ",\n" } else { "\n" });
            }
            indent(out, depth);
            out.push('}');
        }
        scalar => write_compact(scalar, out),
    }
}

fn write_compact(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_compact(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_compact(item, out);
            }
            out.push('}');
        }
    }
}

fn number(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let text = swift_double(n.as_f64().unwrap_or(0.0));
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Swift's `Double.description`: shortest round-trip digits, positional between 1e-4 and 1e16,
/// otherwise `1e-05` / `1.5e+16`. Whole values keep `.0`.
pub fn swift_double(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf".into() } else { "inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    // Rust's `{:e}` is also the shortest round-trip form, e.g. `-1.2345e4`.
    let sci = format!("{:e}", x.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let sign = if x < 0.0 { "-" } else { "" };
    if !(-4..16).contains(&exponent) {
        let rest = &digits[1..];
        let fraction = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let exp_sign = if exponent < 0 { '-' } else { '+' };
        return format!(
            "{sign}{}{fraction}e{exp_sign}{:02}",
            &digits[..1],
            exponent.abs()
        );
    }
    if exponent >= 0 {
        let whole_len = exponent as usize + 1;
        let (whole, fraction) = if digits.len() > whole_len {
            (
                digits[..whole_len].to_string(),
                digits[whole_len..].to_string(),
            )
        } else {
            (format!("{digits:0<whole_len$}"), "0".to_string())
        };
        format!("{sign}{whole}.{fraction}")
    } else {
        let zeros = "0".repeat((-exponent - 1) as usize);
        format!("{sign}0.{zeros}{digits}")
    }
}

// MARK: Dates

/// A date as Foundation's `.iso8601` strategy writes it: UTC, whole seconds, e.g. `2026-10-11T17:00:00Z`.
pub fn format_iso8601(date: SystemTime) -> String {
    let seconds = seconds_since_epoch(date).floor() as i64;
    let days = seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// Parses what Foundation's `.iso8601` strategy accepts: `YYYY-MM-DDTHH:MM:SS`, optional fractional
/// seconds, then `Z`, `±HH:MM` or `±HHMM`.
pub fn parse_iso8601(text: &str) -> Option<SystemTime> {
    let b = text.as_bytes();
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let part = b.get(range)?;
        part.iter()
            .all(u8::is_ascii_digit)
            .then(|| part.iter().fold(0, |acc, d| acc * 10 + (d - b'0') as i64))
    };
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut i = 19;
    let mut fraction = 0.0;
    if b[i] == b'.' {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        fraction = text[start - 1..i].parse::<f64>().ok()?;
    }
    let offset = match b.get(i)? {
        b'Z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') => {
            let rest = &text[i + 1..];
            let (h, m) = match rest.len() {
                5 if rest.as_bytes()[2] == b':' => (num(i + 1..i + 3)?, num(i + 4..i + 6)?),
                4 => (num(i + 1..i + 3)?, num(i + 3..i + 5)?),
                _ => return None,
            };
            let minutes = h * 60 + m;
            if *sign == b'+' {
                minutes * 60
            } else {
                -minutes * 60
            }
        }
        _ => return None,
    };
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    Some(date_from_seconds(seconds as f64 + fraction))
}

/// Seconds since 1970, negative before it.
pub fn seconds_since_epoch(date: SystemTime) -> f64 {
    match date.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

pub fn date_from_seconds(seconds: f64) -> SystemTime {
    if seconds >= 0.0 {
        UNIX_EPOCH + Duration::from_secs_f64(seconds)
    } else {
        UNIX_EPOCH - Duration::from_secs_f64(-seconds)
    }
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

// Howard Hinnant's civil-date algorithms.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (
        if m <= 2 {
            yoe + era * 400 + 1
        } else {
            yoe + era * 400
        },
        m,
        d,
    )
}

// MARK: serde helpers

/// `#[serde(with = "json::date")]` for a required date in Foundation's ISO 8601 form.
pub mod date {
    use super::*;

    pub fn serialize<S: Serializer>(date: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format_iso8601(*date))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let text = String::deserialize(d)?;
        parse_iso8601(&text).ok_or_else(|| serde::de::Error::custom(format!("bad date {text}")))
    }
}

/// `#[serde(with = "json::optional_date")]` for an optional date.
pub mod optional_date {
    use super::*;

    pub fn serialize<S: Serializer>(date: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match date {
            Some(date) => s.serialize_str(&format_iso8601(*date)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(text) => parse_iso8601(&text)
                .map(Some)
                .ok_or_else(|| serde::de::Error::custom(format!("bad date {text}"))),
            None => Ok(None),
        }
    }
}

/// Swift decodes `Int` from any whole JSON number, including `1.0`.
pub mod int {
    use super::*;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        whole(&Number::deserialize(d)?)
            .ok_or_else(|| serde::de::Error::custom("expected a whole number"))
    }

    pub(crate) fn whole(n: &Number) -> Option<i64> {
        n.as_i64().or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && f.abs() < 9.2e18)
                .map(|f| f as i64)
        })
    }
}

/// [`int`] for an optional field.
pub mod optional_int {
    use super::*;

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        match Option::<Number>::deserialize(d)? {
            Some(n) => int::whole(&n)
                .map(Some)
                .ok_or_else(|| serde::de::Error::custom("expected a whole number")),
            None => Ok(None),
        }
    }
}

/// Serializes `value` to a JSON tree (struct fields in declaration order).
pub(crate) fn tree<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Object(Map::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn writes_like_foundation() {
        let value = json!({"c": 42.0, "a": [], "b": {}, "d": 1e-5, "e": "a/b \"q\" \u{1}\t\n·é",
                           "f": 1e16, "g": 0.1, "w": {"a": 1, "b": [1, 2]}, "z": null});
        assert_eq!(
            to_pretty_sorted(&value),
            "{\n  \"a\" : [\n\n  ],\n  \"b\" : {\n\n  },\n  \"c\" : 42,\n  \"d\" : 1e-05,\n  \
             \"e\" : \"a\\/b \\\"q\\\" \\u0001\\t\\n·é\",\n  \"f\" : 1e+16,\n  \"g\" : 0.1,\n  \
             \"w\" : {\n    \"a\" : 1,\n    \"b\" : [\n      1,\n      2\n    ]\n  },\n  \"z\" : null\n}"
        );
        assert_eq!(
            to_compact(&json!({"type": "stop", "protocol": 2})),
            r#"{"type":"stop","protocol":2}"#
        );
    }

    #[test]
    fn doubles_print_like_swift() {
        for (x, text) in [
            (1e-5, "1e-05"),
            (1e16, "1e+16"),
            (0.0001, "0.0001"),
            (123456789012345678.0, "1.2345678901234568e+17"),
            (5.0, "5.0"),
            (-0.0, "-0.0"),
            (1e15, "1000000000000000.0"),
            (11.428282976150513, "11.428282976150513"),
            (2402638.0179791665, "2402638.0179791665"),
            (-0.29029862536117435, "-0.29029862536117435"),
            (0.5, "0.5"),
            (1.5e-7, "1.5e-07"),
        ] {
            assert_eq!(swift_double(x), text);
        }
    }

    #[test]
    fn dates_round_trip_like_foundation() {
        let d = parse_iso8601("2026-10-11T17:00:00Z").unwrap();
        assert_eq!(seconds_since_epoch(d), 1791738000.0);
        assert_eq!(
            seconds_since_epoch(parse_iso8601("2026-10-11T17:00:00+02:00").unwrap()),
            1791730800.0
        );
        assert_eq!(
            seconds_since_epoch(parse_iso8601("2026-10-11T17:00:00+0200").unwrap()),
            1791730800.0
        );
        assert_eq!(
            seconds_since_epoch(parse_iso8601("2026-10-11T17:00:00.5Z").unwrap()),
            1791738000.5
        );
        for bad in [
            "2026-10-11T17:00:00",
            "2026-10-11 17:00:00Z",
            "2026-10-11T17:00Z",
            "2026-02-30T00:00:00Z",
        ] {
            assert!(parse_iso8601(bad).is_none(), "{bad}");
        }
        assert_eq!(
            format_iso8601(date_from_seconds(1.7)),
            "1970-01-01T00:00:01Z"
        );
        assert_eq!(format_iso8601(d), "2026-10-11T17:00:00Z");
        assert_eq!(
            format_iso8601(date_from_seconds(-0.5)),
            "1969-12-31T23:59:59Z"
        );
    }
}

//! Numbers, lengths and dates as the interface shows them. Dates are seconds since 1970
//! (`f64`), the way take metadata stores them, and are written in the user's locale.

use std::time::{SystemTime, UNIX_EPOCH};

use objc2::rc::Retained;
use objc2_foundation::{NSDate, NSDateFormatter, NSDateFormatterStyle, NSLocale};

use crate::util::ns;

/// Now, in seconds since 1970.
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `{n}` in counted text: the region's thousands separator ("1,234").
pub fn count(n: u64) -> String {
    let sep = NSLocale::currentLocale().groupingSeparator().to_string();
    group(n, &sep)
}

fn group(n: u64, sep: &str) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push_str(sep);
        }
        out.push(c);
    }
    out
}

/// A take's length: `4:05`, or `1:02:03` from an hour up. Rounded to the second.
pub fn length(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A transcript timestamp: `00:01:05`, rounded down.
pub fn timestamp(seconds: f64) -> String {
    let s = seconds.max(0.0).floor() as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// The deck's elapsed-time counter, `00:12:47`.
pub fn elapsed(seconds: f64) -> String {
    timestamp(seconds)
}

fn ns_date(t: f64) -> Retained<NSDate> {
    NSDate::dateWithTimeIntervalSince1970(t)
}

fn styled(t: f64, date: NSDateFormatterStyle, time: NSDateFormatterStyle) -> String {
    let f = NSDateFormatter::new();
    f.setDateStyle(date);
    f.setTimeStyle(time);
    f.stringFromDate(&ns_date(t)).to_string()
}

fn templated(t: f64, template: &str) -> String {
    let f = NSDateFormatter::new();
    f.setLocalizedDateFormatFromTemplate(&ns(template));
    f.stringFromDate(&ns_date(t)).to_string()
}

/// "Monday, 26 October 2026 at 11:05 AM": a take's heading, and what search matches.
pub fn date_text(t: f64) -> String {
    styled(
        t,
        NSDateFormatterStyle::FullStyle,
        NSDateFormatterStyle::ShortStyle,
    )
}

/// "26 Oct, 11:05 AM": the Recorded column.
pub fn day_time(t: f64) -> String {
    templated(t, "MMMdjmm")
}

/// "26 Oct".
pub fn day(t: f64) -> String {
    templated(t, "MMMd")
}

/// "11:05 AM".
pub fn time(t: f64) -> String {
    styled(
        t,
        NSDateFormatterStyle::NoStyle,
        NSDateFormatterStyle::ShortStyle,
    )
}

/// Whether two times fall on the same calendar day here.
pub fn same_day(a: f64, b: f64) -> bool {
    let f = NSDateFormatter::new();
    f.setDateFormat(Some(&ns("yyyy-MM-dd")));
    f.stringFromDate(&ns_date(a)) == f.stringFromDate(&ns_date(b))
}

/// A gain step on the fader scale: `+12`, `0 dB`, `−6` (a true minus sign).
pub fn gain_step(db: i32) -> String {
    match db {
        0 => crate::i18n::tr("0 dB"),
        d if d > 0 => format!("+{d}"),
        d => format!("\u{2212}{}", -d),
    }
}

/// Keeps the first three parts of a firmware version: `02.03.17.00` → `02.03.17`.
pub fn short_firmware(fw: &str) -> String {
    fw.split('.').take(3).collect::<Vec<_>>().join(".")
}

/// Compares version strings the way Foundation's numeric compare does: runs of digits as numbers.
pub fn compare_numeric(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n = String::new();
                while let Some(c) = x.next_if(|c| c.is_ascii_digit()) {
                    n.push(c);
                }
                let mut m = String::new();
                while let Some(d) = y.next_if(|d| d.is_ascii_digit()) {
                    m.push(d);
                }
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let o = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                x.next();
                y.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn lengths() {
        assert_eq!(length(0.0), "0:00");
        assert_eq!(length(59.6), "1:00");
        assert_eq!(length(842.0), "14:02");
        assert_eq!(length(3600.0), "1:00:00");
        assert_eq!(length(3723.4), "1:02:03");
        assert_eq!(length(-3.0), "0:00");
    }

    #[test]
    fn timestamps() {
        assert_eq!(timestamp(0.0), "00:00:00");
        assert_eq!(timestamp(65.9), "00:01:05");
        assert_eq!(timestamp(3725.0), "01:02:05");
        assert_eq!(elapsed(767.0), "00:12:47");
    }

    #[test]
    fn grouping() {
        assert_eq!(group(0, ","), "0");
        assert_eq!(group(999, ","), "999");
        assert_eq!(group(1234, ","), "1,234");
        assert_eq!(group(1_234_567, "."), "1.234.567");
    }

    #[test]
    fn gain_steps() {
        assert_eq!(gain_step(12), "+12");
        assert_eq!(gain_step(0), "0 dB");
        assert_eq!(gain_step(-6), "\u{2212}6");
    }

    #[test]
    fn firmware() {
        assert_eq!(short_firmware("02.03.17.00"), "02.03.17");
        assert_eq!(short_firmware("1.2"), "1.2");
        assert_eq!(
            compare_numeric("02.03.17.00", "02.03.9.00"),
            Ordering::Greater
        );
        assert_eq!(
            compare_numeric("02.03.11.00", "02.03.17.00"),
            Ordering::Less
        );
        assert_eq!(compare_numeric("1.10", "1.10"), Ordering::Equal);
        assert_eq!(compare_numeric("1.010", "1.10"), Ordering::Equal);
    }
}

//! Temporal values and ISO 19108 relations.

use super::TemporalOp;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// Time interval [begin, end]; an instant has begin == end
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    pub begin: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl Interval {
    pub fn instant(t: DateTime<Utc>) -> Self {
        Interval { begin: t, end: t }
    }
    pub fn is_instant(&self) -> bool {
        self.begin == self.end
    }
}

/// Parse xsd:dateTime / xsd:date (with optional timezone; UTC assumed) / gYear / gYearMonth
pub fn parse_datetime(text: &str) -> Option<DateTime<Utc>> {
    let s = text.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // dateTime without timezone, optional fractional seconds
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(Utc.from_utc_datetime(&ndt));
        }
    }
    // dateTime with offset but without seconds or with 'Z' variants rfc3339 rejects
    for fmt in [
        "%Y-%m-%dT%H:%M%:z",
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M:%S%.f%z",
    ] {
        if let Ok(dt) = DateTime::<FixedOffset>::parse_from_str(s, fmt) {
            return Some(dt.with_timezone(&Utc));
        }
    }
    // date with optional timezone
    let (date_part, offset) = split_date_tz(s);
    if let Ok(d) = NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
        let ndt = d.and_hms_opt(0, 0, 0)?;
        let offset = offset.unwrap_or(FixedOffset::east_opt(0)?);
        return offset
            .from_local_datetime(&ndt)
            .single()
            .map(|dt| dt.with_timezone(&Utc));
    }
    // gYearMonth / gYear
    if let Ok(d) = NaiveDate::parse_from_str(&format!("{date_part}-01"), "%Y-%m-%d") {
        return Some(Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?));
    }
    if date_part.len() == 4 {
        if let Ok(year) = date_part.parse::<i32>() {
            return Some(
                Utc.from_utc_datetime(&NaiveDate::from_ymd_opt(year, 1, 1)?.and_hms_opt(0, 0, 0)?),
            );
        }
    }
    None
}

fn split_date_tz(s: &str) -> (&str, Option<FixedOffset>) {
    if let Some(d) = s.strip_suffix('Z') {
        return (d, FixedOffset::east_opt(0));
    }
    // date with +hh:mm / -hh:mm suffix (date itself contains '-')
    if s.len() > 6 {
        let (d, tz) = s.split_at(s.len() - 6);
        let bytes = tz.as_bytes();
        if (bytes[0] == b'+' || bytes[0] == b'-') && bytes[3] == b':' {
            if let (Ok(h), Ok(m)) = (tz[1..3].parse::<i32>(), tz[4..6].parse::<i32>()) {
                let secs = (h * 3600 + m * 60) * if bytes[0] == b'-' { -1 } else { 1 };
                return (d, FixedOffset::east_opt(secs));
            }
        }
    }
    (s, None)
}

/// Evaluate temporal relation `a op b`
pub fn relation(op: TemporalOp, a: &Interval, b: &Interval) -> bool {
    let (a1, a2, b1, b2) = (a.begin, a.end, b.begin, b.end);
    match op {
        TemporalOp::Before => a2 < b1,
        TemporalOp::After => a1 > b2,
        TemporalOp::Begins => a1 == b1 && a2 < b2,
        TemporalOp::BegunBy => a1 == b1 && a2 > b2,
        TemporalOp::TContains => a1 < b1 && a2 > b2,
        TemporalOp::During => a1 > b1 && a2 < b2,
        TemporalOp::TEquals => a1 == b1 && a2 == b2,
        TemporalOp::Ends => a2 == b2 && a1 > b1,
        TemporalOp::EndedBy => a2 == b2 && a1 < b1,
        TemporalOp::Meets => !a.is_instant() && a2 == b1,
        TemporalOp::MetBy => !a.is_instant() && a1 == b2,
        TemporalOp::TOverlaps => a1 < b1 && a2 > b1 && a2 < b2,
        TemporalOp::OverlappedBy => a1 > b1 && a1 < b2 && a2 > b2,
        TemporalOp::AnyInteracts => a1 <= b2 && a2 >= b1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime<Utc> {
        parse_datetime(s).unwrap_or_else(|| panic!("cannot parse {s}"))
    }

    #[test]
    fn parse_forms() {
        assert_eq!(dt("2006-06-28T07:08:00+02:00"), dt("2006-06-28T05:08:00Z"));
        assert_eq!(dt("2006-06-28T05:08:00"), dt("2006-06-28T05:08:00Z"));
        assert_eq!(
            dt("2026-10-05T12:34:56.123456Z").timestamp_subsec_micros(),
            123456
        );
        assert_eq!(dt("2006-10-25Z"), dt("2006-10-25T00:00:00Z"));
        assert_eq!(dt("2006-10-25"), dt("2006-10-25T00:00:00Z"));
        assert_eq!(dt("2006-10-25+02:00"), dt("2006-10-24T22:00:00Z"));
        assert_eq!(dt("2006-10-25-05:00"), dt("2006-10-25T05:00:00Z"));
        assert_eq!(dt("2006"), dt("2006-01-01T00:00:00Z"));
        assert_eq!(dt("2006-10"), dt("2006-10-01T00:00:00Z"));
        assert!(parse_datetime("not a date").is_none());
        assert!(parse_datetime("").is_none());
    }

    #[test]
    fn relations() {
        let i = |s: &str| Interval::instant(dt(s));
        let p = |a: &str, b: &str| Interval {
            begin: dt(a),
            end: dt(b),
        };
        let period = p("2020-01-01T00:00:00Z", "2020-12-31T00:00:00Z");
        assert!(relation(
            TemporalOp::During,
            &i("2020-06-01T00:00:00Z"),
            &period
        ));
        assert!(!relation(
            TemporalOp::During,
            &i("2020-01-01T00:00:00Z"),
            &period
        ));
        assert!(relation(
            TemporalOp::Begins,
            &i("2020-01-01T00:00:00Z"),
            &period
        ));
        assert!(relation(
            TemporalOp::Ends,
            &i("2020-12-31T00:00:00Z"),
            &period
        ));
        assert!(relation(
            TemporalOp::After,
            &i("2021-01-01T00:00:00Z"),
            &period
        ));
        assert!(relation(
            TemporalOp::Before,
            &i("2019-01-01T00:00:00Z"),
            &period
        ));
        assert!(relation(
            TemporalOp::After,
            &i("2021-01-01T00:00:00Z"),
            &i("2020-01-01T00:00:00Z")
        ));
        assert!(relation(
            TemporalOp::TEquals,
            &i("2021-01-01T01:00:00+01:00"),
            &i("2021-01-01T00:00:00Z")
        ));
        let other = p("2020-06-01T00:00:00Z", "2021-06-01T00:00:00Z");
        assert!(relation(TemporalOp::TOverlaps, &period, &other));
        assert!(relation(TemporalOp::OverlappedBy, &other, &period));
        assert!(relation(TemporalOp::AnyInteracts, &other, &period));
        let next = p("2020-12-31T00:00:00Z", "2021-06-01T00:00:00Z");
        assert!(relation(TemporalOp::Meets, &period, &next));
        assert!(relation(TemporalOp::MetBy, &next, &period));
        let inner = p("2020-03-01T00:00:00Z", "2020-04-01T00:00:00Z");
        assert!(relation(TemporalOp::TContains, &period, &inner));
        assert!(relation(TemporalOp::During, &inner, &period));
        let begun = p("2020-01-01T00:00:00Z", "2020-04-01T00:00:00Z");
        assert!(relation(TemporalOp::BegunBy, &period, &begun));
        let ended = p("2020-04-01T00:00:00Z", "2020-12-31T00:00:00Z");
        assert!(relation(TemporalOp::EndedBy, &period, &ended));
    }
}

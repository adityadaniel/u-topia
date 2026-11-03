//! Daily-page dates (PRD §5, §10.3.1). Identity is always ISO `YYYY-MM-DD`; a
//! [`DateFormat`] is only a rendering. Parsing refuses to guess: ambiguous numeric
//! forms such as `11/6/26` never resolve to a date (§5.5).

use chrono::{Datelike, NaiveDate, Weekday};
use serde::{Deserialize, Serialize};

pub const DEFAULT_DATE_FORMAT: &str = "MMMM do, yyyy";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// Month-name forms that are always accepted, independent of the graph setting.
const BUILTIN_FORMATS: &[&str] = &[
    "MMMM do, yyyy",
    "MMMM d, yyyy",
    "MMM do, yyyy",
    "MMM d, yyyy",
    "MMMM do yyyy",
    "MMMM d yyyy",
    "do MMMM yyyy",
    "d MMMM yyyy",
    "d MMM yyyy",
    "EEEE, MMMM do, yyyy",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Year,
    MonthName,
    MonthAbbr,
    Month2,
    Month,
    DayOrdinal,
    Day2,
    Day,
    Weekday,
    WeekdayAbbr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Tok(Tok),
    Lit(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateFormat {
    pattern: String,
    /// Formats this graph used before. Links typed under an earlier setting keep
    /// resolving after the format changes (AC-1.8: "no link breaks").
    #[serde(default)]
    previous: Vec<String>,
}

impl Default for DateFormat {
    fn default() -> Self {
        Self::new(DEFAULT_DATE_FORMAT)
    }
}

impl DateFormat {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            previous: Vec::new(),
        }
    }

    /// Switch to `pattern`, remembering the current pattern as a previous one.
    pub fn changed_to(&self, pattern: &str) -> Self {
        let mut previous = self.previous.clone();
        previous.push(self.pattern.clone());
        previous.retain(|p| p != pattern);
        previous.dedup();
        Self {
            pattern: pattern.to_string(),
            previous,
        }
    }

    pub fn with_previous(pattern: impl Into<String>, previous: Vec<String>) -> Self {
        Self {
            pattern: pattern.into(),
            previous,
        }
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    pub fn previous(&self) -> &[String] {
        &self.previous
    }

    fn pieces(&self) -> Vec<Piece> {
        tokenize(&self.pattern)
    }

    pub fn format(&self, date: NaiveDate) -> String {
        render(&self.pieces(), date)
    }

    /// A format is unambiguous when it carries year, month and day, and the month is a
    /// name or the numeric order is year-first (§5.5).
    pub fn is_unambiguous(&self) -> bool {
        let pieces = self.pieces();
        let toks: Vec<Tok> = pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Tok(t) => Some(*t),
                Piece::Lit(_) => None,
            })
            .collect();
        let year = toks.iter().position(|t| *t == Tok::Year);
        let month = toks.iter().position(|t| {
            matches!(
                t,
                Tok::MonthName | Tok::MonthAbbr | Tok::Month2 | Tok::Month
            )
        });
        let day = toks
            .iter()
            .position(|t| matches!(t, Tok::DayOrdinal | Tok::Day2 | Tok::Day));
        match (year, month, day) {
            (Some(y), Some(m), Some(d)) => {
                matches!(toks[m], Tok::MonthName | Tok::MonthAbbr) || (y < m && m < d)
            }
            _ => false,
        }
    }

    pub fn parse(&self, s: &str) -> Option<NaiveDate> {
        parse_with(&self.pieces(), s)
    }
}

/// `parse_unambiguous_date` from §10.3.1: ISO-8601, the built-in month-name forms, and
/// the graph's configured format when that format is unambiguous.
pub fn parse_unambiguous_date(s: &str, graph_format: &DateFormat) -> Option<NaiveDate> {
    let s = s.trim();
    if let Some(d) = parse_iso(s) {
        return Some(d);
    }
    for f in BUILTIN_FORMATS {
        if let Some(d) = parse_with(&tokenize(f), s) {
            return Some(d);
        }
    }
    if graph_format.is_unambiguous() {
        if let Some(d) = graph_format.parse(s) {
            return Some(d);
        }
    }
    graph_format
        .previous
        .iter()
        .map(|p| DateFormat::new(p.as_str()))
        .filter(DateFormat::is_unambiguous)
        .find_map(|f| f.parse(s))
}

pub fn parse_iso(s: &str) -> Option<NaiveDate> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        if b[r.clone()].iter().all(u8::is_ascii_digit) {
            s[r].parse().ok()
        } else {
            None
        }
    };
    NaiveDate::from_ymd_opt(digits(0..4)? as i32, digits(5..7)?, digits(8..10)?)
}

pub fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

pub fn ordinal_suffix(day: u32) -> &'static str {
    match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

fn tokenize(pattern: &str) -> Vec<Piece> {
    const TABLE: &[(&str, Tok)] = &[
        ("yyyy", Tok::Year),
        ("MMMM", Tok::MonthName),
        ("MMM", Tok::MonthAbbr),
        ("MM", Tok::Month2),
        ("M", Tok::Month),
        ("do", Tok::DayOrdinal),
        ("dd", Tok::Day2),
        ("d", Tok::Day),
        ("EEEE", Tok::Weekday),
        ("EEE", Tok::WeekdayAbbr),
    ];
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut rest = pattern;
    'outer: while !rest.is_empty() {
        for (pat, tok) in TABLE {
            if rest.starts_with(pat) {
                if !lit.is_empty() {
                    out.push(Piece::Lit(std::mem::take(&mut lit)));
                }
                out.push(Piece::Tok(*tok));
                rest = &rest[pat.len()..];
                continue 'outer;
            }
        }
        let ch = rest.chars().next().expect("non-empty");
        lit.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    if !lit.is_empty() {
        out.push(Piece::Lit(lit));
    }
    out
}

fn render(pieces: &[Piece], d: NaiveDate) -> String {
    let mut s = String::new();
    for p in pieces {
        match p {
            Piece::Lit(l) => s.push_str(l),
            Piece::Tok(t) => match t {
                Tok::Year => s.push_str(&format!("{:04}", d.year())),
                Tok::MonthName => s.push_str(MONTHS[d.month0() as usize]),
                Tok::MonthAbbr => s.push_str(&MONTHS[d.month0() as usize][..3]),
                Tok::Month2 => s.push_str(&format!("{:02}", d.month())),
                Tok::Month => s.push_str(&d.month().to_string()),
                Tok::DayOrdinal => s.push_str(&format!("{}{}", d.day(), ordinal_suffix(d.day()))),
                Tok::Day2 => s.push_str(&format!("{:02}", d.day())),
                Tok::Day => s.push_str(&d.day().to_string()),
                Tok::Weekday => s.push_str(weekday_name(d.weekday())),
                Tok::WeekdayAbbr => s.push_str(&weekday_name(d.weekday())[..3]),
            },
        }
    }
    s
}

fn weekday_name(w: Weekday) -> &'static str {
    WEEKDAYS[w.num_days_from_monday() as usize]
}

fn take_digits(s: &str, min: usize, max: usize) -> Option<(u32, &str)> {
    let n = s.bytes().take(max).take_while(u8::is_ascii_digit).count();
    if n < min {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

fn take_name<'a>(s: &'a str, names: &[&str], abbr: bool) -> Option<(usize, &'a str)> {
    // Longest match first so "May" vs "Mar" etc. are unambiguous.
    for (i, name) in names.iter().enumerate() {
        let cand = if abbr { &name[..3] } else { name };
        if s.len() >= cand.len()
            && s.is_char_boundary(cand.len())
            && s[..cand.len()].eq_ignore_ascii_case(cand)
        {
            return Some((i, &s[cand.len()..]));
        }
    }
    None
}

fn parse_with(pieces: &[Piece], input: &str) -> Option<NaiveDate> {
    let (mut y, mut m, mut d) = (None, None, None);
    let mut rest = input.trim();
    let mut has_weekday = false;
    for p in pieces {
        match p {
            Piece::Lit(l) => {
                // Whitespace in literals matches any run of whitespace (including none
                // only if the literal is pure whitespace and the input has none).
                let l_trim = l.trim();
                if l.chars().any(char::is_whitespace) {
                    let before = rest;
                    rest = rest.trim_start();
                    if !l_trim.is_empty() {
                        rest = rest.strip_prefix(l_trim)?;
                        rest = rest.trim_start();
                    } else if before.len() == rest.len() {
                        return None;
                    }
                } else {
                    rest = rest.strip_prefix(l.as_str())?;
                }
            }
            Piece::Tok(t) => match t {
                Tok::Year => {
                    let (v, r) = take_digits(rest, 4, 4)?;
                    y = Some(v as i32);
                    rest = r;
                }
                Tok::Month2 => {
                    let (v, r) = take_digits(rest, 2, 2)?;
                    m = Some(v);
                    rest = r;
                }
                Tok::Month => {
                    let (v, r) = take_digits(rest, 1, 2)?;
                    m = Some(v);
                    rest = r;
                }
                Tok::MonthName => {
                    let (i, r) = take_name(rest, &MONTHS, false)?;
                    m = Some(i as u32 + 1);
                    rest = r;
                }
                Tok::MonthAbbr => {
                    let (i, r) = take_name(rest, &MONTHS, true)?;
                    m = Some(i as u32 + 1);
                    rest = r;
                }
                Tok::Day2 => {
                    let (v, r) = take_digits(rest, 2, 2)?;
                    d = Some(v);
                    rest = r;
                }
                Tok::Day => {
                    let (v, r) = take_digits(rest, 1, 2)?;
                    d = Some(v);
                    rest = r;
                }
                Tok::DayOrdinal => {
                    let (v, r) = take_digits(rest, 1, 2)?;
                    let suffix = r.get(..2)?;
                    if !["st", "nd", "rd", "th"]
                        .iter()
                        .any(|s| suffix.eq_ignore_ascii_case(s))
                    {
                        return None;
                    }
                    d = Some(v);
                    rest = &r[2..];
                }
                Tok::Weekday => {
                    let (_, r) = take_name(rest, &WEEKDAYS, false)?;
                    has_weekday = true;
                    rest = r;
                }
                Tok::WeekdayAbbr => {
                    let (_, r) = take_name(rest, &WEEKDAYS, true)?;
                    has_weekday = true;
                    rest = r;
                }
            },
        }
    }
    if !rest.trim().is_empty() {
        return None;
    }
    let _ = has_weekday;
    NaiveDate::from_ymd_opt(y?, m?, d?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn default_format_renders_roam_style() {
        let f = DateFormat::default();
        assert_eq!(f.format(d(2026, 11, 2)), "November 2nd, 2026");
        assert_eq!(f.format(d(2026, 11, 11)), "November 11th, 2026");
        assert_eq!(f.format(d(2026, 12, 1)), "December 1st, 2026");
        assert_eq!(f.format(d(2026, 12, 23)), "December 23rd, 2026");
        assert_eq!(
            DateFormat::new("yyyy-MM-dd").format(d(2026, 1, 6)),
            "2026-01-06"
        );
    }

    #[test]
    fn ac_1_7_three_forms_resolve_identically() {
        let f = DateFormat::new("dd.MMM.yyyy");
        let want = Some(d(2026, 12, 1));
        assert_eq!(parse_unambiguous_date("2026-12-01", &f), want);
        assert_eq!(parse_unambiguous_date("December 1st, 2026", &f), want);
        assert_eq!(parse_unambiguous_date("01.Dec.2026", &f), want);
        assert_eq!(parse_unambiguous_date("december 1, 2026", &f), want);
    }

    #[test]
    fn ambiguous_numeric_dates_never_resolve() {
        let us = DateFormat::new("MM/dd/yy");
        assert!(!us.is_unambiguous());
        assert_eq!(parse_unambiguous_date("11/6/26", &us), None);
        let us4 = DateFormat::new("MM/dd/yyyy");
        assert!(!us4.is_unambiguous());
        assert_eq!(parse_unambiguous_date("11/06/2026", &us4), None);
        assert!(DateFormat::new("yyyy/MM/dd").is_unambiguous());
        assert_eq!(
            parse_unambiguous_date("2026/11/06", &DateFormat::new("yyyy/MM/dd")),
            Some(d(2026, 11, 6))
        );
    }

    #[test]
    fn previous_formats_keep_parsing() {
        let f = DateFormat::new("yyyy/MM/dd").changed_to("yyyy-MM-dd");
        assert_eq!(f.pattern(), "yyyy-MM-dd");
        assert_eq!(
            parse_unambiguous_date("2026/12/01", &f),
            Some(d(2026, 12, 1))
        );
        let back = f.changed_to("yyyy/MM/dd");
        assert_eq!(back.previous(), &["yyyy-MM-dd".to_string()]);
    }

    #[test]
    fn yearless_and_invalid_are_not_dates() {
        let f = DateFormat::default();
        assert_eq!(
            parse_unambiguous_date("Nov 2", &DateFormat::new("MMM d")),
            None
        );
        assert_eq!(parse_unambiguous_date("February 30th, 2026", &f), None);
        assert_eq!(parse_unambiguous_date("2026-13-01", &f), None);
        assert_eq!(parse_unambiguous_date("Modern Religion", &f), None);
        assert_eq!(
            parse_unambiguous_date("2026", &DateFormat::new("yyyy")),
            None
        );
    }

    #[test]
    fn round_trip_formats() {
        for pat in [
            "MMMM do, yyyy",
            "yyyy-MM-dd",
            "EEEE, MMMM do, yyyy",
            "d MMM yyyy",
        ] {
            let f = DateFormat::new(pat);
            let date = d(2027, 1, 6);
            assert_eq!(f.parse(&f.format(date)), Some(date), "{pat}");
        }
    }
}

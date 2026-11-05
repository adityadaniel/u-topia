//! `utopia://` links: other apps (Shortcuts, Alfred, a browser bookmark) open a page,
//! a journal day, a block, a search, or capture a line.
//!
//! - `utopia://page/<title>`
//! - `utopia://journal/today` · `utopia://journal/2026-11-02`
//! - `utopia://block/<uid>`
//! - `utopia://search?q=<text>`
//! - `utopia://capture?text=<text>` — appends to today's page

use chrono::NaiveDate;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlAction {
    Page(String),
    /// `None` is today.
    Journal(Option<NaiveDate>),
    Block(String),
    Search(String),
    Capture(String),
}

/// Percent-decoding; `+` is a space only in query strings.
fn decode(s: &str, plus_is_space: bool) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' if plus_is_space => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then(|| decode(v, true)).flatten()
    })
}

pub fn parse_url(url: &str) -> Option<UrlAction> {
    let rest = url.strip_prefix("utopia://")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (kind, arg) = path.split_once('/').unwrap_or((path, ""));
    let arg = decode(arg.trim_end_matches('/'), false)?;
    let nonempty = |s: String| (!s.trim().is_empty()).then_some(s);
    match kind {
        "page" => nonempty(arg).map(UrlAction::Page),
        "journal" if arg.is_empty() || arg == "today" => Some(UrlAction::Journal(None)),
        "journal" => utopia_core::date::parse_iso(&arg).map(|d| UrlAction::Journal(Some(d))),
        "block" => nonempty(arg).map(UrlAction::Block),
        "search" => param(query, "q").and_then(nonempty).map(UrlAction::Search),
        "capture" => param(query, "text")
            .and_then(nonempty)
            .map(UrlAction::Capture),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_links() {
        assert_eq!(
            parse_url("utopia://page/Project%20Atlas"),
            Some(UrlAction::Page("Project Atlas".into()))
        );
        assert_eq!(
            parse_url("utopia://page/C++"),
            Some(UrlAction::Page("C++".into())),
            "+ is literal in paths"
        );
        assert_eq!(
            parse_url("utopia://journal/today"),
            Some(UrlAction::Journal(None))
        );
        assert_eq!(
            parse_url("utopia://journal/2026-11-02"),
            Some(UrlAction::Journal(NaiveDate::from_ymd_opt(2026, 11, 2)))
        );
        assert_eq!(
            parse_url("utopia://search?q=caf%C3%A9+au+lait"),
            Some(UrlAction::Search("café au lait".into()))
        );
        assert_eq!(
            parse_url("utopia://capture?text=buy+milk&x=1"),
            Some(UrlAction::Capture("buy milk".into()))
        );
        assert_eq!(
            parse_url("utopia://block/01J9ZX"),
            Some(UrlAction::Block("01J9ZX".into()))
        );
        for bad in [
            "https://page/x",
            "utopia://page/",
            "utopia://journal/nope",
            "utopia://search",
            "utopia://page/%zz",
            "utopia://delete/x",
        ] {
            assert_eq!(parse_url(bad), None, "{bad}");
        }
    }
}

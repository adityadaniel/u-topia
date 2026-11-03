//! Text helpers shared by the editor and the store (§10.5): grapheme-aware cursor
//! movement, minimal edit diffs, NFC normalization, and mark toggling (Cmd+B/I/E).

use std::ops::Range;

use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

use crate::command::TextEdit;

pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

pub fn prev_grapheme(s: &str, offset: usize) -> usize {
    s[..offset]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}

pub fn next_grapheme(s: &str, offset: usize) -> usize {
    s[offset..]
        .graphemes(true)
        .next()
        .map_or(s.len(), |g| offset + g.len())
}

/// Start of the word at or before `offset` (Alt+Left / double-click).
pub fn prev_word_start(s: &str, offset: usize) -> usize {
    let mut last = 0;
    for (i, w) in s.split_word_bound_indices() {
        if i >= offset {
            break;
        }
        if w.chars().any(char::is_alphanumeric) {
            last = i;
        }
    }
    last
}

/// End of the word at or after `offset` (Alt+Right / double-click).
pub fn next_word_end(s: &str, offset: usize) -> usize {
    for (i, w) in s.split_word_bound_indices() {
        let end = i + w.len();
        if end > offset && w.chars().any(char::is_alphanumeric) {
            return end;
        }
    }
    s.len()
}

/// The word range containing `offset`, for double-click selection.
pub fn word_range_at(s: &str, offset: usize) -> Range<usize> {
    for (i, w) in s.split_word_bound_indices() {
        if offset >= i && offset < i + w.len() {
            return i..i + w.len();
        }
    }
    s.len()..s.len()
}

/// Clamp `offset` down to the nearest char boundary.
pub fn floor_char_boundary(s: &str, offset: usize) -> usize {
    let mut o = offset.min(s.len());
    while !s.is_char_boundary(o) {
        o -= 1;
    }
    o
}

/// Minimal single-range edit turning `old` into `new` (common prefix/suffix trim),
/// always on char boundaries.
pub fn diff(old: &str, new: &str) -> Option<TextEdit> {
    if old == new {
        return None;
    }
    let prefix = old
        .char_indices()
        .zip(new.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(new.len()), |((i, _), _)| i);
    let prefix = floor_char_boundary(new, floor_char_boundary(old, prefix));
    let old_rest = &old[prefix..];
    let new_rest = &new[prefix..];
    let suffix: usize = old_rest
        .chars()
        .rev()
        .zip(new_rest.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let suffix = suffix.min(old_rest.len()).min(new_rest.len());
    Some(TextEdit {
        range: prefix..old.len() - suffix,
        insert: new[prefix..new.len() - suffix].to_string(),
    })
}

/// Apply `edit` to `s`, validating bounds and char boundaries.
pub fn apply_edit(s: &str, edit: &TextEdit) -> Option<String> {
    let Range { start, end } = edit.range.clone();
    if start > end || end > s.len() || !s.is_char_boundary(start) || !s.is_char_boundary(end) {
        return None;
    }
    let mut out = String::with_capacity(s.len() + edit.insert.len());
    out.push_str(&s[..start]);
    out.push_str(&edit.insert);
    out.push_str(&s[end..]);
    Some(out)
}

/// Wrap/unwrap the selection with `delim` (FR-2.16, §9.1 Cmd+B/I/E). With an empty
/// selection, an unclosed opener before the caret is closed (AC-2.9); otherwise an empty
/// pair is inserted with the caret inside. Returns the new text and selection.
pub fn toggle_mark(text: &str, sel: Range<usize>, delim: &str) -> (String, Range<usize>) {
    let dl = delim.len();
    if sel.start == sel.end {
        let caret = sel.start;
        let before = &text[..caret];
        let opens = before.matches(delim).count();
        if opens % 2 == 1 {
            let mut out = text.to_string();
            out.insert_str(caret, delim);
            return (out, caret + dl..caret + dl);
        }
        let mut out = text.to_string();
        out.insert_str(caret, &format!("{delim}{delim}"));
        return (out, caret + dl..caret + dl);
    }
    let selected = &text[sel.clone()];
    // Selection includes the delimiters.
    if selected.len() >= 2 * dl && selected.starts_with(delim) && selected.ends_with(delim) {
        let inner = &selected[dl..selected.len() - dl];
        let mut out = String::new();
        out.push_str(&text[..sel.start]);
        out.push_str(inner);
        out.push_str(&text[sel.end..]);
        return (out, sel.start..sel.start + inner.len());
    }
    // Delimiters sit just outside the selection.
    if sel.start >= dl && text[..sel.start].ends_with(delim) && text[sel.end..].starts_with(delim) {
        let mut out = String::new();
        out.push_str(&text[..sel.start - dl]);
        out.push_str(selected);
        out.push_str(&text[sel.end + dl..]);
        return (out, sel.start - dl..sel.end - dl);
    }
    let mut out = String::new();
    out.push_str(&text[..sel.start]);
    out.push_str(delim);
    out.push_str(selected);
    out.push_str(delim);
    out.push_str(&text[sel.end..]);
    (out, sel.start + dl..sel.end + dl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes_respect_clusters() {
        let s = "a👩‍👩‍👧b";
        let after_a = next_grapheme(s, 0);
        assert_eq!(after_a, 1);
        let after_family = next_grapheme(s, after_a);
        assert_eq!(&s[after_family..], "b");
        assert_eq!(prev_grapheme(s, after_family), 1);
        let combining = "e\u{301}x";
        assert_eq!(next_grapheme(combining, 0), 3);
    }

    #[test]
    fn diff_round_trips() {
        for (a, b) in [
            ("hello", "hello world"),
            ("hello world", "hello"),
            ("abc", "axc"),
            ("", "日本"),
            ("日本語", "日語"),
            ("aaa", "aa"),
            ("🎉x", "🎊x"),
        ] {
            let e = diff(a, b).unwrap();
            assert_eq!(apply_edit(a, &e).unwrap(), b, "{a} -> {b}");
        }
        assert!(diff("same", "same").is_none());
    }

    #[test]
    fn apply_edit_rejects_bad_ranges() {
        let e = TextEdit {
            range: 1..2,
            insert: String::new(),
        };
        assert!(apply_edit("日", &e).is_none());
        let e = TextEdit {
            range: 0..9,
            insert: String::new(),
        };
        assert!(apply_edit("abc", &e).is_none());
    }

    #[test]
    fn ac_2_9_cmd_b_closes_unclosed() {
        let (t, sel) = toggle_mark("**bold", 6..6, "**");
        assert_eq!(t, "**bold**");
        assert_eq!(sel, 8..8);
    }

    #[test]
    fn wrap_and_unwrap() {
        let (t, sel) = toggle_mark("a word here", 2..6, "**");
        assert_eq!(t, "a **word** here");
        assert_eq!(&t[sel.clone()], "word");
        let (t2, sel2) = toggle_mark(&t, sel, "**");
        assert_eq!(t2, "a word here");
        assert_eq!(&t2[sel2], "word");
        let (t3, _) = toggle_mark("x", 1..1, "`");
        assert_eq!(t3, "x``");
    }

    #[test]
    fn words() {
        let s = "hello big world";
        assert_eq!(prev_word_start(s, 9), 6);
        assert_eq!(next_word_end(s, 6), 9);
        assert_eq!(word_range_at(s, 7), 6..9);
    }
}

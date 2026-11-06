//! Month calendar used as the journal date picker (FR-1.3, FR-1.4).

use std::collections::HashSet;

use chrono::{Datelike, Months, NaiveDate};
use gpui::{div, prelude::*, px, Context, Div, MouseButton, MouseDownEvent, Window};

use crate::theme::theme;

pub fn first_of_month(d: NaiveDate) -> NaiveDate {
    d.with_day(1).expect("day 1 exists")
}

pub fn shift_month(d: NaiveDate, delta: i32) -> NaiveDate {
    let m = first_of_month(d);
    if delta >= 0 {
        m + Months::new(delta as u32)
    } else {
        m - Months::new((-delta) as u32)
    }
}

/// The 6×7 grid of dates shown for `month` (Monday first).
pub fn grid(month: NaiveDate) -> Vec<NaiveDate> {
    let first = first_of_month(month);
    let lead = first.weekday().num_days_from_monday() as u64;
    let start = first - chrono::Days::new(lead);
    (0..42).map(|i| start + chrono::Days::new(i)).collect()
}

pub fn render<V: 'static>(
    cx: &mut Context<V>,
    month: NaiveDate,
    today: NaiveDate,
    existing: &HashSet<NaiveDate>,
    pick: fn(&mut V, NaiveDate, bool, &mut Window, &mut Context<V>),
    shift: fn(&mut V, i32, &mut Context<V>),
) -> Div {
    let th = theme(cx).clone();
    let header = div()
        .flex()
        .justify_between()
        .items_center()
        .text_size(px(crate::theme::BODY_SIZE))
        .child(
            div()
                .id("cal-prev")
                .px_1()
                .cursor_pointer()
                .child("‹")
                .on_click(cx.listener(move |v, _, _, cx| shift(v, -1, cx))),
        )
        .child(
            div()
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(month.format("%B %Y").to_string()),
        )
        .child(
            div()
                .id("cal-next")
                .px_1()
                .cursor_pointer()
                .child("›")
                .on_click(cx.listener(move |v, _, _, cx| shift(v, 1, cx))),
        );
    let names = div()
        .flex()
        .children(["M", "T", "W", "T", "F", "S", "S"].iter().map(|n| {
            div()
                .w(px(26.))
                .text_size(px(crate::theme::CAPTION_SIZE))
                .text_color(th.faint)
                .flex()
                .justify_center()
                .child(*n)
        }));
    let cells = grid(month).into_iter().map(|d| {
        let in_month = d.month() == month.month();
        let is_today = d == today;
        let has = existing.contains(&d);
        div()
            .w(px(26.))
            .h(px(22.))
            .flex()
            .justify_center()
            .items_center()
            .text_size(px(crate::theme::CAPTION_SIZE))
            .rounded_sm()
            .cursor_pointer()
            .text_color(if in_month { th.text } else { th.faint })
            .when(has, |c| c.font_weight(gpui::FontWeight::BOLD))
            .when(is_today, |c| c.bg(th.chip_bg).text_color(th.accent))
            .hover(|s| s.bg(th.hover))
            .child(d.day().to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                    pick(v, d, e.modifiers.shift, w, cx)
                }),
            )
    });
    let mut rows = Vec::new();
    let cells: Vec<_> = cells.collect();
    let mut it = cells.into_iter();
    for _ in 0..6 {
        rows.push(div().flex().children(it.by_ref().take(7)));
    }
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(header)
        .child(names)
        .children(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_starts_on_monday_and_covers_month() {
        let nov = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let g = grid(nov);
        assert_eq!(g.len(), 42);
        assert_eq!(g[0].weekday(), chrono::Weekday::Mon);
        assert!(g.contains(&NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()));
        assert!(g.contains(&NaiveDate::from_ymd_opt(2026, 11, 30).unwrap()));
        assert_eq!(
            shift_month(nov, -11),
            NaiveDate::from_ymd_opt(2025, 12, 1).unwrap()
        );
        assert_eq!(
            shift_month(nov, 2),
            NaiveDate::from_ymd_opt(2027, 1, 1).unwrap()
        );
    }
}

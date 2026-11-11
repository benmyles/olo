//! A screen's use as cards (the Stats tab): when it was started and last
//! used, how long it has run and been in use; its actions by kind and how
//! many failed; how busy it was minute by minute over the last hour of
//! use; and the programs run on it. From the history the window already
//! follows, so it changes as the agent works.

use std::collections::VecDeque;

use std::rc::Rc;

use olo_kit::{
    Bounds, Child, Element, FontWeight, IntoElement, ParentElement as _, ScrollHandle, SharedString, Styled as _, WindowCx, agent, canvas, div, metrics as m, point, px,
    style::{self, c},
    theme,
    widgets::{
        self, EmptyState, Graph, ScrollView, Series, StringView,
        dashboard::{self, Card, FACT_ROW, Facts, GAP},
    },
};

use crate::{
    history::{self, Action},
    replay::{KINDS, Replay},
};

const MIN_CARD: f32 = 250.0;
const GRAPH_H: f32 = 96.0;

/// "14 min", "2 h 3 min", "45 s".
pub fn span(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h {} min", s / 3600, s / 60 % 60),
    }
}

/// When `ms` was: the time today, else the day and time.
pub fn when(ms: u64, now: u64) -> String {
    let t = history::clock(ms);
    let (date, time) = (t.get(..10).unwrap_or(""), t.get(11..16).unwrap_or(""));
    if history::midnight(ms) == history::midnight(now) {
        time.to_owned()
    } else {
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let month = date.get(5..7).and_then(|m| m.parse::<usize>().ok()).and_then(|m| MONTHS.get(m.wrapping_sub(1))).unwrap_or(&"");
        let day = date.get(8..10).and_then(|d| d.parse::<u32>().ok()).unwrap_or(0);
        format!("{month} {day}, {time}")
    }
}

/// "3 min ago", "just now".
pub fn ago(ms: u64, now: u64) -> String {
    match now.saturating_sub(ms) / 1000 {
        0..10 => "just now".into(),
        s @ 10..60 => format!("{s} s ago"),
        s => format!("{} ago", span(s * 1000)),
    }
}

/// The Stats tab for `replay`'s screen: its cards, two abreast when
/// `width` has room, scrolled with `scroll`.
pub fn view(replay: &Child<Replay>, running: bool, width: f32, scroll: &ScrollHandle, cx: &mut WindowCx) -> Element {
    let (name, st, (series, busiest), end, empty) = replay.update(cx, |r, _| {
        let st = r.stats(running).clone();
        let now = history::now_ms();
        let end = if running { now } else { st.last_used.unwrap_or(now) };
        (r.name.clone(), st, per_minute(r.actions(), end), end, r.actions().is_empty())
    });
    if empty {
        return div().size_full().child(EmptyState::new("Nothing has been done on this screen yet.").icon("agent-screens"));
    }
    let p = theme::palette();
    let now = history::now_ms();
    let inner = width - 2.0 * GAP;
    let columns = if inner >= 2.0 * MIN_CARD + GAP { 2 } else { 1 };
    let w = ((inner - GAP * (columns - 1) as f32) / columns as f32).floor();
    let total: usize = st.actions.values().sum();

    // Use.
    let mut facts: Vec<(SharedString, SharedString)> = Vec::new();
    if let Some(t) = st.started {
        facts.push(("Started".into(), format!("{} ({})", when(t, now), ago(t, now)).into()));
    }
    if let Some(t) = st.last_used {
        facts.push(("Last used".into(), format!("{} ({})", when(t, now), ago(t, now)).into()));
    }
    facts.push(("Ran for".into(), span(st.ran_ms).into()));
    facts.push(("In use".into(), span(st.active_ms).into()));
    let caption = if running { "Running now" } else { "Stopped" };
    let use_card = Card::new(format!("stats-use-{name}"), "Use").figure(span(st.active_ms)).caption(caption).child(Facts::new(facts));

    // Actions by kind.
    let kinds: Vec<(&str, usize)> = KINDS.iter().map(|(_, label, ks)| (*label, ks.iter().map(|k| st.actions.get(*k).copied().unwrap_or(0)).sum())).filter(|(_, n)| *n > 0).collect();
    let failed = match st.failed {
        0 => "None failed".to_owned(),
        1 => "1 failed".to_owned(),
        f => format!("{f} failed"),
    };
    let actions_card = Card::new(format!("stats-actions-{name}"), "Actions").figure(style::thousands(total)).caption(failed).child(kind_bars(kinds));

    // How busy, minute by minute.
    let fmt = |v: f64| format!("{v:.0}");
    let lines = vec![Series::new("Actions", c(p.keyboard_navigation), Rc::new(series)).fill(true)];
    let graph = Graph::new(format!("screen-activity-{name}"), lines, end as f64 / 1000.0).span(3600.0).grid(600.0).unit(fmt).smooth(false).plain();
    let activity_card = Card::new(format!("stats-activity-{name}"), "Actions a Minute").figure(format!("{busiest} at most")).caption("The last hour of use").child(div().h(px(GRAPH_H)).child(graph));

    // Programs.
    let programs: Vec<(SharedString, SharedString)> =
        st.programs.iter().map(|(p, n)| (SharedString::from(p.clone()), SharedString::from(if *n == 1 { "once".to_owned() } else { format!("{n} times") }))).collect();
    let mut programs_card = Card::new(format!("stats-programs-{name}"), "Programs").figure(st.programs.len().to_string()).caption("Run on it with “olo-screen run”");
    programs_card = if programs.is_empty() {
        let ink = style::card_ink(&p);
        programs_card.child(StringView::new("None run.").height(FACT_ROW).color(ink.dim))
    } else {
        programs_card.child(Facts::new(programs))
    };

    let cards = vec![use_card, actions_card, activity_card, programs_card];
    let mut grid = div().flex().flex_col().gap(px(GAP)).p(px(GAP));
    let mut cards = cards.into_iter().peekable();
    while cards.peek().is_some() {
        let mut row = div().flex().flex_row().gap(px(GAP));
        for card in cards.by_ref().take(columns) {
            row = row.child(div().w(px(w)).flex().flex_col().child(card));
        }
        grid = grid.child(row);
    }
    ScrollView::new(scroll).border(false).auto_hide(true).background(dashboard::gutter(&p)).child(grid).into_any_element()
}

/// Each kind of action: its name, a bar as long as its share of the
/// most, and how many.
fn kind_bars(kinds: Vec<(&'static str, usize)>) -> impl IntoElement {
    let h = kinds.len().max(1) as f32 * FACT_ROW;
    canvas(move |g, r| {
        let p = theme::palette();
        let ink = style::card_ink(&p);
        let font = style::ui_font(FontWeight::NORMAL);
        let most = kinds.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
        let label_w = kinds.iter().map(|(l, _)| style::text_width_app(l, style::PLAIN_SIZE, &font)).fold(0.0, f32::max).ceil() + m::SPACING;
        let count_w = kinds.iter().map(|(_, n)| style::text_width_app(&n.to_string(), style::PLAIN_SIZE, &font)).fold(0.0, f32::max).ceil() + m::SPACING;
        for (i, (label, n)) in kinds.iter().enumerate() {
            let mid = r.top() + px(i as f32 * FACT_ROW + FACT_ROW / 2.0);
            let l = style::label(g, r.left(), mid, style::Align::Left, *label, style::PLAIN_SIZE, font.clone(), ink.dim);
            let v = style::label(g, r.right(), mid, style::Align::Right, n.to_string(), style::PLAIN_SIZE, font.clone(), ink.text);
            agent::record(g, agent::Node { value: Some(n.to_string().into()), ..agent::Node::label(*label, l.union(&v)) });
            let bar = Bounds::from_corners(point(r.left() + px(label_w), mid - px(4.0)), point(r.right() - px(count_w), mid + px(4.0)));
            if bar.size.width > px(8.0) {
                widgets::level_bar(g, style::snap(g, bar), *n as f32 / most as f32, Some(c(p.keyboard_navigation)), &p);
            }
        }
    })
    .w_full()
    .h(px(h))
    .flex_shrink_0()
}

/// Actions begun each minute over the hour up to `end` (ms), as a graph's
/// samples (seconds, count), and the most in a minute.
fn per_minute(actions: &[Action], end: u64) -> (VecDeque<(f64, f64)>, usize) {
    let end_min = end / 60_000;
    let first = end_min.saturating_sub(59);
    let mut counts = [0usize; 60];
    for a in actions.iter().rev().take_while(|a| a.at / 60_000 >= first) {
        let m = a.at / 60_000;
        if m <= end_min {
            counts[(m - first) as usize] += 1;
        }
    }
    let samples = counts.iter().enumerate().map(|(i, n)| (((first + i as u64) * 60) as f64, *n as f64)).collect();
    (samples, counts.iter().copied().max().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_a_minute() {
        let a = |at: u64| Action { at, ms: 1, kind: "click".into(), args: vec![], result: None, error: None };
        let end = 100 * 60_000 + 30_000;
        let actions = vec![a(10 * 60_000), a(41 * 60_000), a(99 * 60_000 + 1), a(99 * 60_000 + 2), a(100 * 60_000)];
        let (samples, most) = per_minute(&actions, end);
        assert_eq!(samples.len(), 60);
        assert_eq!(most, 2);
        assert_eq!(samples[0], ((41 * 60) as f64, 1.0));
        assert_eq!(samples[58].1, 2.0);
        assert_eq!(samples[59].1, 1.0);
        assert_eq!(span(45_000), "45 s");
        assert_eq!(span(3_780_000), "1 h 3 min");
    }
}

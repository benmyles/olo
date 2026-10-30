//! A screen's use as cards (the Stats tab): when it was started and last
//! used, how long it has run and been in use; its actions by kind and how
//! many failed; how busy it was minute by minute over the last hour of
//! use; and the programs run on it. From the history the window already
//! follows, so it changes as the agent works.

use std::collections::VecDeque;

use olo_kit::{
    dashboard::{self, Card, GAP},
    egui::{Align, Rect, Sense, Ui, pos2, vec2},
    graph::{Graph, Series},
    metrics as m,
    style::{self, c},
    widgets::{self, ScrollView},
};

use crate::{
    history::{self, Action},
    replay::KINDS,
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

pub fn ui(ui: &mut Ui, rect: Rect, replay: &mut crate::replay::Replay, running: bool) {
    let name = replay.name.clone();
    let st = replay.stats(running).clone();
    let actions = replay.actions();
    let p = style::palette(ui.ctx());
    if actions.is_empty() {
        ui.put(rect, widgets::EmptyState::new("Nothing has been done on this screen yet.").icon("agent-screens"));
        return;
    }
    let now = history::now_ms();
    dashboard::gutter(ui, rect);
    ScrollView::new(("screen-stats", &name)).border(false).auto_hide(true).background(style::card_paper(&p).0).show_in(ui, rect, |ui, _| {
        let area = Rect::from_min_max(ui.max_rect().min + vec2(GAP, GAP), pos2(ui.max_rect().right() - GAP, ui.max_rect().top() + 100_000.0));
        let columns = if area.width() >= 2.0 * MIN_CARD + GAP { 2 } else { 1 };
        let w = ((area.width() - GAP * (columns - 1) as f32) / columns as f32).floor();
        let total: usize = st.actions.values().sum();

        // Use.
        let mut facts: Vec<(&str, String)> = Vec::new();
        if let Some(t) = st.started {
            facts.push(("Started", format!("{} ({})", when(t, now), ago(t, now))));
        }
        if let Some(t) = st.last_used {
            facts.push(("Last used", format!("{} ({})", when(t, now), ago(t, now))));
        }
        facts.push(("Ran for", span(st.ran_ms)));
        facts.push(("In use", span(st.active_ms)));
        let use_h = Card::height(dashboard::facts_height(facts.len()), true);
        // Actions by kind.
        let kinds: Vec<(&str, usize)> = KINDS.iter().map(|(_, label, ks)| (*label, ks.iter().map(|k| st.actions.get(*k).copied().unwrap_or(0)).sum())).filter(|(_, n)| *n > 0).collect();
        let kinds_h = Card::height(kinds.len().max(1) as f32 * dashboard::FACT_ROW, true);
        // Programs.
        let programs: Vec<(String, String)> = st.programs.iter().map(|(p, n)| (p.clone(), if *n == 1 { "once".to_owned() } else { format!("{n} times") })).collect();
        let programs_h = Card::height(dashboard::facts_height(programs.len().max(1)), true);
        let activity_h = Card::height(GRAPH_H, true);

        let mut y = area.top();
        let mut place = |heights: &[f32]| -> Vec<Rect> {
            let mut out = Vec::new();
            for row in heights.chunks(columns) {
                let h = row.iter().copied().fold(0.0, f32::max);
                for (i, _) in row.iter().enumerate() {
                    out.push(Rect::from_min_size(pos2(area.left() + i as f32 * (w + GAP), y), vec2(w, h)));
                }
                y += h + GAP;
            }
            out
        };
        let rects = place(&[use_h, kinds_h, activity_h, programs_h]);

        let in_use = span(st.active_ms);
        let caption = if running { "Running now" } else { "Stopped" };
        let card = Card::new("Use").figure(&in_use).caption(caption).show(ui, rects[0]);
        let rows: Vec<(&str, &str)> = facts.iter().map(|(l, v)| (*l, v.as_str())).collect();
        dashboard::facts(ui, card.content, &rows);

        let n = style::thousands(total);
        let failed = match st.failed {
            0 => "None failed".to_owned(),
            1 => "1 failed".to_owned(),
            f => format!("{f} failed"),
        };
        let card = Card::new("Actions").figure(&n).caption(&failed).show(ui, rects[1]);
        kind_bars(ui, card.content, &kinds);

        let end = if running { now } else { st.last_used.unwrap_or(now) };
        let (series, busiest) = per_minute(actions, end);
        let busiest = format!("{busiest} at most");
        let card = Card::new("Actions a Minute").figure(&busiest).caption("The last hour of use").show(ui, rects[2]);
        let fmt = |v: f64| format!("{v:.0}");
        let lines = [Series::new("Actions", c(p.keyboard_navigation), &series).fill(true)];
        Graph::new(("screen-activity", &name), &lines, end as f64 / 1000.0).span(3600.0).grid(600.0).unit(&fmt).smooth(false).plain().show(ui, card.content);

        let count = st.programs.len().to_string();
        let card = Card::new("Programs").figure(&count).caption("Run on it with “olo-screen run”").show(ui, rects[3]);
        if programs.is_empty() {
            style::label(ui.painter(), card.content.left(), card.content.top() + dashboard::FACT_ROW / 2.0, Align::Min, "None run.", style::plain_font(), style::card_ink(&p).dim);
        } else {
            let rows: Vec<(&str, &str)> = programs.iter().map(|(l, v)| (l.as_str(), v.as_str())).collect();
            dashboard::facts(ui, card.content, &rows);
        }
        ui.allocate_rect(Rect::from_min_size(ui.max_rect().min, vec2(area.width(), y - ui.max_rect().top())), Sense::hover());
    });
}

/// Each kind of action: its name, a bar as long as its share of the
/// most, and how many.
fn kind_bars(ui: &Ui, r: Rect, kinds: &[(&str, usize)]) {
    let p = style::palette(ui.ctx());
    let ink = style::card_ink(&p);
    let most = kinds.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
    let label_w = kinds.iter().map(|(l, _)| style::label_width(ui.ctx(), l, style::plain_font())).fold(0.0, f32::max).ceil() + m::SPACING;
    let count_w = kinds.iter().map(|(_, n)| style::label_width(ui.ctx(), &n.to_string(), style::plain_font())).fold(0.0, f32::max).ceil() + m::SPACING;
    for (i, (label, n)) in kinds.iter().enumerate() {
        let y = r.top() + i as f32 * dashboard::FACT_ROW;
        let mid = y + dashboard::FACT_ROW / 2.0;
        style::label(ui.painter(), r.left(), mid, Align::Min, *label, style::plain_font(), ink.dim);
        style::label(ui.painter(), r.right(), mid, Align::Max, &n.to_string(), style::plain_font(), ink.text);
        let bar = Rect::from_min_max(pos2(r.left() + label_w, mid - 4.0), pos2(r.right() - count_w, mid + 4.0));
        if bar.width() > 8.0 {
            widgets::level_bar(ui.painter(), style::snap(ui.ctx(), bar), *n as f32 / most as f32, Some(c(p.keyboard_navigation)));
        }
    }
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

//! Kache's Deskbar widget (`olo_protocol::glance`; asked for with the app,
//! Ben, 2026-11-15: "a Deskbar widget of its own"). `olo-kache --glance`,
//! started by Deskbar, no window:
//!
//! - **Build Cache**: the day's hit rate (of what could be cached), the
//!   pool's slots in use; a note when kache is stopped or its store is
//!   over its limit; and the last 6 or 12 hours across, each its builds and
//!   the share of them that were hits (a bar). Its tip says the store too.
//!
//! Made from what Kache reads (`data::read`), again when kache's files
//! change, but at most every ten seconds (a burst of builds writes the
//! log many times a second), and as the hour turns (the hours move on).

use std::time::{Duration, Instant};

use olo_protocol::glance::{Body, Entry, Glance, Request, Slot, Timeline};

use crate::data;

/// The least time between two looks.
const EVERY: Duration = Duration::from_secs(10);

/// Serve Deskbar's Kache widgets until it asks for none.
pub fn serve() -> std::process::ExitCode {
    let dir = data::cache_dir();
    let watched = if data::demo() { Vec::new() } else { vec![dir.join("events.jsonl"), dir.join("gc_stats.json"), data::socket_path(), data::config_path()] };
    let mut last: Option<Instant> = None;
    olo_kit::glance::serve("kache", watched, move |asked| {
        // (Too soon after the last: nothing written, looked at once it's time.)
        if let Some(at) = last
            && at.elapsed() < EVERY
        {
            return (Vec::new(), Some(EVERY - at.elapsed()));
        }
        last = Some(Instant::now());
        let snap = data::read();
        let now = (data::now_ms() / 1000) as i64;
        let made = asked.iter().map(|r| (r.widget, timeline(r, &snap, now))).collect();
        // (Again as the hour turns: the hours move on.)
        (made, Some(Duration::from_secs((3600 - now.rem_euclid(3600)) as u64 + 1)))
    })
}

/// Widget `r`'s timeline: one entry, from this minute.
pub fn timeline(r: &Request, snap: &data::Snapshot, now: i64) -> Timeline {
    let kind = olo_protocol::glance::kind("kache", &r.kind);
    let span: usize = kind.map_or("6", |k| k.value(&r.options, "span")).parse().unwrap_or(6);
    Timeline { entries: vec![Entry { from: now - now.rem_euclid(60), glance: glance(snap, span) }], made: now }
}

/// What it shows.
pub fn glance(snap: &data::Snapshot, span: usize) -> Glance {
    let running = snap.unit.as_ref().map(|u| u.running());
    let day = &snap.day;
    let value = match (running, day.rate()) {
        (Some(false), _) => "off".to_owned(),
        (_, Some(r)) => format!("{}%", (r * 100.0).round()),
        (_, None) => "—".to_owned(),
    };
    let mut detail = Vec::new();
    if let Some(p) = &snap.pool {
        detail.push(format!("{} of {} slots", p.compiles + p.tests, p.size));
    }
    let over = snap.daemon.as_ref().is_some_and(|d| d.limit > 0 && d.size > d.limit);
    let note = match running {
        Some(false) => Some("kache is stopped: builds go on uncached".to_owned()),
        _ if over => snap.daemon.as_ref().map(|d| format!("The store is over its {} limit", data::bytes(d.limit))),
        _ => None,
    };
    // The last hours, oldest first: each its builds and its share of hits.
    let n = span.clamp(1, day.hours.len().max(1));
    let first = day.hours.len().saturating_sub(n);
    let slots: Vec<Slot> = day.hours[first..]
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let at = day.from_ms + ((first + i) as u64) * 3_600_000;
            let label = chrono::DateTime::from_timestamp_millis(at as i64).map(|t| t.with_timezone(&chrono::Local).format("%H").to_string()).unwrap_or_default();
            let cacheable = h[0] + h[1];
            Slot { label, icon: None, value: style_count(h[0] + h[1] + h[2]), level: (cacheable > 0).then(|| h[0] as f32 / cacheable as f32) }
        })
        .collect();
    let tip = format!(
        "Kache: {}{}{}.",
        match (running, day.rate()) {
            (Some(false), _) => "stopped".to_owned(),
            (_, Some(r)) => format!("{}% of what could be cached came from it in the last 24 hours", (r * 100.0).round()),
            _ => "no builds in the last 24 hours".to_owned(),
        },
        {
            let mut all = detail.clone();
            all.extend(snap.daemon.as_ref().map(|d| format!("{} in its store", data::bytes(d.size))));
            if all.is_empty() { String::new() } else { format!("; {}", all.join(", ")) }
        },
        note.as_ref().map(|n| format!("; {}", n.to_lowercase())).unwrap_or_default()
    );
    Glance {
        icon: Some("kache".into()),
        value,
        title: "Kache".into(),
        detail: detail.join(" · "),
        note,
        body: (!slots.is_empty()).then_some(Body::Slots { slots, hue: Some(olo_theme::Hue::Green) }),
        tip,
    }
}

/// A count, short: "840", "1.2k".
fn style_count(n: u32) -> String {
    if n >= 1000 { format!("{:.1}k", n as f64 / 1000.0) } else { n.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_widget_says() {
        let snap = data::made_up(1_794_800_000_000);
        let g = glance(&snap, 6);
        assert_eq!(g.value, format!("{}%", (snap.day.rate().unwrap() * 100.0).round()));
        assert_eq!(g.detail, "9 of 16 slots");
        assert!(g.tip.contains("88.0 GB in its store"), "{}", g.tip);
        assert!(g.note.is_none(), "under its limit, running");
        let Some(Body::Slots { slots, .. }) = &g.body else { panic!("{:?}", g.body) };
        assert_eq!(slots.len(), 6);
        assert!(slots.iter().all(|s| s.level.is_some_and(|l| (0.0..=1.0).contains(&l))));
        assert!(g.tip.starts_with("Kache: ") && g.tip.contains("9 of 16 slots"), "{}", g.tip);
        let mut stopped = snap.clone();
        if let Some(u) = stopped.unit.as_mut() {
            u.active = "inactive".into();
            u.sub = "dead".into();
        }
        let g = glance(&stopped, 12);
        assert_eq!(g.value, "off");
        assert!(g.note.as_deref().is_some_and(|n| n.contains("stopped")));
        let Some(Body::Slots { slots, .. }) = &g.body else { panic!() };
        assert_eq!(slots.len(), 12);
        let mut over = snap.clone();
        if let Some(d) = over.daemon.as_mut() {
            d.size = 200_000_000_000;
        }
        assert!(glance(&over, 6).note.is_some_and(|n| n.contains("over its 102 GB limit")));
        assert_eq!((style_count(840), style_count(1234)), ("840".to_owned(), "1.2k".to_owned()));
    }
}

//! What the window shows: every calendar kept (`olo_calendar::store`),
//! read, each in its colour, and their events' occurrences between two
//! days on the person's own clock.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use jiff::{
    civil::{Date, DateTime},
    tz::TimeZone,
};
use olo_calendar::{
    event::{self, Event, Status, When},
    store::{Kept, Stored},
};
use olo_theme::Hue;

/// A calendar kept, read.
#[derive(Debug, Clone)]
pub struct Cal {
    pub kept: Kept,
    pub events: Vec<Stored>,
}

impl Cal {
    /// Every event of `kept` read (off the UI thread: a calendar can
    /// hold thousands).
    pub fn read(kept: Kept) -> Cal {
        Cal { events: kept.events(), kept }
    }
}

/// What the person chose of their calendars, by each one's address:
/// those not shown, and the colours of their own choosing (a look's
/// colour, by name).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Choices {
    pub hidden: BTreeSet<String>,
    pub hues: BTreeMap<String, String>,
}

/// The colours calendars take in turn where their server says none.
const TURN: [Hue; 8] = [Hue::Blue, Hue::Green, Hue::Red, Hue::Purple, Hue::Orange, Hue::Teal, Hue::Pink, Hue::Brown];

pub fn hue_named(name: &str) -> Option<Hue> {
    Hue::ALL.into_iter().find(|h| h.name().eq_ignore_ascii_case(name))
}

/// The look's colour nearest "#RRGGBB" (a server's colour for a
/// calendar): Olo draws each calendar in one of its look's own colours,
/// so it reads in every look by day and by night.
pub fn hue_of(color: &str) -> Option<Hue> {
    let hex = color.strip_prefix('#')?;
    let n = u32::from_str_radix(hex.get(..6)?, 16).ok()?;
    let (r, g, b) = (((n >> 16) & 255) as f32 / 255.0, ((n >> 8) & 255) as f32 / 255.0, (n & 255) as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let (light, spread) = ((max + min) / 2.0, max - min);
    let saturation = if spread == 0.0 { 0.0 } else { spread / (1.0 - (2.0 * light - 1.0).abs()) };
    if saturation < 0.16 {
        return Some(Hue::Gray);
    }
    let angle = 60.0
        * if max == r {
            ((g - b) / spread).rem_euclid(6.0)
        } else if max == g {
            (b - r) / spread + 2.0
        } else {
            (r - g) / spread + 4.0
        };
    Some(match angle {
        // (A dark, dull orange is brown.)
        a if (12.0..48.0).contains(&a) && light < 0.42 && saturation < 0.75 => Hue::Brown,
        a if a < 12.0 => Hue::Red,
        a if a < 42.0 => Hue::Orange,
        a if a < 68.0 => Hue::Yellow,
        a if a < 160.0 => Hue::Green,
        a if a < 198.0 => Hue::Teal,
        a if a < 258.0 => Hue::Blue,
        a if a < 300.0 => Hue::Purple,
        a if a < 345.0 => Hue::Pink,
        _ => Hue::Red,
    })
}

/// One time an event happens, as the views draw it.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    /// Which calendar (in the book) and which event there.
    pub cal: usize,
    pub href: Arc<str>,
    pub title: Arc<str>,
    pub location: Arc<str>,
    /// On the person's clock; the end isn't part of it.
    pub start: DateTime,
    pub end: DateTime,
    pub all_day: bool,
    pub hue: Hue,
    /// Which of a repeating event's it is.
    pub id: Option<When>,
    pub repeats: bool,
    pub tentative: bool,
    /// The person has been asked to it and hasn't said yes.
    pub unanswered: bool,
    /// Takes no time (shown lighter).
    pub free: bool,
}

impl Shown {
    /// What tells it from every other occurrence shown (a selection's).
    pub fn key(&self) -> (usize, Arc<str>, DateTime) {
        (self.cal, self.href.clone(), self.id.as_ref().map_or(self.start, |w| w.local()))
    }

    /// The days it's on: its first, and its last (an event ending at
    /// midnight isn't on the day that starts then).
    pub fn days(&self) -> (Date, Date) {
        let first = self.start.date();
        let last = if self.end > self.start && self.end.time() == jiff::civil::Time::midnight() { self.end.date().yesterday().unwrap_or(first) } else { self.end.date() };
        (first, last.max(first))
    }

    /// Drawn as a bar across its days (an all-day event, or one on more
    /// than a day), not as a time and a title in one.
    pub fn bar(&self) -> bool {
        let (first, last) = self.days();
        self.all_day || first != last
    }
}

/// Every calendar kept, in the order shown: by account (as they were
/// added), then by name.
#[derive(Debug, Clone, Default)]
pub struct Book {
    pub cals: Vec<Arc<Cal>>,
}

impl Book {
    /// `cals`, with the accounts in `order` (their ids).
    pub fn new(mut cals: Vec<Arc<Cal>>, order: &[String]) -> Book {
        let place = |c: &Cal| order.iter().position(|id| *id == c.kept.account).unwrap_or(usize::MAX);
        cals.sort_by(|a, b| (place(a), a.kept.name.to_lowercase(), &a.kept.href).cmp(&(place(b), b.kept.name.to_lowercase(), &b.kept.href)));
        Book { cals }
    }

    /// With `account`'s calendars now `theirs` (the others as they were,
    /// nothing of them read again).
    pub fn with(&self, account: &str, theirs: Vec<Cal>, order: &[String]) -> Book {
        let others = self.cals.iter().filter(|c| c.kept.account != account).cloned();
        Book::new(others.chain(theirs.into_iter().map(Arc::new)).collect(), order)
    }

    /// Calendar `i`'s colour: the person's own, else the look's nearest
    /// its server's, else one of its own by its place.
    pub fn hue(&self, i: usize, choices: &Choices) -> Hue {
        let Some(c) = self.cals.get(i) else { return Hue::Blue };
        choices.hues.get(&c.kept.href).and_then(|n| hue_named(n)).or_else(|| c.kept.color.as_deref().and_then(hue_of)).unwrap_or(TURN[i % TURN.len()])
    }

    /// The occurrences on the days from `from` to `to` (not `to` itself)
    /// of every calendar shown, in order: by start, the longer first.
    pub fn shown(&self, from: Date, to: Date, zone: &TimeZone, me: &[String], choices: &Choices) -> Vec<Shown> {
        let (Ok(a), Ok(b)) = (from.at(0, 0, 0, 0).to_zoned(zone.clone()), to.at(0, 0, 0, 0).to_zoned(zone.clone())) else { return Vec::new() };
        let mut out = Vec::new();
        for (i, cal) in self.cals.iter().enumerate() {
            if choices.hidden.contains(&cal.kept.href) {
                continue;
            }
            let hue = self.hue(i, choices);
            for stored in &cal.events {
                if stored.event.status == Status::Cancelled {
                    continue;
                }
                let href: Arc<str> = Arc::from(stored.href.as_str());
                for o in event::occurrences(&stored.event, &stored.overrides, &a, &b, zone) {
                    let e: &Event = o.changed.and_then(|i| stored.overrides.get(i)).unwrap_or(&stored.event);
                    let mine = e.attendees.iter().find(|p| me.iter().any(|m| m.eq_ignore_ascii_case(&p.address)));
                    out.push(Shown {
                        cal: i,
                        href: href.clone(),
                        title: Arc::from(if e.summary.trim().is_empty() { "(No title)" } else { e.summary.trim() }),
                        location: Arc::from(e.location.trim()),
                        start: o.start.datetime(),
                        end: o.end.datetime(),
                        all_day: o.all_day,
                        hue,
                        id: o.id,
                        repeats: stored.event.repeats || !stored.event.rdates.is_empty(),
                        tentative: e.status == Status::Tentative,
                        unanswered: mine.is_some_and(|p| p.answer == olo_calendar::event::Answer::NotYet),
                        free: e.free,
                    });
                }
            }
        }
        out.sort_by(|x, y| (x.start, std::cmp::Reverse(x.end), &x.title).cmp(&(y.start, std::cmp::Reverse(y.end), &y.title)));
        out
    }

    /// The event an occurrence is of.
    pub fn stored(&self, shown: &Shown) -> Option<&Stored> {
        self.cals.get(shown.cal)?.events.iter().find(|s| *s.href == *shown.href)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_calendar::store::Item;

    #[test]
    fn a_servers_colour_is_the_looks_nearest() {
        for (color, hue) in [
            ("#4285F4", Hue::Blue),
            ("#0B8043", Hue::Green),
            ("#D50000", Hue::Red),
            ("#F4511E", Hue::Orange),
            ("#F6BF26", Hue::Yellow),
            ("#8E24AA", Hue::Purple),
            ("#E67C73", Hue::Red),
            ("#039BE5", Hue::Blue),
            ("#33B679", Hue::Green),
            ("#616161", Hue::Gray),
            ("#795548", Hue::Brown),
            ("#D81B60", Hue::Pink),
            ("#009688", Hue::Teal),
            ("#3F51B5FF", Hue::Blue),
        ] {
            assert_eq!(hue_of(color), Some(hue), "{color}");
        }
        assert_eq!((hue_of("blue"), hue_of("#12"), hue_named("teal"), hue_named("mauve")), (None, None, Some(Hue::Teal), None));
    }

    fn kept(name: &str, color: Option<&str>, events: &[&str]) -> Kept {
        let mut k = Kept { account: "a".into(), href: format!("https://x/{name}/"), name: name.into(), color: color.map(str::to_owned), ..Default::default() };
        for (i, body) in events.iter().enumerate() {
            let ics = format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:{name}{i}\n{body}\nEND:VEVENT\nEND:VCALENDAR\n");
            k.items.insert(format!("https://x/{name}/{i}.ics"), Item { etag: "e".into(), ics });
        }
        k
    }

    /// Every calendar's occurrences together, in order, each in its
    /// calendar's colour; a hidden calendar's left out; an event ending at
    /// midnight isn't on the next day.
    #[test]
    fn every_calendar_in_one() {
        let zone = TimeZone::get("Europe/Amsterdam").unwrap();
        let home = kept("Home", Some("#0B8043"), &["DTSTART;VALUE=DATE:20261120\nDTEND;VALUE=DATE:20261123\nSUMMARY:Away", "DTSTART:20261120T170000Z\nDTEND:20261120T230000Z\nSUMMARY:Party"]);
        let work = kept(
            "Work",
            None,
            &[
                "DTSTART;TZID=Europe/Amsterdam:20261116T090000\nDTEND;TZID=Europe/Amsterdam:20261116T093000\nRRULE:FREQ=DAILY;COUNT=5\nSUMMARY:Standup",
                "DTSTART:20261120T130000Z\nDTEND:20261120T140000Z\nSUMMARY: ",
            ],
        );
        let order = ["a".to_owned()];
        let mut choices = Choices::default();
        let book = Book::new(vec![Arc::new(Cal::read(work.clone())), Arc::new(Cal::read(home.clone()))], &order);
        assert_eq!(book.cals.iter().map(|c| c.kept.name.as_str()).collect::<Vec<_>>(), ["Home", "Work"]);
        assert_eq!((book.hue(0, &choices), book.hue(1, &choices)), (Hue::Green, Hue::Green));
        let day = |d: i8| jiff::civil::date(2026, 11, d);
        let shown = book.shown(day(20), day(21), &zone, &[], &choices);
        let said: Vec<String> = shown.iter().map(|s| format!("{} {}-{} {:?}", s.title, s.start.time(), s.end.time(), s.hue)).collect();
        assert_eq!(said, ["Away 00:00:00-00:00:00 Green", "Standup 09:00:00-09:30:00 Green", "(No title) 14:00:00-15:00:00 Green", "Party 18:00:00-00:00:00 Green"]);
        // Away is on the 20th to the 22nd; the party, ending at midnight, on the 20th alone.
        assert_eq!((shown[0].days(), shown[0].bar()), ((day(20), day(22)), true));
        assert_eq!((shown[3].days(), shown[3].bar()), ((day(20), day(20)), false));
        assert!(shown[1].repeats && shown[1].id.is_some() && !shown[0].repeats);
        // Each occurrence its own, the event it's of found again.
        assert_ne!(shown[1].key(), book.shown(day(19), day(20), &zone, &[], &choices)[0].key());
        assert_eq!(book.stored(&shown[3]).map(|s| s.event.summary.as_str()), Some("Party"));
        // Hidden: left out. A colour of the person's own: that one.
        choices.hidden.insert(home.href.clone());
        choices.hues.insert(work.href.clone(), "Purple".into());
        assert_eq!(book.shown(day(20), day(21), &zone, &[], &choices).iter().map(|s| (&*s.title, s.hue)).collect::<Vec<_>>(), [("Standup", Hue::Purple), ("(No title)", Hue::Purple)]);
        // An account's calendars replaced: the others as they were, the very same.
        let again = book.with("b", vec![Cal::read(Kept { account: "b".into(), name: "Other".into(), ..Default::default() })], &["b".to_owned(), "a".to_owned()]);
        assert_eq!(again.cals.iter().map(|c| c.kept.name.as_str()).collect::<Vec<_>>(), ["Other", "Home", "Work"]);
        assert!(Arc::ptr_eq(&again.cals[1], &book.cals[0]));
    }

    /// What working out a view's occurrences costs with a great many
    /// events (it runs on a worker; this says how long the window shows
    /// the days before their events). `cargo test --release -p
    /// olo-calendar-app shown_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn shown_cost() {
        let zone = TimeZone::get("Europe/Amsterdam").unwrap();
        let mut cals = Vec::new();
        for c in 0..4 {
            let mut events = Vec::new();
            for i in 0..2500u32 {
                // Spread over eight years; a third repeat weekly, from their day on.
                let day = jiff::civil::date(2019, 1, 1).checked_add(jiff::Span::new().days((i as i64 * 7919 + c * 13) % 2900)).unwrap();
                let (h, rule) = (8 + (i % 10) as i8, if i % 3 == 0 { "\nRRULE:FREQ=WEEKLY" } else { "" });
                events.push(format!("DTSTART;TZID=Europe/Amsterdam:{}T{h:02}0000\nDTEND;TZID=Europe/Amsterdam:{}T{h:02}4500{rule}\nSUMMARY:Event {i}", day.strftime("%Y%m%d"), day.strftime("%Y%m%d")));
            }
            let refs: Vec<&str> = events.iter().map(String::as_str).collect();
            let t = std::time::Instant::now();
            let cal = Cal::read(kept(&format!("Cal{c}"), None, &refs));
            eprintln!("read a calendar of {} events in {:.1} ms", cal.events.len(), t.elapsed().as_secs_f64() * 1e3);
            cals.push(Arc::new(cal));
        }
        let book = Book::new(cals, &["a".to_owned()]);
        let choices = Choices::default();
        for (what, from, days) in [("a week", jiff::civil::date(2026, 11, 16), 7), ("a month and as much either side", jiff::civil::date(2026, 9, 21), 126)] {
            let to = from.checked_add(jiff::Span::new().days(days)).unwrap();
            let t = std::time::Instant::now();
            let shown = book.shown(from, to, &zone, &[], &choices);
            eprintln!("{what}: {} occurrences of 10,000 events in {:.1} ms", shown.len(), t.elapsed().as_secs_f64() * 1e3);
        }
    }
}

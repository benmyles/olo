//! The clock's menu (Ben, 2026-11-17: Calendar "integrates with all the
//! olo stuff"): the day in full, what's still to come today and
//! tomorrow in Calendar (each a click from Calendar, open on it),
//! Calendar itself, and the Date & Time settings. BeOS's clock showed
//! the date at a click and its menu changed the time; Haiku's opens a
//! month: here it's what the day holds.
//!
//! Read when the clock is clicked from the small file Calendar's
//! service keeps (`olo_protocol::calendar`): nothing is followed or
//! asked meanwhile, and Deskbar reads no calendar itself.

use olo_kit::{Hsla, menu::Menu};
use olo_protocol::calendar::{Coming, Event};

/// The most events listed for today, for tomorrow, and for the next day
/// that has any when neither does.
const TODAY: usize = 8;
const TOMORROW: usize = 5;
const LATER: usize = 3;
/// A title is cut past this many letters.
const TITLE: usize = 44;

/// What the menu is made from, beside the events: the moment, and how
/// this computer says a day and a time.
pub struct Clock<'a> {
    /// Now (seconds since 1970).
    pub now: i64,
    /// The local day a moment is in, as a number (the next day one more).
    pub day: &'a dyn Fn(i64) -> i64,
    /// A moment's time of day, as the clock writes it ("2:00 PM", "14:00").
    pub time: &'a dyn Fn(i64) -> String,
    /// A moment's day in full ("Wednesday, November 18, 2026"), and
    /// without the year.
    pub date: &'a dyn Fn(i64, bool) -> String,
    /// A calendar's colour here.
    pub hue: &'a dyn Fn(olo_kit::palette::Hue) -> Hsla,
}

fn cut(title: &str) -> String {
    if title.chars().count() <= TITLE { title.to_owned() } else { format!("{}…", title.chars().take(TITLE - 1).collect::<String>().trim_end()) }
}

/// An event's line: when (on now: until when; a whole day's: "All
/// day"), then its title.
fn line(e: &Event, c: &Clock) -> String {
    let when = if e.all_day {
        olo_kit::words::tr("All day").to_owned()
    } else if e.start <= c.now {
        format!("Now, until {}", (c.time)(e.end))
    } else {
        (c.time)(e.start)
    };
    format!("{when} · {}", cut(&e.title))
}

/// What a click on an event runs: Calendar, open on it.
fn shows(e: &Event) -> String {
    let words: Vec<String> = e.shows().iter().enumerate().map(|(i, w)| if i == 0 { w.clone() } else { crate::glances::quoted(w) }).collect();
    format!("run:{}", words.join(" "))
}

/// The clock's menu at `c.now`, from what Calendar last said is coming
/// (None: it never has).
pub fn menu(coming: Option<&Coming>, c: &Clock) -> Menu {
    let mut menu = Menu::new("Clock").item((c.date)(c.now, true), "").enabled(false);
    let today = (c.day)(c.now);
    if let Some(coming) = coming.filter(|c| c.set_up) {
        // (One begun before today and not over is on today.)
        let on = |day: i64| coming.from(c.now).filter(move |e| (c.day)(e.start).max(today) == day).collect::<Vec<&Event>>();
        let list = |mut menu: Menu, events: &[&Event], most: usize| {
            for e in events.iter().take(most) {
                menu = menu.item(line(e, c), shows(e));
                if let Some(hue) = e.hue {
                    menu = menu.swatch((c.hue)(hue));
                }
            }
            if events.len() > most {
                let more = events.len() - most;
                menu = menu.item(olo_kit::words::count(more as u64, "1 More in Calendar…", "{n} More in Calendar…"), "run:olo-calendar");
            }
            menu
        };
        menu = menu.separator();
        let (now, next) = (on(today), on(today + 1));
        menu = if now.is_empty() { menu.item("Nothing more today", "").enabled(false) } else { list(menu, &now, TODAY) };
        if !next.is_empty() {
            menu = list(menu.separator().item("Tomorrow", "").enabled(false), &next, TOMORROW);
        } else if now.is_empty()
            && let Some(first) = coming.from(c.now).find(|e| (c.day)(e.start) > today + 1)
        {
            // Nothing today or tomorrow: the next day that has anything.
            let day = (c.day)(first.start);
            menu = list(menu.separator().item((c.date)(first.start, false), "").enabled(false), &on(day), LATER);
        }
    }
    menu.separator().item("Open Calendar", "run:olo-calendar").separator().settings_item("datetime", "pref:datetime")
}

#[cfg(test)]
mod tests {
    use super::*;
    use olo_kit::menu::MenuItem;

    const DAY: i64 = 86_400;
    /// Wednesday 2026-11-18, 10:30 (a clock on UTC).
    const NOW: i64 = 1_794_997_800;

    fn event(title: &str, start: i64, length: i64) -> Event {
        Event {
            href: format!("https://x/{title}.ics"),
            starts: "2026-11-18T14:00:00".into(),
            title: title.into(),
            start,
            end: start + length,
            hue: Some(olo_kit::palette::Hue::Blue),
            ..Event::default()
        }
    }

    fn said(coming: Option<&Coming>, now: i64) -> Vec<String> {
        let time = |t: i64| format!("{:02}:{:02}", t.rem_euclid(DAY) / 3600, t.rem_euclid(3600) / 60);
        let date = |t: i64, year: bool| format!("day {}{}", t.div_euclid(DAY) - NOW.div_euclid(DAY), if year { " 2026" } else { "" });
        let c = Clock { now, day: &|t| t.div_euclid(DAY), time: &time, date: &date, hue: &|_| Hsla::default() };
        menu(coming, &c)
            .items
            .iter()
            .map(|i| match i {
                MenuItem::Action(a) => format!("{}{}{}", if a.enabled { "" } else { "(" }, a.label, if a.swatch.is_some() { " ■" } else { "" }),
                MenuItem::Separator => "-".to_owned(),
                MenuItem::Submenu(m) => format!("{} ▸", m.title),
            })
            .collect()
    }

    /// The day; what's still to come today (one on now says until
    /// when, a whole day's "All day", one begun yesterday too) and
    /// tomorrow; Calendar; the settings. What's over isn't listed.
    #[test]
    fn the_day_and_what_it_holds() {
        let midnight = NOW - NOW.rem_euclid(DAY);
        let mut away = event("Away", midnight - DAY, 3 * DAY);
        away.all_day = true;
        let coming = Coming {
            set_up: true,
            until: midnight + 14 * DAY,
            events: vec![
                away,
                event("Over", midnight + 8 * 3600, 3600),
                event("Under way", midnight + 10 * 3600, 3600),
                event("Design review", midnight + 14 * 3600, 5400),
                event("Standup", midnight + DAY + 9 * 3600 + 1800, 900),
            ],
        };
        assert_eq!(
            said(Some(&coming), NOW),
            [
                "(day 0 2026",
                "-",
                "All day · Away ■",
                "Now, until 11:00 · Under way ■",
                "14:00 · Design review ■",
                "-",
                "(Tomorrow",
                "09:30 · Standup ■",
                "-",
                "Open Calendar",
                "-",
                "Date & Time Settings…"
            ]
        );
        // In the evening: nothing more today, tomorrow still.
        let evening = said(Some(&Coming { events: coming.events[1..].to_vec(), ..coming.clone() }), midnight + 20 * 3600);
        assert_eq!(evening[..5], ["(day 0 2026", "-", "(Nothing more today", "-", "(Tomorrow"]);
        // What a click runs.
        let c = Clock { now: NOW, day: &|t| t.div_euclid(DAY), time: &|_| String::new(), date: &|_, _| String::new(), hue: &|_| Hsla::default() };
        let MenuItem::Action(first) = &menu(Some(&coming), &c).items[2] else { panic!() };
        assert_eq!(first.command.as_ref(), "run:olo-calendar '--show' 'https://x/Away.ics' '2026-11-18T14:00:00'");
    }

    /// No account, or Calendar never heard from: the day, Calendar and
    /// the settings, and nothing said of events. Nothing today or
    /// tomorrow: the next day that has anything, by its date. More than
    /// fit: how many more, in Calendar.
    #[test]
    fn nothing_and_too_much() {
        let plain = ["(day 0 2026", "-", "Open Calendar", "-", "Date & Time Settings…"];
        assert_eq!(said(None, NOW), plain);
        assert_eq!(said(Some(&Coming::default()), NOW), plain);
        let later =
            Coming { set_up: true, until: NOW + 14 * DAY, events: vec![event("Offsite", NOW + 5 * DAY, 3600), event("Dinner", NOW + 5 * DAY + 7200, 3600), event("Flight", NOW + 9 * DAY, 3600)] };
        assert_eq!(said(Some(&later), NOW)[..7], ["(day 0 2026", "-", "(Nothing more today", "-", "(day 5", "10:30 · Offsite ■", "12:30 · Dinner ■"]);
        let full = Coming { set_up: true, until: NOW + 14 * DAY, events: (0..11).map(|i| event(&format!("E{i}"), NOW + 600 * (i + 1), 300)).collect() };
        let list = said(Some(&full), NOW);
        assert_eq!((list[9].as_str(), list[10].as_str(), list[11].as_str()), ("10:30 · E7 ■".replace("10:30", "11:50").as_str(), "3 More in Calendar…", "-"));
        // A long title is cut.
        let long = Coming { set_up: true, until: NOW + DAY, events: vec![event(&"Quarterly planning ".repeat(5), NOW + 600, 300)] };
        assert!(said(Some(&long), NOW)[2].ends_with("… ■") && said(Some(&long), NOW)[2].chars().count() < 60);
    }
}

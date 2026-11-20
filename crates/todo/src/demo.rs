//! A made-up set of lists, for pictures, the layout lint and trying
//! To-Do with nothing of one's own (`OLO_TODO_DEMO=1`, into an empty
//! `OLO_TODO_DIR`): a household's, a job's with a board's sections, a
//! shopping list and an empty one, their dues laid round a day so the
//! same pictures come out whenever they're taken.

use std::path::Path;

use jiff::{Timestamp, Zoned};
use olo_calendar::event::When;
use olo_theme::Hue;

use crate::{
    item::{Attachment, Remind, Status, ToDo},
    store::{self, List},
};

struct Made<'a> {
    title: &'a str,
    /// Days from today, and a time on that day (hour, minute).
    due: Option<(i64, Option<(i8, i8)>)>,
    remind: Option<i64>,
    section: Option<&'a str>,
    notes: &'a str,
    link: Option<&'a str>,
    done: bool,
}

const fn made(title: &str) -> Made<'_> {
    Made { title, due: None, remind: None, section: None, notes: "", link: None, done: false }
}

/// The made-up lists written under `root`, round `now`'s day. Nothing,
/// if `root` has a list already.
pub fn fill(root: &Path, now: &Zoned) -> std::io::Result<()> {
    if std::fs::read_dir(root).is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.path().is_dir()))) {
        return Ok(());
    }
    let lists: [(&str, Hue, &[&str], Vec<Made>); 4] = [
        (
            "Home",
            Hue::Green,
            &[],
            vec![
                Made { due: Some((-1, None)), notes: "The hinge at the top is loose.\nScrews are in the shed.", ..made("Fix the garden gate") },
                Made { due: Some((0, None)), ..made("Put the bins out") },
                Made { due: Some((0, Some((15, 0)))), remind: Some(900), notes: "Ask about Tuesday", link: Some("https://example.com/trip"), ..made("Call Ann about the trip") },
                Made { due: Some((2, None)), ..made("Book the dentist") },
                Made { due: Some((11, None)), remind: Some(-9 * 3600), ..made("Renew the car insurance before it lapses at the end of the month") },
                made("Water the plants"),
                Made { due: Some((-2, None)), done: true, ..made("Sweep the porch") },
            ],
        ),
        (
            "Work",
            Hue::Teal,
            &["To Do", "Doing"],
            vec![
                Made { due: Some((0, Some((17, 0)))), remind: Some(3600), section: Some("Doing"), notes: "Cover the three options", ..made("Write up the design review") },
                Made { due: Some((1, None)), section: Some("To Do"), ..made("Reply to the vendor") },
                Made { due: Some((4, None)), section: Some("To Do"), ..made("Plan next sprint with the team and draft the goals") },
                Made { section: Some("Doing"), ..made("Fix the flaky login test") },
                Made { section: Some("To Do"), notes: "Screenshots are out of date", ..made("Update the onboarding guide") },
                Made { due: Some((-3, None)), section: Some("Doing"), done: true, ..made("Ship the settings redesign") },
                Made { due: Some((6, None)), section: Some("To Do"), link: Some("https://example.com/expenses"), ..made("Expense report") },
            ],
        ),
        ("Shopping", Hue::Orange, &[], vec![made("Olive oil"), made("Coffee beans"), Made { due: Some((3, None)), ..made("A birthday card for Sam") }]),
        ("Reading", Hue::Purple, &[], Vec::new()),
    ];
    let today = now.date();
    for (n, (name, hue, sections, todos)) in lists.into_iter().enumerate() {
        // (Folders named so they sort as listed, and are the same each time.)
        let id = format!("demo-{n}-{}", name.to_lowercase());
        let list = List { id: id.clone(), name: name.to_owned(), hue, sections: sections.iter().map(|s| (*s).to_owned()).collect() };
        std::fs::create_dir_all(root.join(&id))?;
        store::change_list(root, None, &list)?;
        for (i, m) in todos.into_iter().enumerate() {
            let mut todo = ToDo::new(&format!("{id}-{i}@olo"), m.title);
            todo.notes = m.notes.to_owned();
            todo.order = (i as i64 + 1) * 1024;
            todo.section = m.section.map(str::to_owned);
            todo.due = m.due.and_then(|(days, time)| {
                let day = today.checked_add(jiff::Span::new().days(days)).ok()?;
                Some(match time {
                    Some((h, min)) => When::Zoned(day.at(h, min, 0, 0).to_zoned(now.time_zone().clone()).ok()?),
                    None => When::Date(day),
                })
            });
            todo.remind = m.remind.map(Remind::Before);
            if let Some(uri) = m.link {
                todo.attachments.push(Attachment { uri: uri.to_owned(), name: uri.trim_start_matches("https://").to_owned(), mime: None });
            }
            if m.done {
                todo.status = Status::Done;
                todo.done_at = now.checked_sub(jiff::Span::new().hours(14)).ok().map(|z| z.timestamp());
            }
            let path = root.join(&id).join(store::file_name(&todo.uid));
            std::fs::write(path, crate::item::new_file(&todo, now.checked_sub(jiff::Span::new().days(2)).map_or_else(|_| Timestamp::now(), |z| z.timestamp())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_made_up_lists() {
        let root = std::env::current_exe().unwrap().parent().unwrap().join(format!("olo-todo-demo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let now = jiff::civil::date(2026, 11, 19).at(10, 30, 0, 0).to_zoned(jiff::tz::TimeZone::UTC).unwrap();
        fill(&root, &now).unwrap();
        let book = store::Book::load(&root);
        assert_eq!(book.lists.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["Home", "Work", "Shopping", "Reading"]);
        assert_eq!(book.items.len(), 17);
        assert_eq!(book.items.iter().filter(|i| !i.todo.open()).count(), 2);
        let call = book.item("demo-0-home-2@olo").unwrap();
        assert_eq!(crate::item::remind_at(&call.todo, &jiff::tz::TimeZone::UTC), crate::item::moment("20261119T144500Z"));
        // Filled once: a second time changes nothing (never over a person's own).
        let before = book.clone();
        fill(&root, &now).unwrap();
        assert_eq!(store::Book::load(&root), before);
        let _ = std::fs::remove_dir_all(&root);
    }
}

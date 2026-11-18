//! Notifications' widget for Deskbar (`olo-notifications --glance`, no
//! window: `olo_kit::glance::serve`): the Inbox, what is unread and what
//! waits in apps, as a stack of cards to work through
//! (`olo_protocol::glance::Body::Inbox`, drawn by `olo_kit::glance`).
//! Made again whenever the service says what is unread changed
//! (`NotificationState`, which carries the cards); Deskbar asks the
//! service to open, snooze or dismiss one (`--invoke`, `--snooze`,
//! `--dismiss`).

use olo_protocol::{
    NotificationState,
    glance::{self, Body, Entry, Glance, Timeline},
};

pub fn serve() -> std::process::ExitCode {
    olo_kit::glance::serve("notifications", vec![NotificationState::path()], |requests| {
        let state = NotificationState::load().unwrap_or_default();
        let Some(kind) = glance::kind("notifications", "inbox") else { return (Vec::new(), None) };
        let made = requests
            .iter()
            .map(|r| {
                let option = |name| kind.value(&r.options, name);
                let mut cards: Vec<_> = state
                    .cards
                    .iter()
                    .filter(|c| match option("source") {
                        "attention" => c.attention,
                        "messages" => !c.attention,
                        _ => true,
                    })
                    .cloned()
                    .collect();
                if option("order") == "old" {
                    cards.reverse();
                }
                let (n, red) = (cards.len(), cards.iter().filter(|c| c.attention).count());
                let tip = match (n, red) {
                    (0, _) => "Nothing new".to_owned(),
                    (n, 0) => format!("{n} to read"),
                    (n, red) if red == n => format!("{n} waiting in {}", if n == 1 { "its app" } else { "their apps" }),
                    (n, red) => format!("{n} to deal with, {red} waiting in {}", if red == 1 { "its app" } else { "their apps" }),
                };
                let body = Body::Inbox { cards, snooze_minutes: option("snooze").parse().unwrap_or(15), preview: option("detail") == "full" };
                let glance = Glance { icon: Some("notifications".into()), value: n.to_string(), title: "Inbox".into(), tip, body: Some(body), ..Default::default() };
                (r.widget, Timeline { made: 0, entries: vec![Entry { from: 0, glance }] })
            })
            .collect();
        (made, None)
    })
}

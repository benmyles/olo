//! The phone's other notifications (an iPhone's, over Apple's
//! Notification Center Service: `olo_system::phone::ancs`): which of
//! them are shown here, and what's asked of the phone to show them.
//!
//! The phone says a notification was added, changed or removed, by a
//! number; what it says is asked for, one question at a time (an answer
//! comes in pieces on one stream, so two asked at once couldn't be told
//! apart), and its app's name the first time the app is met. Then it's
//! shown, with its actions: an incoming call's Answer and Decline, a
//! notification's Clear. When the phone removes it (read there, the
//! call ended), it goes here too.
//!
//! Not shown: what was on the phone before this computer listened
//! (old news, but for a call ringing now), and text messages while they
//! come the other way, whole (the Message Access Profile), with their
//! own notification.
//!
//! Only what to do is decided here ([`Effect`]): the service writes to
//! the phone and shows the notifications.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{Duration, Instant},
};

use olo_system::phone::ancs::{self, Answer, Category, Event, Notification, Told};

/// How long an answer is waited for before the question is given up on
/// (the link went quiet, the notification went meanwhile).
const ANSWER_WITHIN: Duration = Duration::from_secs(5);

/// No more questions than this wait at once (a phone with a flood of
/// them: the oldest are let go).
const MOST_WAITING: usize = 64;

/// A notification to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The phone's number for it.
    pub uid: u32,
    /// Its app, as the phone names it ("Mail").
    pub app: String,
    pub title: String,
    pub body: String,
    /// Its actions' labels, where it has them.
    pub positive: Option<String>,
    pub negative: Option<String>,
    /// An incoming call: it stays until answered, declined or over.
    pub call: bool,
    /// Delivered quietly on the phone.
    pub quiet: bool,
}

/// What the service is to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Write this to the phone's Control Point.
    Write(Vec<u8>),
    /// Show it (again, if it's shown already: it changed).
    Show(Shown),
    /// The phone removed it.
    Close(u32),
}

#[derive(Debug, Default)]
pub struct Others {
    /// Text messages come whole another way: theirs aren't shown twice.
    pub messages_elsewhere: bool,
    /// Apps' names, by their identifiers.
    names: HashMap<String, String>,
    /// The notifications to be shown, as the phone last told of each.
    live: HashMap<u32, Told>,
    /// Answered, their app's name still asked for.
    unnamed: Vec<Notification>,
    /// Questions not yet asked, and since when one is out.
    queue: VecDeque<Vec<u8>>,
    asked: Option<Instant>,
    reader: ancs::Reader,
    shown: HashSet<u32>,
}

impl Others {
    /// The phone told of a notification (a Notification Source packet).
    pub fn told(&mut self, packet: &[u8], now: Instant) -> Vec<Effect> {
        let Some(t) = ancs::told(packet) else { return Vec::new() };
        let mut effects = Vec::new();
        match t.event {
            Event::Removed => {
                self.live.remove(&t.uid);
                self.unnamed.retain(|n| n.uid != t.uid);
                if self.shown.remove(&t.uid) {
                    effects.push(Effect::Close(t.uid));
                }
            }
            // (There before this computer listened: old news, but for
            // a call ringing now.)
            Event::Added if t.pre_existing && t.category != Category::IncomingCall => {}
            // (A change to one that isn't shown: nothing to change.)
            Event::Modified if !self.live.contains_key(&t.uid) => {}
            Event::Added | Event::Modified => {
                self.live.insert(t.uid, t);
                self.ask(ancs::get_notification(t.uid));
            }
        }
        self.next(now, &mut effects);
        effects
    }

    /// An answer's packet came (the Data Source).
    pub fn data(&mut self, packet: &[u8], now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        for answer in self.reader.push(packet) {
            self.asked = None;
            match answer {
                Answer::Notification(n) => {
                    if !self.live.contains_key(&n.uid) || (self.messages_elsewhere && n.app == ancs::MESSAGES_APP) {
                        // (Its shape alone, never what it says: what a
                        // group's message looks like here isn't known
                        // yet. TODO, "Groups' names".)
                        if n.app == ancs::MESSAGES_APP {
                            tracing::info!("phone: a text message's notification has {} subtitle", if n.subtitle.is_empty() { "no" } else { "a" });
                        }
                        self.live.remove(&n.uid);
                        continue;
                    }
                    if self.names.contains_key(&n.app) {
                        self.show(n, &mut effects);
                    } else {
                        if !self.unnamed.iter().any(|u| u.app == n.app) {
                            self.ask(ancs::get_app(&n.app));
                        }
                        self.unnamed.push(n);
                    }
                }
                Answer::App(id, name) => {
                    // (An app with no name of its own: its identifier's
                    // last word.)
                    let name = if name.is_empty() { id.rsplit('.').next().unwrap_or(&id).to_owned() } else { name };
                    self.names.insert(id.clone(), name);
                    let (named, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.unnamed).into_iter().partition(|n| n.app == id);
                    self.unnamed = rest;
                    for n in named {
                        self.show(n, &mut effects);
                    }
                }
            }
        }
        self.next(now, &mut effects);
        effects
    }

    /// Do a shown notification's action on the phone (Answer, Decline,
    /// Clear): nothing if the phone has removed it since.
    pub fn act(&self, uid: u32, positive: bool) -> Vec<Effect> {
        if self.live.contains_key(&uid) { vec![Effect::Write(ancs::act(uid, positive))] } else { Vec::new() }
    }

    /// Time passed: a question never answered is given up on, and the
    /// next asked.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let mut effects = Vec::new();
        if self.asked.is_some_and(|at| now.duration_since(at) >= ANSWER_WITHIN) {
            self.asked = None;
            self.reader.reset();
        }
        self.next(now, &mut effects);
        effects
    }

    /// When `tick` next has something to do.
    pub fn due(&self) -> Option<Instant> {
        self.asked.map(|at| at + ANSWER_WITHIN)
    }

    fn ask(&mut self, command: Vec<u8>) {
        if self.queue.contains(&command) {
            return;
        }
        if self.queue.len() >= MOST_WAITING {
            self.queue.pop_front();
        }
        self.queue.push_back(command);
    }

    /// Ask the next question, if none is out.
    fn next(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        if self.asked.is_none()
            && let Some(command) = self.queue.pop_front()
        {
            self.asked = Some(now);
            effects.push(Effect::Write(command));
        }
    }

    fn show(&mut self, n: Notification, effects: &mut Vec<Effect>) {
        let Some(t) = self.live.get(&n.uid) else { return };
        let app = self.names.get(&n.app).cloned().unwrap_or_default();
        let call = t.category == Category::IncomingCall;
        let label = |has: bool, said: &str, or: &str| has.then(|| if said.is_empty() { or.to_owned() } else { said.to_owned() });
        let body = [n.subtitle.as_str(), n.message.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n");
        self.shown.insert(n.uid);
        effects.push(Effect::Show(Shown {
            uid: n.uid,
            title: if n.title.is_empty() { app.clone() } else { n.title },
            app,
            body,
            positive: label(t.positive, &n.positive, if call { "Answer" } else { "Open" }),
            negative: label(t.negative, &n.negative, if call { "Decline" } else { "Clear" }),
            call,
            quiet: t.silent,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn told(event: Event, uid: u32, category: Category, pre_existing: bool, actions: (bool, bool)) -> Vec<u8> {
        ancs::told_bytes(&Told { event, silent: false, important: false, pre_existing, positive: actions.0, negative: actions.1, category, uid }, 1)
    }

    fn says(uid: u32, app: &str, title: &str, message: &str, labels: (&str, &str)) -> Vec<u8> {
        ancs::answer_bytes(&Answer::Notification(Notification {
            uid,
            app: app.into(),
            title: title.into(),
            subtitle: String::new(),
            message: message.into(),
            positive: labels.0.into(),
            negative: labels.1.into(),
        }))
    }

    /// A notification from an app not met before: what it says is asked,
    /// then the app's name, then it's shown; the next from that app at
    /// once; removed on the phone, it goes.
    #[test]
    fn asked_named_shown_and_gone() {
        let (mut o, now) = (Others::default(), Instant::now());
        assert_eq!(o.told(&told(Event::Added, 5, Category::Other, false, (false, true)), now), [Effect::Write(ancs::get_notification(5))]);
        // (A second meanwhile waits its turn.)
        assert_eq!(o.told(&told(Event::Added, 6, Category::Other, false, (false, true)), now), []);
        let mail = says(5, "com.apple.mobilemail", "Jun Tanaka", "Photos are up", ("", "Clear"));
        // Its answer in two pieces: whole at the second, and the app's
        // name asked next (before the second notification).
        assert_eq!(o.data(&mail[..20], now), []);
        assert_eq!(o.data(&mail[20..], now), [Effect::Write(ancs::get_notification(6))]);
        assert_eq!(o.data(&says(6, "com.apple.mobilemail", "Ada", "Lunch?", ("", "Clear")), now), [Effect::Write(ancs::get_app("com.apple.mobilemail"))]);
        let shown = o.data(&ancs::answer_bytes(&Answer::App("com.apple.mobilemail".into(), "Mail".into())), now);
        let show = |uid: u32, title: &str, body: &str| {
            Effect::Show(Shown { uid, app: "Mail".into(), title: title.into(), body: body.into(), positive: None, negative: Some("Clear".into()), call: false, quiet: false })
        };
        assert_eq!(shown, [show(5, "Jun Tanaka", "Photos are up"), show(6, "Ada", "Lunch?")]);
        // Changed on the phone: asked again and shown again.
        assert_eq!(o.told(&told(Event::Modified, 6, Category::Other, false, (false, true)), now), [Effect::Write(ancs::get_notification(6))]);
        assert_eq!(o.data(&says(6, "com.apple.mobilemail", "Ada", "Lunch at one?", ("", "Clear")), now), [show(6, "Ada", "Lunch at one?")]);
        assert_eq!(o.act(6, false), [Effect::Write(ancs::act(6, false))]);
        assert_eq!(o.told(&told(Event::Removed, 6, Category::Other, false, (false, false)), now), [Effect::Close(6)]);
        assert_eq!(o.act(6, false), [], "gone: nothing to do");
        assert_eq!(o.told(&told(Event::Removed, 6, Category::Other, false, (false, false)), now), [], "removed once");
    }

    /// What was there before isn't shown, but for a call ringing; text
    /// messages aren't while they come the other way; a change to
    /// something not shown is nothing.
    #[test]
    fn what_is_not_shown() {
        let (mut o, now) = (Others::default(), Instant::now());
        assert_eq!(o.told(&told(Event::Added, 1, Category::Other, true, (false, true)), now), []);
        assert_eq!(o.told(&told(Event::Modified, 1, Category::Other, false, (false, true)), now), []);
        assert_eq!(o.told(&told(Event::Removed, 1, Category::Other, false, (false, false)), now), []);
        // A call ringing as the link came: shown, with its actions.
        assert_eq!(o.told(&told(Event::Added, 2, Category::IncomingCall, true, (true, true)), now), [Effect::Write(ancs::get_notification(2))]);
        assert_eq!(o.data(&says(2, "com.apple.mobilephone", "Mira Kovač", "Incoming Call", ("", "")), now), [Effect::Write(ancs::get_app("com.apple.mobilephone"))]);
        let shown = o.data(&ancs::answer_bytes(&Answer::App("com.apple.mobilephone".into(), String::new())), now);
        assert_eq!(
            shown,
            [Effect::Show(Shown {
                uid: 2,
                app: "mobilephone".into(),
                title: "Mira Kovač".into(),
                body: "Incoming Call".into(),
                positive: Some("Answer".into()),
                negative: Some("Decline".into()),
                call: true,
                quiet: false
            })]
        );
        assert_eq!(o.act(2, true), [Effect::Write(ancs::act(2, true))]);
        // A text message, while messages come whole the other way.
        o.messages_elsewhere = true;
        assert_eq!(o.told(&told(Event::Added, 3, Category::Other, false, (false, true)), now), [Effect::Write(ancs::get_notification(3))]);
        assert_eq!(o.data(&says(3, ancs::MESSAGES_APP, "Mira Kovač", "On my way", ("", "Clear")), now), []);
        assert_eq!(o.told(&told(Event::Removed, 3, Category::Other, false, (false, false)), now), []);
        // And when they don't: shown as any other.
        o.messages_elsewhere = false;
        o.told(&told(Event::Added, 4, Category::Other, false, (false, true)), now);
        assert_eq!(o.data(&says(4, ancs::MESSAGES_APP, "Mira Kovač", "On my way", ("", "Clear")), now), [Effect::Write(ancs::get_app(ancs::MESSAGES_APP))]);
    }

    /// A question never answered is given up on after a while, half an
    /// answer with it, and the next is asked.
    #[test]
    fn a_question_never_answered() {
        let (mut o, now) = (Others::default(), Instant::now());
        o.told(&told(Event::Added, 5, Category::Other, false, (false, false)), now);
        o.told(&told(Event::Added, 6, Category::Other, false, (false, false)), now);
        assert_eq!(o.due(), Some(now + ANSWER_WITHIN));
        assert_eq!(o.data(&says(5, "a.b", "T", "M", ("", ""))[..9], now), []);
        assert_eq!(o.tick(now + Duration::from_secs(1)), []);
        assert_eq!(o.tick(now + ANSWER_WITHIN), [Effect::Write(ancs::get_notification(6))]);
        // (The next answer is read from its start, not onto the half.)
        assert_eq!(o.data(&says(6, "a.b", "T", "M", ("", "")), now + ANSWER_WITHIN), [Effect::Write(ancs::get_app("a.b"))]);
    }
}

//! `olo-phone --service`: the session's phone service. It follows
//! BlueFerry's backend on the session bus (its signals: nothing is
//! polled), publishes what it knows ([`PhoneState`], none of it what a
//! message says), tells of each message as it comes in a notification of
//! Olo's own (answered in place), finds the one-time code in one
//! (codes.rs) and gets it to the browser tab that is waiting for it:
//! every browser's extension host stays connected to the service's
//! socket, is asked which of its tabs are waiting for a code when one
//! comes, and says so by itself when a code field comes up; the service
//! chooses among them all (`codes::choose`) and only the chosen browser
//! is given the code. A tab on an agent's screen gets it by the person's
//! setting (put in at once when it surely is that tab's, the person told;
//! else asked first, or never), and what it got goes in that screen's
//! history, without the code.
//!
//! **It starts the backend and keeps it running** (Ben, 2026-11-16: "olo
//! should handle starting and keeping it running"). BlueFerry's unit
//! starts only once a phone is paired (its configuration is there), and
//! then with each session; here, once it's installed and paired and not
//! on the bus (just paired, just installed, stopped, crashed past
//! systemd's own restarts), it is asked for (`start`: the bus starts
//! it), on a thread, again after 5 s, 30 s, 2 min and then every 10 min
//! while it won't start, saying why in the state (`trouble`).
//!
//! A code is kept in memory only, for [`CODE_LIFE`], wiped when it's used
//! or too old. Nothing a message says is logged or written to disk here.

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use olo_kit_base::notify::{Note, NoteEvent, NoteId, Notifier};
use olo_protocol::{
    notifications::NoteStyle,
    phone::{self, Backend, FromBrowser, Link, Notify, PhoneSettings, PhoneState, Reply, Request, ToBrowser, Waiting},
};
use olo_system::blueferry::{self, Client, Event, Thread};
use zeroize::Zeroize;

use crate::codes::{self, Code, How};

/// How long a code can be used after it came.
pub const CODE_LIFE: Duration = Duration::from_secs(5 * 60);
/// How long the browsers have to say which tabs are waiting.
const ROUND: Duration = Duration::from_millis(700);
/// A message is one that has just come if it's no older than this (and
/// not from more than a minute ahead: the phone's clock and this one's).
const JUST: i64 = 5 * 60;
/// The conversations looked through when the history changes (the
/// newest; a new message is in one of them).
const THREADS: u32 = 50;

enum Ev {
    Bus(Event),
    Settings,
    /// The backend's files came or went (installed, removed, updated),
    /// or its configuration did (a phone paired, or forgotten).
    Installed,
    /// Starting the backend came to this (`None`: it's up).
    Started(Option<String>),
    Ask(Request, bool, mpsc::Sender<Reply>),
    Hello(u64, Option<String>, u32, UnixStream),
    Said(u64, FromBrowser),
    Left(u64),
    Note(NoteEvent),
    /// A reply sent from a notification came to this.
    Sent(NoteId, Result<(), String>),
}

struct Browser {
    /// The agent's screen its Chrome is on, if it is on one.
    screen: Option<String>,
    out: UnixStream,
}

/// The browsers being asked which tabs wait for a code.
struct Round {
    waiting: HashSet<u64>,
    tabs: Vec<(u64, Waiting)>,
    until: Instant,
}

/// A code that has come and can still be used.
struct Pending {
    id: u64,
    code: Code,
    /// Who it's from, as told: the service the message names, else the
    /// conversation's name.
    from: String,
    until: Instant,
    round: Option<Round>,
    /// The person is being asked whether an agent's tab gets it: the
    /// notification asking, and the tab.
    asking: Option<(NoteId, u64, Waiting)>,
    /// Tabs it's been shown or put in (browser, tab): not again.
    given: HashSet<(u64, i64)>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.code.code.zeroize();
    }
}

/// What a notification of ours is about.
#[derive(Clone)]
enum About {
    /// A message in this conversation (its key), with the code it has.
    Message(String, Option<u64>),
    /// Whether an agent's tab gets code `id`.
    Agent(u64),
}

struct Service {
    client: Client,
    settings: PhoneSettings,
    state: PhoneState,
    published: Option<PhoneState>,
    threads: Vec<Thread>,
    /// The messages seen (their handles): one not among them is new.
    seen: HashSet<String>,
    first: bool,
    codes: Vec<Pending>,
    next_code: u64,
    browsers: HashMap<u64, Browser>,
    notes: Notifier,
    about: Vec<(NoteId, About)>,
    tx: mpsc::Sender<Ev>,
    /// Keeping the backend running: it's being started now; when to try
    /// next; how many tries have failed in a row.
    starting: bool,
    start_at: Option<Instant>,
    failed: u32,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// When a message says it was sent, in Unix seconds: with its zone, or
/// (the phone often leaves it out) as this computer's time.
pub fn when(at: &str) -> Option<i64> {
    use chrono::TimeZone as _;
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(at) {
        return Some(t.timestamp());
    }
    let naive = chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%dT%H:%M:%S").or_else(|_| chrono::NaiveDateTime::parse_from_str(at, "%Y%m%dT%H%M%S")).ok()?;
    chrono::Local.from_local_datetime(&naive).earliest().map(|t| t.timestamp())
}

/// Whether a message sent at `at` has just come (`now` in Unix seconds).
/// One that doesn't say when counts: it is new to us.
fn just_came(at: &str, now: i64) -> bool {
    when(at).is_none_or(|t| (now - t) <= JUST && (t - now) <= 60)
}

pub fn run() -> ! {
    let (tx, rx) = mpsc::channel::<Ev>();
    let client = match Client::connect() {
        Ok(c) => c,
        Err(e) => {
            // (No session bus: nothing to follow. Not an error to be
            // started again for.)
            tracing::warn!("phone: no session bus: {e}");
            std::process::exit(0);
        }
    };
    let to = tx.clone();
    if let Err(e) = Client::watch(move |e| {
        let _ = to.send(Ev::Bus(e));
    }) {
        tracing::warn!("phone: can't follow the backend: {e}");
    }
    let to = tx.clone();
    let _watch = olo_kit_base::watch::watch([PhoneSettings::path()], move |_| {
        let _ = to.send(Ev::Settings);
    });
    // The backend installed while the session runs (Olo Doctor's fix):
    // seen by its D-Bus activation file coming.
    let to = tx.clone();
    let _installed = olo_kit_base::watch::watch([std::path::PathBuf::from(blueferry::ACTIVATION), blueferry::configuration()], move |_| {
        let _ = to.send(Ev::Installed);
    });
    let to = tx.clone();
    let notes = Notifier::with_events("Phone", "olo-phone", move |e| {
        let _ = to.send(Ev::Note(e));
    });
    serve(tx.clone());
    let mut s = Service {
        client,
        settings: PhoneSettings::load(),
        state: PhoneState { pid: std::process::id(), ..Default::default() },
        published: None,
        threads: Vec::new(),
        seen: HashSet::new(),
        first: true,
        codes: Vec::new(),
        next_code: 0,
        browsers: HashMap::new(),
        notes,
        about: Vec::new(),
        tx,
        starting: false,
        start_at: None,
        failed: 0,
    };
    s.look();
    loop {
        // Asleep until something is said, or until a round of asking
        // ends or a code gets too old.
        let next = s.codes.iter().flat_map(|c| [Some(c.until), c.round.as_ref().map(|r| r.until)]).flatten().chain(s.start_at).min();
        let ev = match next {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(ev) => Some(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => std::process::exit(1),
            },
            None => match rx.recv() {
                Ok(ev) => Some(ev),
                Err(_) => std::process::exit(1),
            },
        };
        if let Some(ev) = ev {
            s.take(ev);
        }
        s.tick();
        s.publish();
    }
}

impl Service {
    fn take(&mut self, ev: Ev) {
        match ev {
            Ev::Bus(Event::Up) => {
                (self.failed, self.start_at) = (0, None);
                self.first = true;
                self.look();
            }
            // Gone: looked at, and started again in a moment if it
            // should be running (systemd restarts one that crashed
            // itself, 5 s on: after that).
            Ev::Bus(Event::Down) => {
                self.first = true;
                self.start_at = Some(Instant::now() + Duration::from_secs(8));
                self.look();
            }
            Ev::Started(why) => {
                self.starting = false;
                match why {
                    None => (self.failed, self.start_at) = (0, None),
                    Some(why) => {
                        self.failed += 1;
                        let wait = [5, 30, 120, 600][(self.failed as usize - 1).min(3)];
                        self.start_at = Some(Instant::now() + Duration::from_secs(wait));
                        tracing::info!("phone: the backend didn't start (try {}): {why}", self.failed);
                        self.state.trouble = why;
                    }
                }
                self.look();
            }
            Ev::Bus(Event::Status) => self.status(),
            Ev::Bus(Event::History) => self.history(),
            // (A popup of the backend's own was clicked: the message it
            // wants shown. Nothing shows conversations yet.)
            Ev::Bus(Event::Open(_)) => {}
            // Just installed, or just paired: looked at again, and
            // started if it should be running.
            Ev::Installed => {
                (self.failed, self.start_at) = (0, None);
                self.first = true;
                self.look();
            }
            Ev::Settings => {
                let settings = PhoneSettings::load();
                if settings != self.settings {
                    self.settings = settings;
                    self.policy();
                    if !self.settings.codes {
                        self.codes.clear();
                    }
                }
            }
            Ev::Ask(request, on_a_screen, answer) => {
                let reply = match request {
                    Request::Status => Reply { ok: true, state: Some(self.state.clone()), ..Default::default() },
                    // (An agent's program asks its tab's way, not this.)
                    Request::Code if on_a_screen => Reply { ok: false, error: Some("not from an agent's screen".into()), ..Default::default() },
                    Request::Code => match self.codes.last() {
                        Some(c) => Reply { ok: true, code: Some(c.code.code.clone()), from: Some(c.from.clone()), ..Default::default() },
                        None => Reply { ok: false, error: Some("no code has come in the last few minutes".into()), ..Default::default() },
                    },
                    Request::Browser { .. } => Reply::default(),
                };
                let _ = answer.send(reply);
            }
            Ev::Hello(id, screen, pid, out) => {
                tracing::info!("phone: a browser is listening (process {pid}{})", screen.as_ref().map(|s| format!(", on agent screen {s}")).unwrap_or_default());
                self.browsers.insert(id, Browser { screen, out });
            }
            Ev::Said(id, FromBrowser::Waiting { id: asked, tabs }) => self.waiting(id, asked, tabs),
            Ev::Said(id, FromBrowser::Outcome { id: code, outcome }) => self.outcome(id, code, &outcome),
            Ev::Left(id) => {
                self.browsers.remove(&id);
                for c in &mut self.codes {
                    if let Some(r) = &mut c.round {
                        r.waiting.remove(&id);
                    }
                }
            }
            Ev::Note(e) => self.note(e),
            Ev::Sent(id, Err(why)) => {
                self.notes.show(Some(id), Note::new("Not sent").body(why).kind(olo_kit_base::notify::NoteKind::Error));
            }
            Ev::Sent(_, Ok(())) => {}
        }
    }

    /// The backend as it stands now, from the start.
    fn look(&mut self) {
        if !self.client.running() {
            let installed = blueferry::installed();
            self.state = PhoneState {
                pid: std::process::id(),
                backend: if installed { Backend::Stopped } else { Backend::Missing },
                release: blueferry::installed_release().unwrap_or_default(),
                paired: installed && blueferry::configured(),
                trouble: std::mem::take(&mut self.state.trouble),
                codes: self.state.codes,
                ..Default::default()
            };
            self.threads.clear();
            // Installed, a phone paired, and not running: started.
            if self.state.paired && self.start_at.is_none() && self.failed == 0 {
                self.start();
            } else if !self.state.paired {
                self.state.trouble.clear();
            }
            return;
        }
        self.state.trouble.clear();
        self.status();
        if self.state.backend == Backend::Running {
            self.policy();
            self.history();
        }
    }

    fn status(&mut self) {
        let status = match self.client.status() {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("phone: the backend's status: {e}");
                return;
            }
        };
        let st = &mut self.state;
        st.release = status.release.clone();
        st.backend = if status.api == phone::API { Backend::Running } else { Backend::Other };
        st.paired = status.profile.is_some();
        st.link = if status.profile.is_none() { Link::NotPaired } else { Link::of(&status.connectivity) };
        (st.messages, st.contacts, st.notifications) = (status.map, status.pbap, status.ancs);
        st.battery = status.battery.filter(|_| st.link.connected());
        st.storage = status.storage_state.clone();
        if status.notification_policy != self.settings.notify.backend_policy() && st.backend == Backend::Running {
            self.policy();
        }
    }

    /// The backend's own popups as Olo's setting has them: Olo tells of
    /// messages itself, so they're for everything on the phone, or off.
    fn policy(&mut self) {
        if self.state.backend != Backend::Running {
            return;
        }
        let want = self.settings.notify.backend_policy();
        match self.client.status() {
            Ok(s) if s.notification_policy != want => {
                if let Err(e) = self.client.set_notification_policy(want) {
                    tracing::warn!("phone: the backend's popups: {e}");
                }
            }
            _ => {}
        }
    }

    /// The conversations changed: what's new in them.
    fn history(&mut self) {
        if self.state.backend != Backend::Running {
            return;
        }
        let threads = match self.client.threads(THREADS) {
            Ok(t) => t,
            Err(e) => {
                // (Locked or not ready: said in the state by the status.)
                tracing::debug!("phone: the conversations: {}", blueferry::error_name(&e).unwrap_or("error"));
                return;
            }
        };
        let now_s = now() as i64;
        let mut seen = HashSet::new();
        let mut new: Vec<(usize, usize)> = Vec::new();
        for (ti, t) in threads.iter().enumerate() {
            for (mi, m) in t.messages.iter().enumerate() {
                if !self.seen.contains(&m.handle) && !m.outgoing && !m.read && just_came(&m.at, now_s) && (!self.first || when(&m.at).is_some()) {
                    new.push((ti, mi));
                }
                seen.insert(m.handle.clone());
            }
        }
        self.seen = seen;
        self.state.conversations = threads.len() as u32;
        self.state.unread = threads.iter().map(|t| t.unread() as u32).sum();
        self.threads = threads;
        // (At the start, what's there was told of before: only a code
        // still good is taken up.)
        let first = std::mem::take(&mut self.first);
        for (ti, mi) in new {
            self.came(ti, mi, first);
        }
        self.notes.badge(self.state.unread);
    }

    /// A message has just come.
    fn came(&mut self, ti: usize, mi: usize, quiet: bool) {
        let thread = &self.threads[ti];
        let message = &thread.messages[mi];
        let code = if self.settings.codes && !thread.is_group { codes::find(&message.body) } else { None };
        let id = code.map(|code| {
            self.next_code += 1;
            let from = code.service.clone().unwrap_or_else(|| thread.name.clone());
            let age = Duration::from_secs(when(&message.at).map_or(0, |t| (now() as i64 - t).clamp(0, JUST) as u64));
            self.codes.push(Pending { id: self.next_code, code, from, until: Instant::now() + CODE_LIFE.saturating_sub(age), round: None, asking: None, given: HashSet::new() });
            self.state.codes += 1;
            self.state.code_at = Some(now());
            self.next_code
        });
        if !quiet && self.settings.notify == Notify::Messages {
            let who = if thread.is_group && !message.sender.is_empty() { Some(message.sender.clone()) } else { None };
            let mut note = Note::new(thread.name.clone()).style(NoteStyle::Message).icon("phone").action("read", "Mark as Read");
            note = note.body(if self.settings.show_text { message.body.clone() } else { "New message".into() });
            if let Some(who) = who {
                note = note.subtitle(who);
            }
            if thread.reply_ready && !thread.is_group {
                note = note.reply("Reply");
            }
            if id.is_some() {
                note = note.action("copy", "Copy Code");
            }
            let key = thread.key.clone();
            let note_id = self.notes.show(None, note);
            self.remember(note_id, About::Message(key, id));
        }
        if let Some(id) = id {
            self.ask_browsers(id);
        }
    }

    /// Ask every browser which of its tabs are waiting for a code.
    fn ask_browsers(&mut self, id: u64) {
        if !self.settings.codes_in_browser || self.browsers.is_empty() {
            self.no_tab(id);
            return;
        }
        let mut gone = Vec::new();
        let mut asked = HashSet::new();
        for (b, browser) in &self.browsers {
            if say(&browser.out, &ToBrowser::Any { id }) {
                asked.insert(*b);
            } else {
                gone.push(*b);
            }
        }
        for b in gone {
            self.browsers.remove(&b);
        }
        if let Some(c) = self.codes.iter_mut().find(|c| c.id == id) {
            c.round = Some(Round { waiting: asked, tabs: Vec::new(), until: Instant::now() + ROUND });
        }
    }

    /// A browser says which of its tabs are waiting: its answer to being
    /// asked about code `asked`, or by itself (a code field has come up:
    /// any code still good may be for it).
    fn waiting(&mut self, browser: u64, asked: Option<u64>, tabs: Vec<Waiting>) {
        match asked {
            Some(id) => {
                let Some(c) = self.codes.iter_mut().find(|c| c.id == id) else { return };
                let Some(r) = &mut c.round else { return };
                r.waiting.remove(&browser);
                r.tabs.extend(tabs.into_iter().map(|t| (browser, t)));
                if r.waiting.is_empty() {
                    self.decide(id);
                }
            }
            None => {
                if !self.settings.codes_in_browser {
                    return;
                }
                let tabs: Vec<(u64, Waiting)> = tabs.into_iter().map(|t| (browser, t)).collect();
                let ids: Vec<u64> = self.codes.iter().rev().filter(|c| c.round.is_none() && c.asking.is_none()).map(|c| c.id).collect();
                for id in ids {
                    if self.give(id, &tabs) {
                        break;
                    }
                }
            }
        }
    }

    /// The asking is over: the tab it's for, of those that answered.
    fn decide(&mut self, id: u64) {
        let Some(c) = self.codes.iter_mut().find(|c| c.id == id) else { return };
        let Some(round) = c.round.take() else { return };
        if !self.give(id, &round.tabs) {
            self.no_tab(id);
        }
    }

    /// Give code `id` to the best of `tabs`, if any is for it.
    fn give(&mut self, id: u64, tabs: &[(u64, Waiting)]) -> bool {
        let Some(c) = self.codes.iter().find(|c| c.id == id) else { return false };
        let tabs: Vec<(u64, Waiting)> = tabs.iter().filter(|(b, t)| self.browsers.contains_key(b) && !c.given.contains(&(*b, t.tab))).cloned().collect();
        let with_screens: Vec<(Option<String>, Waiting)> = tabs.iter().map(|(b, t)| (self.browsers[b].screen.clone(), t.clone())).collect();
        let Some((i, how)) = codes::choose(&c.code, &with_screens, self.settings.agent_screens) else { return false };
        let (browser, tab) = tabs[i].clone();
        match how {
            How::Offer => self.send_code(id, browser, &tab, false),
            How::Fill => self.send_code(id, browser, &tab, true),
            How::Ask => {
                let screen = with_screens[i].0.clone().unwrap_or_default();
                let from = c.from.clone();
                let note = Note::new(format!("Agent Screen “{screen}” is waiting for a code"))
                    .body(format!("At {}. Give it the code from {from}?", tab.host))
                    .icon("phone")
                    .kind(olo_kit_base::notify::NoteKind::Important)
                    .sticky(true)
                    .action("give", "Give Code")
                    .action("no", "Not Now");
                let note_id = self.notes.show(None, note);
                self.remember(note_id, About::Agent(id));
                if let Some(c) = self.codes.iter_mut().find(|c| c.id == id) {
                    c.asking = Some((note_id, browser, tab));
                }
            }
        }
        true
    }

    fn send_code(&mut self, id: u64, browser: u64, tab: &Waiting, fill: bool) {
        let Some(c) = self.codes.iter_mut().find(|c| c.id == id) else { return };
        let Some(b) = self.browsers.get(&browser) else { return };
        c.given.insert((browser, tab.tab));
        let mut message = ToBrowser::Code { id, tab: tab.tab, frame: tab.frame, code: c.code.code.clone(), from: c.from.clone(), fill };
        if !say(&b.out, &message) {
            self.browsers.remove(&browser);
        }
        if let ToBrowser::Code { code, .. } = &mut message {
            code.zeroize();
        }
        if fill && let Some(screen) = self.browsers.get(&browser).and_then(|b| b.screen.clone()) {
            // What the agent's tab got, in its screen's history (never
            // the code), and said to the person.
            record(&screen, &c.from, tab);
            self.notes.show(None, Note::new(format!("Code given to Agent Screen “{screen}”")).body(format!("The code from {}, at {}.", c.from, tab.host)).icon("phone"));
        }
    }

    /// No tab is waiting for code `id` (yet): copied, if that's wanted.
    /// It stays, for a field that comes up while it's still good.
    fn no_tab(&mut self, id: u64) {
        if !self.settings.copy_codes {
            return;
        }
        if let Some(c) = self.codes.iter().find(|c| c.id == id) {
            copy(c.code.code.clone());
        }
    }

    /// What came of a code given to a browser.
    fn outcome(&mut self, browser: u64, id: u64, outcome: &str) {
        if outcome != "filled" {
            return;
        }
        // Used: gone from everywhere else it shows, and from here.
        for (b, other) in &self.browsers {
            if *b != browser {
                say(&other.out, &ToBrowser::Gone { id });
            }
        }
        self.drop_code(id);
    }

    fn drop_code(&mut self, id: u64) {
        if let Some(c) = self.codes.iter().find(|c| c.id == id)
            && let Some((note, ..)) = &c.asking
        {
            self.notes.close(*note);
        }
        self.codes.retain(|c| c.id != id);
        if self.codes.is_empty() {
            self.state.code_at = None;
        }
    }

    /// Ask the bus for the backend, which starts it: on a thread (it
    /// waits on the start), its outcome back as `Ev::Started`.
    fn start(&mut self) {
        if self.starting {
            return;
        }
        (self.starting, self.start_at) = (true, None);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            // (The bus told of an activation file that has just come.)
            let _ = olo_system::env::reload_session_bus();
            let why = Client::connect().and_then(|c| c.status()).err().map(|e| blueferry::said(&e));
            let _ = tx.send(Ev::Started(why));
        });
    }

    /// Rounds that have waited long enough are decided; codes too old
    /// go; a backend that should be running is tried again when it's time.
    fn tick(&mut self) {
        let t = Instant::now();
        if self.start_at.is_some_and(|at| at <= t) {
            self.start_at = None;
            if blueferry::installed() && blueferry::configured() && !self.client.running() {
                self.start();
            }
        }
        let due: Vec<u64> = self.codes.iter().filter(|c| c.round.as_ref().is_some_and(|r| r.until <= t)).map(|c| c.id).collect();
        for id in due {
            self.decide(id);
        }
        let old: Vec<u64> = self.codes.iter().filter(|c| c.until <= t).map(|c| c.id).collect();
        for id in old {
            for b in self.browsers.values() {
                say(&b.out, &ToBrowser::Gone { id });
            }
            self.drop_code(id);
        }
    }

    /// What the person did with a notification of ours.
    fn note(&mut self, e: NoteEvent) {
        match e {
            NoteEvent::Action { id, key, .. } => match (self.about(id).cloned(), key.as_str()) {
                (Some(About::Message(thread, _)), "read") => {
                    if let Err(e) = self.client.mark_read(&thread) {
                        tracing::debug!("phone: marking read: {}", blueferry::error_name(&e).unwrap_or("error"));
                    }
                }
                (Some(About::Message(_, Some(code))), "copy") => {
                    if let Some(c) = self.codes.iter().find(|c| c.id == code) {
                        copy(c.code.code.clone());
                    }
                }
                (Some(About::Agent(code)), "give") => {
                    if let Some(c) = self.codes.iter_mut().find(|c| c.id == code)
                        && let Some((_, browser, tab)) = c.asking.take()
                    {
                        self.send_code(code, browser, &tab, true);
                    }
                }
                (Some(About::Agent(code)), _) => {
                    if let Some(c) = self.codes.iter_mut().find(|c| c.id == code) {
                        c.asking = None;
                    }
                }
                _ => {}
            },
            NoteEvent::Replied { id, text } => {
                let Some(About::Message(key, _)) = self.about(id) else { return };
                let Some(thread) = self.threads.iter().find(|t| &t.key == key).cloned() else { return };
                // (Sending waits on the phone: not here.)
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let sent = Client::connect().and_then(|c| c.reply(&thread, &text)).map_err(|e| blueferry::said(&e));
                    let _ = tx.send(Ev::Sent(id, sent));
                });
            }
            // (Gone from the screen; a message's stays in the
            // notification centre, where it can still be answered, so
            // the newest couple of hundred are remembered.)
            NoteEvent::Closed { id } => {
                if let Some(About::Agent(code)) = self.about(id) {
                    let code = *code;
                    if let Some(c) = self.codes.iter_mut().find(|c| c.id == code) {
                        c.asking = None;
                    }
                    self.about.retain(|(i, _)| *i != id);
                }
            }
        }
    }

    /// Remember what a notification is about (the newest couple of
    /// hundred: older ones can no longer be answered from).
    fn remember(&mut self, id: NoteId, about: About) {
        self.about.retain(|(i, _)| *i != id);
        self.about.push((id, about));
        if self.about.len() > 200 {
            self.about.remove(0);
        }
    }

    fn about(&self, id: NoteId) -> Option<&About> {
        self.about.iter().find(|(i, _)| *i == id).map(|(_, a)| a)
    }

    fn publish(&mut self) {
        if self.published.as_ref() != Some(&self.state) {
            if let Err(e) = self.state.save() {
                tracing::warn!("phone: can't write {}: {e}", PhoneState::path().display());
            }
            self.published = Some(self.state.clone());
        }
    }
}

/// A line to a browser's host; false if it has gone.
fn say(out: &UnixStream, message: &ToBrowser) -> bool {
    let Ok(mut line) = serde_json::to_vec(message) else { return false };
    line.push(b'\n');
    let sent = (&*out).write_all(&line).is_ok();
    line.zeroize();
    sent
}

/// A code on the clipboard, held until something else is copied.
fn copy(mut code: String) {
    std::thread::spawn(move || {
        if let Err(e) = olo_kit_base::clipboard::hold(olo_kit_base::clipboard::text(&code)) {
            tracing::warn!("phone: can't copy: {e}");
        }
        code.zeroize();
    });
}

/// A code went to a tab on agent screen `screen`: in that screen's
/// history (Agent Screens' `history::Action`, a JSON object a line), who
/// it was from and where it went. Never the code.
fn record(screen: &str, from: &str, tab: &Waiting) {
    if screen.is_empty() || screen.contains('/') {
        return;
    }
    let dir = olo_xdg::state_dir().join("screens").join(screen);
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let line = serde_json::json!({
        "at": at, "ms": 0, "kind": "code", "args": [tab.host],
        "result": {"from": from, "host": tab.host, "title": tab.title, "what": "a one-time code from the person's phone, put in the field waiting for it"},
    });
    if std::fs::create_dir_all(&dir).is_ok()
        && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("actions.jsonl"))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// The process at the other end of `stream`, and the agent's screen it
/// runs on, if it does (`OLO_SCREEN` in its environment; one that can't
/// be read counts as on one).
fn peer(stream: &UnixStream) -> (u32, Option<String>) {
    use std::os::fd::AsRawFd as _;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the kernel fills a ucred of the size given.
    let r = unsafe { libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len) };
    if r != 0 || cred.pid <= 0 {
        return (0, Some(String::new()));
    }
    let screen = match std::fs::read(format!("/proc/{}/environ", cred.pid)) {
        Ok(env) => env.split(|b| *b == 0).find_map(|kv| kv.strip_prefix(b"OLO_SCREEN=")).filter(|v| !v.is_empty()).map(|v| String::from_utf8_lossy(v).into_owned()),
        Err(_) => Some(String::new()),
    };
    (cred.pid as u32, screen)
}

/// Answer on the socket: a thread a connection. A browser's stays, a
/// line each way; the others ask once.
fn serve(tx: mpsc::Sender<Ev>) {
    let path = PhoneState::socket();
    // One service at a time: a socket someone answers on is theirs.
    if UnixStream::connect(&path).is_ok() {
        tracing::warn!("phone: another service is running");
        std::process::exit(0);
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("phone: can't listen on {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    std::thread::Builder::new()
        .name("phone-socket".into())
        .spawn(move || {
            let mut next = 0u64;
            for stream in listener.incoming().flatten() {
                next += 1;
                let (id, tx) = (next, tx.clone());
                std::thread::spawn(move || {
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
                    let (pid, screen) = peer(&stream);
                    let mut lines = BufReader::new(&stream).lines();
                    let Some(Ok(first)) = lines.next() else { return };
                    match serde_json::from_str::<Request>(&first) {
                        Ok(Request::Browser { pid: chrome, .. }) => {
                            let Ok(out) = stream.try_clone() else { return };
                            let _ = tx.send(Ev::Hello(id, screen, if chrome != 0 { chrome } else { pid }, out));
                            for line in lines.map_while(Result::ok) {
                                if let Ok(said) = serde_json::from_str::<FromBrowser>(&line) {
                                    let _ = tx.send(Ev::Said(id, said));
                                }
                            }
                            let _ = tx.send(Ev::Left(id));
                        }
                        Ok(request) => {
                            let (answer, got) = mpsc::channel();
                            let _ = tx.send(Ev::Ask(request, screen.is_some(), answer));
                            let reply = got.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| Reply { ok: false, error: Some("the service is busy".into()), ..Default::default() });
                            let mut out = serde_json::to_vec(&reply).unwrap_or_default();
                            out.push(b'\n');
                            let _ = (&stream).write_all(&out);
                            out.zeroize();
                        }
                        Err(e) => {
                            let reply = Reply { ok: false, error: Some(format!("not understood: {e}")), ..Default::default() };
                            let mut out = serde_json::to_vec(&reply).unwrap_or_default();
                            out.push(b'\n');
                            let _ = (&stream).write_all(&out);
                        }
                    }
                });
            }
        })
        .expect("thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_a_message_came() {
        assert_eq!(when("2026-08-12T10:00:00+00:00"), Some(1_786_528_800));
        assert_eq!(when("2026-08-12T12:00:00+02:00"), Some(1_786_528_800));
        // (Without a zone: this computer's, whatever it is.)
        let local = when("2026-08-12T10:00:00").unwrap();
        assert!((local - 1_786_528_800).abs() <= 14 * 3600);
        assert_eq!(when("yesterday"), None);
        let now = 1_786_528_800;
        assert!(just_came("2026-08-12T09:56:00+00:00", now));
        assert!(!just_came("2026-08-12T09:54:00+00:00", now), "older than five minutes");
        assert!(!just_came("2026-08-12T10:02:00+00:00", now), "from the future");
        assert!(just_came("", now), "one that doesn't say when is new to us");
    }
}

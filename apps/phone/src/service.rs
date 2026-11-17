//! `olo-phoned`: the session's phone service. It speaks to the
//! phone itself (`olo_system::phone`: Olo's profiles registered with
//! BlueZ, the Message Access and Phone Book Access profiles over the
//! channels BlueZ opens), keeps the conversations, publishes how the
//! phone stands ([`PhoneState`], none of it what a message says), tells
//! of each message as it comes in a notification of Olo's own (answered
//! in place), finds the one-time code in one (codes.rs) and gets it to
//! the browser tab that is waiting for it, and answers Phone the app and
//! the command line on its socket (the conversations, sending).
//!
//! **The link.** With a phone paired (anything paired that offers its
//! messages), the service asks BlueZ for a channel to its messages and
//! opens a session on it (a thread of its own, `session`: it waits on the
//! phone, the service's loop never does): refused, the phone's owner
//! hasn't allowed this computer yet (`Link::Allow`: tried again every
//! ten seconds, as allowing it says nothing); open, the phone is asked to
//! tell of messages as they come, which it does by connecting to Olo's
//! own Notification Server, and the newest of its inbox is read. Its
//! contacts are fetched once a session (names for numbers). A phone out
//! of reach is tried again when BlueZ says anything of it and otherwise
//! after 15 s, 30 s, 1, 2 and then every 5 minutes: nothing else runs
//! while it's away.
//!
//! **Codes.** Every browser's extension host stays connected to the
//! service's socket, is asked which of its tabs are waiting for a code
//! when one comes, and says so by itself when a code field comes up; the
//! service chooses among them all (`codes::choose`) and only the chosen
//! browser is given the code. A tab on an agent's screen gets it by the
//! person's setting, and what it got goes in that screen's history,
//! without the code. A code is kept in memory only, for [`CODE_LIFE`],
//! wiped when it's used or too old.
//!
//! Conversations are kept in memory (what the phone's inbox has when it
//! connects, what comes, what's sent from here): nothing a message says
//! is logged or written to disk.

use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::{Arc, mpsc},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::others::{Effect, Others};
use olo_kit_base::notify::{Note, NoteEvent, NoteId, Notifier};
use olo_protocol::{
    notifications::NoteStyle,
    phone::{Conversation, FromBrowser, Kind, Link, Message, Notify, Person, PhoneSettings, PhoneState, Reply, Request, ToBrowser, Waiting},
};
use olo_system::phone::{
    link::{self, Channel, Opened, Profiles, Stream},
    map::{self, BMessage, Listed, Mas},
    obex, pbap,
};
use zeroize::Zeroize;

use crate::codes::{self, Code, How};

/// How long a code can be used after it came.
pub const CODE_LIFE: Duration = Duration::from_secs(5 * 60);
/// How long the browsers have to say which tabs are waiting.
const ROUND: Duration = Duration::from_millis(700);
/// A message is one that has just come if it's no older than this (and
/// not from more than a minute ahead: the phone's clock and this one's).
const JUST: i64 = 5 * 60;
/// How much of the phone's inbox is read as a session opens.
const INBOX: u16 = 60;
/// How often a phone that hasn't allowed this computer is tried.
const ALLOW_EVERY: Duration = Duration::from_secs(10);
/// How long the phone's owner is waited for so (and, an iPhone,
/// advertised to: `link::Profiles::solicit`) from when the phone first
/// refused or Try Again was last pressed; after it, every five minutes
/// and with no advertisement, so a phone never allowed costs next to
/// nothing.
const ALLOW_FOR: Duration = Duration::from_secs(15 * 60);
/// How long BlueZ has to answer when asked for a channel (it waits on
/// the phone, half a minute at most), and how long the channel has to
/// come once it has said yes (it's opened by then). Past either, the
/// asking is given up and made again: nothing waits for good.
const OPEN_ANSWER: Duration = Duration::from_secs(60);
const OPEN_COMES: Duration = Duration::from_secs(5);
/// How long the service's own loop may take over one thing (it asks
/// BlueZ what phones there are, and BlueZ can stop answering) before
/// it's taken for stuck and ended, for the session to start afresh.
const STUCK: Duration = Duration::from_secs(90);
const ALLOW_LATER: Duration = Duration::from_secs(300);
/// How long after a try a phone out of reach is tried again, try by try.
const AWAY: [u64; 5] = [15, 30, 60, 120, 300];
/// How soon after the conversations or contacts change they're kept for
/// the next run (`kept`): once a burst of them has come, not at each.
const KEEP_AFTER: Duration = Duration::from_secs(2);

enum Ev {
    /// BlueZ opened a channel.
    Opened(Opened),
    /// BlueZ said something of its devices.
    Devices,
    /// BlueZ said the phone's battery or what it's playing changed
    /// (nothing about reaching it: no try is made for it).
    Detail,
    /// The phone's notification service (its link numbered so) said
    /// something.
    Others(u64, OthersSaid),
    /// The session (the one numbered so) says something.
    Session(u64, FromSession),
    Settings,
    Ask(Request, bool, mpsc::Sender<Reply>),
    Hello(u64, Option<String>, u32, UnixStream),
    Said(u64, FromBrowser),
    Left(u64),
    Note(NoteEvent),
}

/// What a session's thread is asked.
enum Cmd {
    /// Fetch the message with this handle (it has just come).
    Fetch(String),
    Send {
        to: String,
        text: String,
        note: Option<NoteId>,
        answer: Option<mpsc::Sender<Reply>>,
    },
    MarkRead(Vec<String>),
    Close,
}

/// What a session's thread says.
enum FromSession {
    /// Asking BlueZ for the channel failed (out of reach).
    NoChannel(String),
    /// The phone's owner hasn't allowed this computer.
    NotAllowed,
    /// The session is open: the newest of the inbox.
    Up(Vec<Listed>),
    /// A message that came: as its listing has it, and whole.
    Came(Listed, BMessage),
    Contacts(Vec<pbap::Contact>),
    Sent {
        to: String,
        text: String,
        done: Result<(), String>,
        note: Option<NoteId>,
        answer: Option<mpsc::Sender<Reply>>,
    },
    /// The session ended (the link went, or it failed): why.
    Down(String),
    /// BlueZ said yes to opening the channel: it's due.
    Asked,
    /// The phone closed the channel, or the link under it went.
    Hung,
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
    /// One of the phone's other notifications (its number there, in the
    /// link numbered so); an incoming call among them.
    Other(u64, u32),
    Call(u64, u32),
}

/// The phone's other notifications, while their link is open.
struct OthersLink {
    /// Which opening this is (an earlier one's packets are old news).
    generation: u64,
    state: Others,
    /// To the thread that writes its commands to the phone.
    write: mpsc::Sender<Vec<u8>>,
    /// The notifications shown, by the phone's numbers.
    notes: HashMap<u32, NoteId>,
}

/// What the notification service's threads say.
enum OthersSaid {
    Told(Vec<u8>),
    Data(Vec<u8>),
    /// A stream ended: the link went.
    Gone,
}

struct Service {
    /// Olo's profiles with BlueZ (None: not registered, `state.trouble`
    /// says why).
    profiles: Option<Arc<Profiles>>,
    profiles_at: Option<Instant>,
    /// Olo's advertisement to an iPhone whose switches are waited for,
    /// while it's broadcast; whether it couldn't be (said once); and
    /// since when its owner has been waited for.
    solicit: Option<link::Solicit>,
    solicit_failed: bool,
    allow_since: Option<Instant>,
    /// Why the last channel couldn't be had (logged when it changes).
    said_why: String,
    /// While a channel is being opened: when to give up on it
    /// (`OPEN_ANSWER`, `OPEN_COMES`).
    opening_until: Option<Instant>,
    /// BlueZ said yes to the last asking and opened nothing: it's told
    /// to let go of the profile before it's asked again.
    let_go: bool,
    /// What the conversations and contacts came to when last published
    /// (`PhoneState::changes` counts its changes), and when to keep them
    /// for the next run (`kept`).
    digest: u64,
    keep_at: Option<Instant>,
    /// The phone in use, and its music's player with BlueZ while it
    /// has one.
    phone: Option<link::Phone>,
    music_player: Option<String>,
    /// Its other notifications, while their link is open; how many
    /// times it has opened; and since when it's been waited for (the
    /// phone connected: advertised to and asked for a while).
    others: Option<OthersLink>,
    others_opened: u64,
    others_since: Option<Instant>,
    /// The session open or being opened: its number, and its thread's ear
    /// once the channel has come.
    generation: u64,
    opening: bool,
    session: Option<mpsc::Sender<Cmd>>,
    /// When to try the phone again, and how many tries have failed.
    retry_at: Option<Instant>,
    tries: usize,
    /// When to ask for its contacts again (they come only once the
    /// phone's owner allows them, which says nothing), and how often
    /// they've been asked for this session.
    contacts_at: Option<Instant>,
    contacts_tries: u32,
    settings: PhoneSettings,
    state: PhoneState,
    published: Option<PhoneState>,
    contacts: Vec<pbap::Contact>,
    threads: Vec<Conversation>,
    /// The messages seen (their handles): one not among them is new.
    seen: HashSet<String>,
    codes: Vec<Pending>,
    next_code: u64,
    browsers: HashMap<u64, Browser>,
    notes: Notifier,
    about: Vec<(NoteId, About)>,
    tx: mpsc::Sender<Ev>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// When a message says it was sent, in Unix seconds: a listing's
/// `YYYYMMDDTHHMMSS`, with `Z` or an offset (`+0100`) when the phone
/// knows the zone, and otherwise (an iPhone often) as this computer's
/// time.
pub fn when(at: &str) -> Option<i64> {
    use chrono::TimeZone as _;
    let at = at.trim();
    let (stamp, zone) = at.split_at(at.len().min(15));
    let naive = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S").ok()?;
    match zone {
        "" => chrono::Local.from_local_datetime(&naive).earliest().map(|t| t.timestamp()),
        "Z" | "z" => Some(naive.and_utc().timestamp()),
        z => {
            let sign = match z.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let digits: String = z[1..].chars().filter(char::is_ascii_digit).collect();
            let (h, m): (i64, i64) = (digits.get(..2)?.parse().ok()?, digits.get(2..4).unwrap_or("00").parse().ok()?);
            Some(naive.and_utc().timestamp() - sign * (h * 3600 + m * 60))
        }
    }
}

/// Whether a message sent at `at` has just come (`now` in Unix seconds).
/// One that doesn't say when counts: it is new to us.
fn just_came(at: &str, now: i64) -> bool {
    when(at).is_none_or(|t| (now - t) <= JUST && (t - now) <= 60)
}

/// One session with the phone's messages, on a thread of its own: from
/// asking BlueZ for the channel to the link's end. It says what happens
/// (`FromSession`) and does what it's asked (`Cmd`).
fn session(stream: Stream, cmds: mpsc::Receiver<Cmd>, said: impl Fn(FromSession)) {
    // However it ends, the channel is shut (not only closed here: what
    // waits for its end, `Stream::gone`, holds it too).
    struct Shut(link::Closer);
    impl Drop for Shut {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _shut = Shut(stream.closer());
    let mut mas = match Mas::open(stream) {
        Ok(m) => m,
        Err(obex::Error::Refused(obex::code::FORBIDDEN | obex::code::UNAUTHORIZED)) => return said(FromSession::NotAllowed),
        Err(e) => return said(FromSession::Down(e.to_string())),
    };
    // Told of messages as they come (the phone connects back), and what
    // the inbox has now.
    let opened = mas.notify(true).and_then(|()| mas.listing("inbox", INBOX));
    match opened {
        Ok(listed) => said(FromSession::Up(listed)),
        Err(e) => return said(FromSession::Down(e.to_string())),
    }
    for cmd in cmds {
        let done = match cmd {
            Cmd::Fetch(handle) => (|| {
                // (Its listing's row has when it came; the message
                // whole has all of its text, which an Android's row
                // doesn't.)
                let row = mas.listing("inbox", 12)?.into_iter().find(|m| m.handle == handle).unwrap_or(Listed { handle: handle.clone(), ..Default::default() });
                let whole = mas.message(&handle)?;
                said(FromSession::Came(row, whole));
                Ok(())
            })(),
            Cmd::Send { to, text, note, answer } => {
                let done = match BMessage::outgoing(&[to.as_str()], &text) {
                    None => Err("That isn't a telephone number or an address.".to_owned()),
                    Some(b) => mas.send(&b).map(|_| ()).map_err(|e| e.to_string()),
                };
                let broke = done.is_err() && !mas_alive(&mut mas);
                said(FromSession::Sent { to, text, done, note, answer });
                if broke { Err(obex::Error::Malformed("the link went")) } else { Ok(()) }
            }
            Cmd::MarkRead(handles) => handles.iter().try_for_each(|h| mas.mark_read(h).or_else(|e| if matches!(e, obex::Error::Refused(_)) { Ok(()) } else { Err(e) })),
            Cmd::Close => break,
        };
        if let Err(e) = done {
            return said(FromSession::Down(e.to_string()));
        }
    }
    mas.close();
    said(FromSession::Down(String::new()))
}

/// Whether the session still answers (after something failed: a refusal
/// leaves it up, a link gone doesn't).
fn mas_alive(mas: &mut Mas<Stream>) -> bool {
    mas.listing("inbox", 1).is_ok()
}

/// The service's own loop, watched: it's told when the loop takes
/// something up (true) and when it's done with it (false), and if the
/// second doesn't come within `STUCK` the service is stuck (waiting on a
/// BlueZ that has stopped answering, most likely) and is ended; the
/// session starts it again, and it starts from what the phone and BlueZ
/// say then. Asleep whenever the loop is: it costs two messages a turn
/// of the loop and nothing at rest.
fn watchdog() -> mpsc::Sender<bool> {
    watch(STUCK, || {
        tracing::error!("phone: stuck for {} s (Bluetooth not answering?): ending, to be started afresh", STUCK.as_secs());
        std::process::exit(75);
    })
}

/// [`watchdog`]'s thread: `stuck` is called if what was taken up (true)
/// isn't done with (false) within `limit`.
fn watch(limit: Duration, stuck: impl Fn() + Send + 'static) -> mpsc::Sender<bool> {
    let (tx, rx) = mpsc::channel::<bool>();
    let _ = std::thread::Builder::new().name("phone-watch".into()).spawn(move || {
        loop {
            match rx.recv() {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => return,
            }
            loop {
                match rx.recv_timeout(limit) {
                    Ok(false) => break,
                    Ok(true) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        stuck();
                        break;
                    }
                }
            }
        }
    });
    tx
}

pub fn run() -> ! {
    let (tx, rx) = mpsc::channel::<Ev>();
    let to = tx.clone();
    let _watch = olo_kit_base::watch::watch([PhoneSettings::path()], move |_| {
        let _ = to.send(Ev::Settings);
    });
    let to = tx.clone();
    let notes = Notifier::with_events("Phone", "olo-phone", move |e| {
        let _ = to.send(Ev::Note(e));
    });
    serve(tx.clone());
    let mut s = Service {
        profiles: None,
        profiles_at: None,
        solicit: None,
        solicit_failed: false,
        allow_since: None,
        said_why: String::new(),
        opening_until: None,
        let_go: false,
        digest: 0,
        phone: None,
        music_player: None,
        others: None,
        others_opened: 0,
        others_since: None,
        keep_at: None,
        generation: 0,
        opening: false,
        session: None,
        retry_at: None,
        tries: 0,
        contacts_at: None,
        contacts_tries: 0,
        settings: PhoneSettings::load(),
        state: PhoneState { pid: std::process::id(), ..Default::default() },
        published: None,
        contacts: Vec::new(),
        threads: Vec::new(),
        seen: HashSet::new(),
        codes: Vec::new(),
        next_code: 0,
        browsers: HashMap::new(),
        notes,
        about: Vec::new(),
        tx,
    };
    let beat = watchdog();
    let _ = beat.send(true);
    s.register();
    s.devices();
    s.publish();
    let _ = beat.send(false);
    loop {
        // Asleep until something is said, or until a round of asking
        // ends, a code gets too old, or it's time to try the phone again.
        let next = s
            .codes
            .iter()
            .flat_map(|c| [Some(c.until), c.round.as_ref().map(|r| r.until)])
            .flatten()
            .chain(s.retry_at)
            .chain(s.profiles_at)
            .chain(s.contacts_at)
            .chain(s.opening_until)
            .chain(s.keep_at)
            .chain(s.others.as_ref().and_then(|o| o.state.due()))
            // (When advertising for its other notifications ends.)
            .chain(s.others_since.filter(|_| s.others.is_none() && s.solicit.is_some()).map(|since| since + ALLOW_FOR).filter(|at| *at > Instant::now()))
            .min();
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
        // (Watched while it works: see `watchdog`.)
        let _ = beat.send(true);
        if let Some(ev) = ev {
            s.take(ev);
        }
        s.tick();
        s.publish();
        let _ = beat.send(false);
    }
}

impl Service {
    fn take(&mut self, ev: Ev) {
        match ev {
            Ev::Opened(opened) => self.opened(opened),
            Ev::Devices => {
                // (Something changed: a phone back in reach is tried now.)
                self.tries = 0;
                self.devices();
            }
            Ev::Detail => {
                if let (Some(profiles), Some(phone)) = (self.profiles.clone(), self.phone.as_mut())
                    && let Ok(phones) = profiles.phones()
                    && let Some(now) = phones.iter().find(|p| p.path == phone.path)
                {
                    phone.battery = now.battery;
                }
                self.details();
            }
            Ev::Others(generation, said) => {
                if self.others.as_ref().is_some_and(|o| o.generation == generation) {
                    self.others_said(said);
                }
            }
            Ev::Session(generation, said) if generation == self.generation => self.from_session(said),
            // (An earlier session's: old news; but an answer owed is given.)
            Ev::Session(_, FromSession::Sent { answer: Some(answer), .. }) => {
                let _ = answer.send(Reply { ok: false, error: Some("The phone went.".into()), ..Default::default() });
            }
            Ev::Session(..) => {}
            Ev::Settings => {
                let settings = PhoneSettings::load();
                if settings != self.settings {
                    self.settings = settings;
                    if !self.settings.codes {
                        self.codes.clear();
                    }
                    // (Its other notifications, turned on or off.)
                    self.others_link();
                }
            }
            Ev::Ask(request, on_a_screen, answer) => self.ask(request, on_a_screen, answer),
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
        }
    }

    /// Olo's profiles, registered with the BlueZ running now: registered
    /// at first, and again with a BlueZ started since (it forgets them
    /// and says nothing: an update, Olo Doctor changing how it's started,
    /// a crash). Looked at whenever BlueZ says anything, before every
    /// try, and again in a minute where it can't be done (no BlueZ, or
    /// another program has them). `PhoneState::profiles` says how it
    /// stands, for Olo Doctor.
    fn register(&mut self) {
        self.profiles_at = None;
        let done = match &self.profiles {
            Some(p) => p.ensure(),
            None => {
                let to = self.tx.clone();
                Profiles::register(move |opened| {
                    let _ = to.send(Ev::Opened(opened));
                })
                .and_then(|p| {
                    let to = self.tx.clone();
                    p.watch(move |change| {
                        let _ = to.send(match change {
                            link::Change::Link => Ev::Devices,
                            link::Change::Detail => Ev::Detail,
                        });
                    })?;
                    self.profiles = Some(Arc::new(p));
                    Ok(true)
                })
            }
        };
        match done {
            Ok(again) => {
                if again || !self.state.profiles {
                    tracing::info!("phone: Olo's profiles (messages, contacts, its notification server) are registered with BlueZ");
                    // (Every channel there was went with the BlueZ that
                    // had them.)
                    self.end_session();
                    (self.solicit, self.solicit_failed) = (None, false);
                    (self.tries, self.retry_at) = (0, None);
                    self.state.trouble.clear();
                    if self.state.link == Link::Taken {
                        self.state.link = Link::Away;
                    }
                }
                self.state.profiles = true;
            }
            Err(e) => {
                let taken = e.to_string().contains("AlreadyExists");
                let trouble = if taken {
                    "Another program has the phone’s message profiles (BlueFerry’s or GNOME’s obexd). Olo Doctor removes BlueFerry.".to_owned()
                } else {
                    "Bluetooth isn’t answering.".to_owned()
                };
                if self.state.profiles || self.state.trouble != trouble {
                    tracing::info!("phone: can't register with BlueZ: {e}");
                }
                self.end_session();
                self.solicit = None;
                self.state.profiles = false;
                self.state.trouble = trouble;
                self.state.link = if taken {
                    Link::Taken
                } else if self.phone.is_some() {
                    Link::Away
                } else {
                    self.state.link
                };
                self.profiles_at = Some(Instant::now() + Duration::from_secs(60));
            }
        }
    }

    /// The phones there are now: the one in use, and a session opened to
    /// it if there's none.
    fn devices(&mut self) {
        self.register();
        // (With no BlueZ to ask, what's known stands: it's asked again
        // when BlueZ is back.)
        let Some(profiles) = self.profiles.clone().filter(|_| self.state.profiles) else { return };
        let Ok(phones) = profiles.phones() else { return };
        // (The one in use while it's still paired; else one in reach;
        // else the first.)
        let phone = self.phone.as_ref().and_then(|had| phones.iter().find(|p| p.path == had.path)).or_else(|| phones.iter().find(|p| p.connected)).or(phones.first()).cloned();
        if phone.as_ref().map(|p| (&p.path, p.kind, p.connected)) != self.phone.as_ref().map(|p| (&p.path, p.kind, p.connected)) {
            match &phone {
                Some(p) => {
                    tracing::info!("phone: {} ({}), {}", p.name, if p.kind == link::Kind::IPhone { "an iPhone" } else { "not an iPhone" }, if p.connected { "connected" } else { "not connected" })
                }
                None => tracing::info!("phone: none paired that offers its messages"),
            }
            // (Its owner is waited for afresh each time it comes.)
            self.allow_since = None;
        }
        if phone.as_ref().map(|p| &p.path) != self.phone.as_ref().map(|p| &p.path) {
            // Another phone, or none: what the last one had is let go.
            self.end_session();
            (self.threads, self.contacts, self.seen) = (Vec::new(), Vec::new(), HashSet::new());
            self.codes.clear();
            (self.state.conversations, self.state.unread, self.state.contacts, self.state.code_at) = (0, 0, false, None);
            self.keep_at = None;
            match &phone {
                // What was kept of it when the service last ran: there
                // at once, before the phone has been asked anything.
                Some(p) => {
                    if let Some(kept) = crate::kept::load(&p.address) {
                        tracing::info!("phone: {} conversations and {} contacts kept from before", kept.threads.len(), kept.contacts.len());
                        self.threads = kept.threads;
                        self.seen = kept.seen.into_iter().collect();
                        self.contacts = kept.contacts.into_iter().map(|c| pbap::Contact { name: c.name, phones: c.phones, emails: c.emails }).collect();
                        self.count();
                    }
                }
                // Unpaired (its device gone while the adapter is
                // there): nothing of it stays.
                None if self.phone.is_some() && profiles.has_adapter() => crate::kept::forget(),
                None => {}
            }
        }
        self.phone = phone;
        self.details();
        self.others_link();
        let st = &mut self.state;
        match &self.phone {
            None => {
                (st.name, st.path, st.battery, st.link) = (String::new(), String::new(), None, Link::NotPaired);
                self.retry_at = None;
            }
            Some(p) => {
                st.name = p.name.clone();
                st.path = p.path.clone();
                st.kind = if p.kind == link::Kind::IPhone { Kind::IPhone } else { Kind::Android };
                st.battery = p.battery.filter(|_| p.connected);
                // Out of reach with a session open: the session has
                // ended, whether or not its channel has said so yet.
                let gone = !p.connected && (self.session.is_some() || self.opening);
                if gone {
                    if let Some(cmds) = self.session.take() {
                        let _ = cmds.send(Cmd::Close);
                    }
                    (self.opening, self.opening_until) = (false, None);
                    self.generation += 1;
                    st.link = Link::Away;
                }
                if self.session.is_none() && !self.opening {
                    if st.link == Link::NotPaired || st.link == Link::Ready {
                        st.link = Link::Away;
                    }
                    if self.retry_at.is_none() {
                        self.open();
                    }
                }
            }
        }
    }

    /// The phone's other notifications (an iPhone's, `others`): their
    /// link opened when the phone shows the service (connected over low
    /// energy, its owner having allowed it), closed when the phone goes
    /// or they're turned off. While the phone is connected without it,
    /// it's asked to connect that way too, once, and advertised to for
    /// a while (`solicit`).
    fn others_link(&mut self) {
        let wanted = self.settings.others;
        let phone = self.phone.clone().filter(|p| p.connected && p.kind == link::Kind::IPhone && wanted);
        let Some((phone, profiles)) = phone.zip(self.profiles.clone()) else {
            self.others_end();
            self.others_since = None;
            return;
        };
        if self.state.model.is_empty()
            && let Some(model) = profiles.model(&phone.path)
        {
            tracing::info!("phone: it says it is {model}");
            self.state.model = model;
        }
        if self.others.is_some() {
            return;
        }
        let mut link = match profiles.notifications(&phone.path) {
            Ok(Some(link)) => link,
            Ok(None) => {
                // Not there (yet): asked for once each time the phone
                // comes.
                if self.others_since.is_none() {
                    self.others_since = Some(Instant::now());
                    profiles.connect_low_energy(&phone.path);
                }
                return;
            }
            Err(e) => {
                tracing::info!("phone: its other notifications can't be opened: {e}");
                return;
            }
        };
        let Some((source, data)) = link.streams() else { return };
        self.others_opened += 1;
        let generation = self.others_opened;
        // A thread each: the two streams (a packet a read, until the
        // link goes), and the commands (a write waits for the phone).
        for (name, mut stream, data) in [("phone-told", source, false), ("phone-answers", data, true)] {
            let to = self.tx.clone();
            let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
                use std::io::Read as _;
                let mut packet = [0u8; 1024];
                loop {
                    match stream.read(&mut packet) {
                        Ok(n) if n > 0 => {
                            let said = if data { OthersSaid::Data(packet[..n].to_vec()) } else { OthersSaid::Told(packet[..n].to_vec()) };
                            if to.send(Ev::Others(generation, said)).is_err() {
                                return;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        _ => break,
                    }
                }
                let _ = to.send(Ev::Others(generation, OthersSaid::Gone));
            });
        }
        let (write, commands) = mpsc::channel::<Vec<u8>>();
        let writer = link.writer();
        let _ = std::thread::Builder::new().name("phone-asks".into()).spawn(move || {
            for command in commands {
                if let Err(e) = writer(&command) {
                    tracing::info!("phone: it didn't take a question of its notifications: {e}");
                }
            }
        });
        tracing::info!("phone: its other notifications are open");
        let mut state = Others::default();
        state.messages_elsewhere = self.session.is_some();
        self.others = Some(OthersLink { generation, state, write, notes: HashMap::new() });
        self.state.others = true;
    }

    /// The link to the phone's other notifications is over: a call
    /// still shown as ringing goes (nothing more will be heard of it);
    /// the rest stay, as read.
    fn others_end(&mut self) {
        let Some(link) = self.others.take() else { return };
        tracing::info!("phone: its other notifications are closed");
        self.state.others = false;
        let calls: Vec<NoteId> =
            self.about.iter().filter(|(id, about)| matches!(about, About::Call(g, _) if *g == link.generation) && link.notes.values().any(|n| n == id)).map(|(id, _)| *id).collect();
        for id in calls {
            self.notes.close(id);
        }
    }

    fn others_said(&mut self, said: OthersSaid) {
        let now = Instant::now();
        let Some(o) = &mut self.others else { return };
        // (Text messages come whole another way while their session is
        // open: theirs aren't shown twice.)
        o.state.messages_elsewhere = self.session.is_some();
        let effects = match said {
            OthersSaid::Told(packet) => {
                tracing::debug!("phone: told of a notification ({} bytes)", packet.len());
                o.state.told(&packet, now)
            }
            OthersSaid::Data(packet) => o.state.data(&packet, now),
            OthersSaid::Gone => {
                self.others_end();
                // (It may come again: asked for anew.)
                self.others_since = None;
                self.others_link();
                return;
            }
        };
        self.others_do(effects);
    }

    fn others_do(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            let Some(o) = &mut self.others else { return };
            match effect {
                Effect::Write(command) => {
                    let _ = o.write.send(command);
                }
                Effect::Show(shown) => {
                    let mut note = Note::new(shown.title.clone()).subtitle(shown.app.clone()).body(shown.body.clone()).icon("phone");
                    if shown.call {
                        note = note.sticky(true);
                    }
                    if let Some(label) = &shown.positive {
                        note = note.action("positive", label.clone());
                    }
                    if let Some(label) = &shown.negative {
                        note = note.action("negative", label.clone());
                    }
                    // (Which app, never what it says.)
                    tracing::info!("phone: a notification of its {} shown{}", shown.app, if shown.call { " (a call)" } else { "" });
                    let (generation, had) = (o.generation, o.notes.get(&shown.uid).copied());
                    let id = self.notes.show(had, note);
                    if let Some(o) = &mut self.others {
                        o.notes.insert(shown.uid, id);
                    }
                    if had.is_none() {
                        self.remember(id, if shown.call { About::Call(generation, shown.uid) } else { About::Other(generation, shown.uid) });
                    }
                }
                Effect::Close(uid) => {
                    if let Some(id) = o.notes.remove(&uid) {
                        self.notes.close(id);
                    }
                }
            }
        }
    }

    /// Keep the conversations, the messages seen and the contacts for
    /// the next run (`kept`).
    fn keep_for_next_run(&mut self) {
        self.keep_at = None;
        let Some(phone) = &self.phone else { return };
        let mut seen: Vec<String> = self.seen.iter().cloned().collect();
        seen.sort();
        let kept = crate::kept::Kept {
            phone: phone.address.clone(),
            threads: self.threads.clone(),
            seen,
            contacts: self.contacts.iter().map(|c| Person { name: c.name.clone(), phones: c.phones.clone(), emails: c.emails.clone() }).collect(),
        };
        if let Err(e) = crate::kept::save(&kept) {
            tracing::warn!("phone: can't keep its conversations in {}: {e}", crate::kept::path().display());
        }
    }

    /// The phone's battery and what it's playing (BlueZ's own remote
    /// control of its player), while it's connected.
    fn details(&mut self) {
        let phone = self.phone.as_ref().filter(|p| p.connected);
        let music = phone.zip(self.profiles.as_ref()).and_then(|(p, profiles)| profiles.music(&p.path));
        self.state.battery = phone.and_then(|p| p.battery);
        self.music_player = music.as_ref().map(|m| m.player.clone());
        self.state.music = music.map(|m| olo_protocol::phone::Music { playing: m.playing, title: m.title, artist: m.artist, album: m.album });
    }

    /// Ask BlueZ for a channel to the phone's messages (on a thread: it
    /// waits while the phone is looked for, half a minute with a real
    /// one); the channel comes as `Ev::Opened`, a failure as the
    /// session's `NoChannel`.
    fn open(&mut self) {
        self.register();
        let (Some(profiles), Some(phone)) = (self.profiles.clone().filter(|_| self.state.profiles), &self.phone) else { return };
        self.generation += 1;
        (self.opening, self.retry_at) = (true, None);
        self.opening_until = Some(Instant::now() + OPEN_ANSWER);
        if self.state.link != Link::Allow {
            self.state.link = if phone.connected { Link::Connecting } else { Link::Away };
        }
        let (tx, generation, path) = (self.tx.clone(), self.generation, phone.path.clone());
        let let_go = std::mem::take(&mut self.let_go);
        let _ = std::thread::Builder::new().name("phone-open".into()).spawn(move || {
            if let_go {
                profiles.let_go(&path, Channel::Messages);
            }
            let said = match profiles.open(&path, Channel::Messages) {
                Ok(()) => FromSession::Asked,
                Err(e) => FromSession::NoChannel(e.to_string()),
            };
            let _ = tx.send(Ev::Session(generation, said));
        });
    }

    /// A channel BlueZ opened.
    fn opened(&mut self, opened: Opened) {
        if self.phone.as_ref().map(|p| p.path.as_str()) != Some(opened.phone.as_str()) {
            return;
        }
        let (tx, generation) = (self.tx.clone(), self.generation);
        match opened.channel {
            Channel::Messages => {
                // (One session: another there, unasked for, is ended.)
                if let Some(old) = self.session.take() {
                    let _ = old.send(Cmd::Close);
                }
                tracing::info!("phone: a channel to its messages is open ({})", if opened.stream.packets() { "packets: L2CAP" } else { "a stream: RFCOMM" });
                let (cmds, heard) = mpsc::channel();
                self.session = Some(cmds);
                (self.opening, self.opening_until, self.let_go) = (false, None, false);
                // Its end is waited for (asleep): a phone that closes the
                // channel, or a link that goes under it, is known of at
                // once, not when next something is asked of it.
                let (gone, hung) = (opened.stream.gone(), tx.clone());
                let _ = std::thread::Builder::new().name("phone-link".into()).spawn(move || {
                    gone.wait();
                    let _ = hung.send(Ev::Session(generation, FromSession::Hung));
                });
                let _ = std::thread::Builder::new().name("phone-session".into()).spawn(move || {
                    session(opened.stream, heard, |said| {
                        let _ = tx.send(Ev::Session(generation, said));
                    })
                });
            }
            // The phone telling of messages: each fetched by the session.
            Channel::Notifications => {
                let Some(cmds) = self.session.clone() else { return };
                opened.stream.patience(Duration::ZERO);
                let _ = std::thread::Builder::new().name("phone-events".into()).spawn(move || {
                    let _ = map::serve_notifications(opened.stream, |event| {
                        if let map::Event::New { handle, .. } = event {
                            let _ = cmds.send(Cmd::Fetch(handle));
                        }
                    });
                });
            }
            Channel::Contacts => {
                let _ = std::thread::Builder::new().name("phone-contacts".into()).spawn(move || {
                    if let Ok(contacts) = pbap::contacts(opened.stream) {
                        let _ = tx.send(Ev::Session(generation, FromSession::Contacts(contacts)));
                    }
                });
            }
        }
    }

    /// Olo's advertisement to an iPhone, broadcast while its switches for
    /// this computer are waited for (its messages refused; or its
    /// contacts, for the first minutes after its messages opened) and
    /// for no longer than `ALLOW_FOR`: an iPhone offers the switches only
    /// while it's asked so (`link::Profiles::solicit`).
    fn solicit(&mut self) {
        let waited = self.allow_since.is_some_and(|since| since.elapsed() < ALLOW_FOR);
        let wanting = match self.state.link {
            Link::Allow => true,
            Link::Ready => !self.state.contacts,
            _ => false,
        };
        // (And for its other notifications, which come over low energy:
        // an iPhone connects that way to a computer advertising for
        // them. For a while after the phone came, while they aren't
        // open.)
        let others = self.settings.others && self.others.is_none() && self.others_since.is_some_and(|since| since.elapsed() < ALLOW_FOR);
        let phone = self.phone.as_ref().filter(|p| p.kind == link::Kind::IPhone && p.connected && self.state.profiles && ((wanting && waited) || others));
        let Some(phone) = phone else {
            if self.solicit.take().is_some() {
                tracing::info!("phone: no longer advertising for the iPhone's switches");
            }
            return;
        };
        if self.solicit.is_some() || self.solicit_failed {
            return;
        }
        let Some(profiles) = &self.profiles else { return };
        match profiles.solicit(&phone.path) {
            Ok(s) => {
                tracing::info!("phone: advertising to the iPhone for its switches (Show Notifications, Sync Contacts)");
                self.solicit = Some(s);
            }
            Err(e) => {
                tracing::info!("phone: can't advertise for the iPhone's switches: {e}");
                self.solicit_failed = true;
            }
        }
    }

    fn end_session(&mut self) {
        if let Some(cmds) = self.session.take() {
            let _ = cmds.send(Cmd::Close);
        }
        (self.opening, self.opening_until) = (false, None);
        self.generation += 1;
    }

    /// What the session says.
    fn from_session(&mut self, said: FromSession) {
        match said {
            FromSession::Asked => {
                if self.opening {
                    self.opening_until = Some(Instant::now() + OPEN_COMES);
                }
            }
            FromSession::Hung => {
                // (Its own end, after it ended here, says nothing new.)
                let Some(cmds) = self.session.take() else { return };
                tracing::info!("phone: it closed the channel to its messages (or the link went): asking again");
                let _ = cmds.send(Cmd::Close);
                self.generation += 1;
                (self.opening, self.opening_until) = (false, None);
                self.state.link = Link::Away;
                (self.tries, self.retry_at) = (0, Some(Instant::now() + Duration::from_secs(1)));
            }
            FromSession::NoChannel(why) => {
                (self.opening, self.session, self.opening_until) = (false, None, None);
                self.state.link = Link::Away;
                // (BlueZ busy with the phone, connecting or pairing:
                // soon; otherwise further and further apart.)
                let wait = if why.contains("InProgress") || why.contains("no channel came") { 3 } else { AWAY[self.tries.min(AWAY.len() - 1)] };
                self.tries += 1;
                self.retry_at = Some(Instant::now() + Duration::from_secs(wait));
                // Why, said plainly when the phone is there and still
                // can't be spoken to (out of reach needs no more words).
                let connected = self.phone.as_ref().is_some_and(|p| p.connected);
                if self.said_why != why {
                    tracing::info!("phone: no channel to its messages, the phone {} ({why})", if connected { "connected" } else { "not connected" });
                    self.said_why = why.clone();
                }
                self.state.trouble =
                    if connected { format!("Bluetooth couldn’t open the phone’s messages ({}).", why.rsplit(": ").next().unwrap_or(&why).trim_end_matches('.')) } else { String::new() };
            }
            FromSession::NotAllowed => {
                (self.opening, self.session, self.opening_until) = (false, None, None);
                if self.state.link != Link::Allow {
                    tracing::info!("phone: it refuses its messages until its owner allows this computer (its switches for it)");
                }
                self.state.link = Link::Allow;
                self.state.trouble.clear();
                self.said_why.clear();
                let since = *self.allow_since.get_or_insert_with(Instant::now);
                self.retry_at = Some(Instant::now() + if since.elapsed() < ALLOW_FOR { ALLOW_EVERY } else { ALLOW_LATER });
            }
            FromSession::Up(listed) => {
                (self.tries, self.retry_at) = (0, None);
                tracing::info!("phone: its messages are open ({} in its inbox)", listed.len());
                self.state.link = Link::Ready;
                self.state.trouble.clear();
                self.said_why.clear();
                self.allow_since.get_or_insert_with(Instant::now);
                let now_s = now() as i64;
                let mut fresh = Vec::new();
                // (Oldest first, as conversations keep them.)
                for row in listed.into_iter().rev() {
                    if self.seen.contains(&row.handle) {
                        continue;
                    }
                    // One that has just come (while the service wasn't
                    // there to be told) is fetched whole and told of; the
                    // rest is what there was.
                    if !row.read && when(&row.datetime).is_some() && just_came(&row.datetime, now_s) {
                        fresh.push(row.handle);
                    } else {
                        self.keep(&row, row.subject.clone());
                    }
                }
                if let Some(cmds) = &self.session {
                    for handle in fresh {
                        let _ = cmds.send(Cmd::Fetch(handle));
                    }
                }
                // Its contacts (names for numbers): asked for now, and
                // again while they haven't come.
                self.contacts_tries = 0;
                self.ask_contacts();
                self.count();
            }
            FromSession::Came(row, whole) => {
                if self.seen.contains(&row.handle) {
                    return;
                }
                let row = Listed {
                    sender: if row.sender.is_empty() { whole.from.clone() } else { row.sender },
                    sender_name: if row.sender_name.is_empty() { whole.from_name.clone() } else { row.sender_name },
                    read: false,
                    ..row
                };
                let text = if whole.text.is_empty() { row.subject.clone() } else { whole.text };
                let (ti, mi) = self.keep(&row, text);
                self.count();
                self.came(ti, mi);
            }
            FromSession::Contacts(contacts) => {
                if !self.state.contacts {
                    tracing::info!("phone: its contacts came ({})", contacts.len());
                }
                self.contacts = contacts;
                self.state.contacts = true;
                self.contacts_at = None;
                for t in &mut self.threads {
                    if let Some(name) = pbap::name_of(&self.contacts, &t.address) {
                        t.name = name.to_owned();
                    }
                }
            }
            FromSession::Sent { to, text, done, note, answer } => {
                if done.is_ok() {
                    // (An iPhone keeps nothing of what was sent through
                    // it: kept here.)
                    let at = now() as i64;
                    let ti = self.thread_for(&to, "");
                    self.threads[ti].messages.push(Message { handle: String::new(), text, at, outgoing: true, read: true });
                    self.count();
                    // (Kept at once: what was sent is nowhere else.)
                    self.keep_for_next_run();
                }
                if let (Err(why), Some(note)) = (&done, note) {
                    self.notes.show(Some(note), Note::new("Not sent").body(why.clone()).kind(olo_kit_base::notify::NoteKind::Error));
                }
                if let Some(answer) = answer {
                    let _ = answer.send(Reply { ok: done.is_ok(), error: done.err(), ..Default::default() });
                }
            }
            FromSession::Down(why) => {
                (self.opening, self.session, self.opening_until) = (false, None, None);
                if !why.is_empty() {
                    tracing::info!("phone: the session ended: {why}");
                    if self.phone.as_ref().is_some_and(|p| p.connected) {
                        self.state.trouble = format!("The phone’s messages stopped ({}).", why.trim_end_matches('.'));
                    }
                }
                self.state.link = Link::Away;
                // (Tried again at once, once: a link dropped and still there.)
                self.retry_at = Some(Instant::now() + Duration::from_secs(if self.tries == 0 { 2 } else { AWAY[self.tries.min(AWAY.len() - 1)] }));
                self.tries += 1;
            }
        }
    }

    /// Ask BlueZ for a channel to the phone's contacts; and, until they
    /// come, again: every 15 s for the first five minutes of a session
    /// (the phone's owner is turning its switches on), then every five
    /// minutes.
    fn ask_contacts(&mut self) {
        self.contacts_at = None;
        let (Some(profiles), Some(phone)) = (self.profiles.clone(), &self.phone) else { return };
        if !phone.contacts || self.state.link != Link::Ready {
            return;
        }
        let path = phone.path.clone();
        let _ = std::thread::Builder::new().name("phone-open".into()).spawn(move || {
            if let Err(e) = profiles.open(&path, Channel::Contacts) {
                tracing::debug!("phone: its contacts: {e}");
            }
        });
        if !self.state.contacts {
            self.contacts_tries += 1;
            self.contacts_at = Some(Instant::now() + Duration::from_secs(if self.contacts_tries <= 20 { 15 } else { 300 }));
        }
    }

    /// The conversation with `address` (made if there's none), named by
    /// the contacts, else as `named`, else by the address itself.
    fn thread_for(&mut self, address: &str, named: &str) -> usize {
        let key = pbap::key(address);
        if let Some(i) = self.threads.iter().position(|t| t.key == key) {
            return i;
        }
        let name = pbap::name_of(&self.contacts, address).map(str::to_owned).or_else(|| Some(named.trim().to_owned()).filter(|n| !n.is_empty())).unwrap_or_else(|| address.to_owned());
        self.threads.push(Conversation { key, name, address: address.to_owned(), messages: Vec::new() });
        self.threads.len() - 1
    }

    /// Keep a message that came, in its conversation: where it is.
    fn keep(&mut self, row: &Listed, text: String) -> (usize, usize) {
        self.seen.insert(row.handle.clone());
        let ti = self.thread_for(&row.sender, &row.sender_name);
        let at = when(&row.datetime).unwrap_or_else(|| now() as i64);
        let t = &mut self.threads[ti];
        let mi = t.messages.iter().position(|m| m.at > at).unwrap_or(t.messages.len());
        t.messages.insert(mi, Message { handle: row.handle.clone(), text, at, outgoing: false, read: row.read });
        (ti, mi)
    }

    /// How many conversations and unread messages there are, said.
    fn count(&mut self) {
        self.state.conversations = self.threads.len() as u32;
        self.state.unread = self.threads.iter().map(|t| t.unread() as u32).sum();
        self.notes.badge(self.state.unread);
    }

    /// A message has just come: told of, and its code taken up.
    fn came(&mut self, ti: usize, mi: usize) {
        let thread = &self.threads[ti];
        let message = &thread.messages[mi];
        let code = if self.settings.codes { codes::find(&message.text) } else { None };
        let id = code.map(|code| {
            self.next_code += 1;
            let from = code.service.clone().unwrap_or_else(|| thread.name.clone());
            let age = Duration::from_secs((now() as i64 - message.at).clamp(0, JUST) as u64);
            self.codes.push(Pending { id: self.next_code, code, from, until: Instant::now() + CODE_LIFE.saturating_sub(age), round: None, asking: None, given: HashSet::new() });
            self.state.codes += 1;
            self.state.code_at = Some(now());
            self.next_code
        });
        if self.settings.notify == Notify::Messages {
            let mut note = Note::new(thread.name.clone()).style(NoteStyle::Message).icon("phone").action("read", "Mark as Read").reply("Reply");
            note = note.body(if self.settings.show_text { message.text.clone() } else { "New message".into() });
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

    /// Send `text` to `to`: a conversation's key, or a number or an
    /// address.
    fn send(&mut self, to: &str, text: String, note: Option<NoteId>, answer: Option<mpsc::Sender<Reply>>) {
        let to = self.threads.iter().find(|t| t.key == to).map_or_else(|| to.trim().to_owned(), |t| t.address.clone());
        let refuse = |why: &str, answer: Option<mpsc::Sender<Reply>>| {
            if let Some(answer) = answer {
                let _ = answer.send(Reply { ok: false, error: Some(why.to_owned()), ..Default::default() });
            }
        };
        match &self.session {
            Some(cmds) if self.state.link == Link::Ready => {
                if let Err(mpsc::SendError(Cmd::Send { answer, .. })) = cmds.send(Cmd::Send { to, text, note, answer }) {
                    refuse("The phone went.", answer);
                }
            }
            _ => {
                if let Some(note) = note {
                    self.notes.show(Some(note), Note::new("Not sent").body("The phone isn’t connected.").kind(olo_kit_base::notify::NoteKind::Error));
                }
                refuse("The phone isn’t connected.", answer);
            }
        }
    }

    /// A conversation's messages read, here and on the phone.
    fn mark_read(&mut self, key: &str) {
        let Some(t) = self.threads.iter_mut().find(|t| t.key == key) else { return };
        let handles: Vec<String> = t
            .messages
            .iter_mut()
            .filter(|m| !m.outgoing && !m.read)
            .map(|m| {
                m.read = true;
                m.handle.clone()
            })
            .filter(|h| !h.is_empty())
            .collect();
        if let (Some(cmds), false) = (&self.session, handles.is_empty()) {
            let _ = cmds.send(Cmd::MarkRead(handles));
        }
        self.count();
    }

    /// What's asked on the socket.
    fn ask(&mut self, request: Request, on_a_screen: bool, answer: mpsc::Sender<Reply>) {
        let no = |why: &str| Reply { ok: false, error: Some(why.to_owned()), ..Default::default() };
        // (What a message says is the person's: not a program's on an
        // agent's screen.)
        let private = !matches!(request, Request::Status | Request::Browser { .. });
        let reply = match request {
            _ if private && on_a_screen => no("not from an agent's screen"),
            Request::Status => Reply { ok: true, state: Some(self.state.clone()), ..Default::default() },
            Request::Code => match self.codes.last() {
                Some(c) => Reply { ok: true, code: Some(c.code.code.clone()), from: Some(c.from.clone()), ..Default::default() },
                None => no("no code has come in the last few minutes"),
            },
            Request::Conversations => {
                let mut all = self.threads.clone();
                all.sort_by_key(|t| std::cmp::Reverse(t.latest()));
                Reply { ok: true, conversations: Some(all), ..Default::default() }
            }
            Request::Contacts { query } => {
                let q = query.trim().to_lowercase();
                let digits: String = q.chars().filter(char::is_ascii_digit).collect();
                let found = self.contacts.iter().filter(|c| {
                    q.is_empty()
                        || c.name.to_lowercase().contains(&q)
                        || c.emails.iter().any(|e| e.to_lowercase().contains(&q))
                        || (digits.len() >= 3 && c.phones.iter().any(|p| p.chars().filter(char::is_ascii_digit).collect::<String>().contains(&digits)))
                });
                Reply { ok: true, contacts: Some(found.map(|c| Person { name: c.name.clone(), phones: c.phones.clone(), emails: c.emails.clone() }).collect()), ..Default::default() }
            }
            Request::Send { to, text } => {
                if text.trim().is_empty() {
                    no("nothing to send")
                } else {
                    // (Answered when the phone has taken it.)
                    return self.send(&to, text, None, Some(answer));
                }
            }
            Request::MarkRead { key } => {
                self.mark_read(&key);
                Reply { ok: true, ..Default::default() }
            }
            Request::Reconnect => {
                self.tries = 0;
                // (Asked for: its owner is waited for afresh, and what
                // couldn't be done is tried again.)
                (self.allow_since, self.solicit_failed) = (None, false);
                self.devices();
                if !self.state.contacts {
                    self.contacts_tries = 0;
                    self.ask_contacts();
                }
                if self.session.is_none() && !self.opening && self.phone.is_some() {
                    self.open();
                }
                Reply { ok: true, state: Some(self.state.clone()), ..Default::default() }
            }
            Request::Music { action } => {
                use olo_protocol::phone::MusicAction;
                use olo_system::phone::music::Action;
                let playing = self.state.music.as_ref().is_some_and(|m| m.playing);
                let action = match action {
                    MusicAction::Play => Action::Play,
                    MusicAction::Pause => Action::Pause,
                    MusicAction::Toggle if playing => Action::Pause,
                    MusicAction::Toggle => Action::Play,
                    MusicAction::Next => Action::Next,
                    MusicAction::Previous => Action::Previous,
                };
                // (What it then plays comes as BlueZ says so.)
                match (self.profiles.as_ref(), self.music_player.as_deref()) {
                    (Some(profiles), Some(player)) => match profiles.music_act(player, action) {
                        Ok(()) => Reply { ok: true, ..Default::default() },
                        Err(e) => no(&format!("the phone didn't take it: {e}")),
                    },
                    _ => no("the phone isn't playing anything"),
                }
            }
            Request::Browser { .. } => Reply::default(),
        };
        // (What it changed is published before it's answered: who asked
        // may read the state next.)
        self.publish();
        let _ = answer.send(reply);
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

    /// Rounds that have waited long enough are decided; codes too old
    /// go; a phone out of reach is tried again when it's time.
    fn tick(&mut self) {
        let t = Instant::now();
        if self.keep_at.is_some_and(|at| at <= t) {
            self.keep_for_next_run();
        }
        if let Some(o) = &mut self.others
            && o.state.due().is_some_and(|at| at <= t)
        {
            let effects = o.state.tick(t);
            self.others_do(effects);
        }
        if self.profiles_at.is_some_and(|at| at <= t) {
            self.register();
            self.devices();
        }
        if self.contacts_at.is_some_and(|at| at <= t) {
            self.ask_contacts();
        }
        // A channel asked for that never came: given up, BlueZ told to
        // let go of it, and asked again (what comes of the old asking
        // now is old news).
        if self.opening && self.opening_until.is_some_and(|at| at <= t) {
            self.generation += 1;
            self.let_go = true;
            self.from_session(FromSession::NoChannel("Bluetooth said it would open them and no channel came".into()));
        }
        if self.retry_at.is_some_and(|at| at <= t) {
            self.retry_at = None;
            if self.session.is_none() && !self.opening && self.phone.is_some() {
                self.open();
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
            NoteEvent::Action { id, key, token } => match (self.about(id).cloned(), key.as_str()) {
                (Some(About::Message(thread, _)), "read") => self.mark_read(&thread),
                // Clicked: Phone, at its conversation (in front, by the
                // click's own leave to be).
                (Some(About::Message(thread, _)), "default") => {
                    // (Phone is the program beside this one.)
                    let me = std::env::current_exe().map(|p| p.with_file_name("olo-phone").to_string_lossy().into_owned()).unwrap_or_else(|_| "olo-phone".into());
                    let env: Vec<(&str, &str)> = token.as_deref().map(|t| ("XDG_ACTIVATION_TOKEN", t)).into_iter().collect();
                    if let Err(e) = olo_xdg::spawn_detached_app_with(&format!("'{}' --open '{}'", me.replace('\'', "'\\''"), thread.replace('\'', "'\\''")), "olo.phone", &env) {
                        tracing::warn!("phone: opening Phone: {e}");
                    }
                }
                (Some(About::Message(_, Some(code))), "copy") => {
                    if let Some(c) = self.codes.iter().find(|c| c.id == code) {
                        copy(c.code.code.clone());
                    }
                }
                // One of the phone's other notifications: its action,
                // done on the phone (Answer, Decline, Clear).
                (Some(About::Other(generation, uid) | About::Call(generation, uid)), "positive" | "negative") => {
                    if let Some(o) = self.others.as_ref().filter(|o| o.generation == generation) {
                        let effects = o.state.act(uid, key == "positive");
                        self.others_do(effects);
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
                let Some(About::Message(key, _)) = self.about(id).cloned() else { return };
                self.mark_read(&key);
                self.send(&key, text, Some(id), None);
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
        self.solicit();
        // (A count of what changed in the conversations and the
        // contacts, for a window that shows them to ask again by.)
        let digest = {
            use std::hash::{Hash as _, Hasher as _};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            self.contacts.len().hash(&mut h);
            for t in &self.threads {
                (&t.key, &t.name, t.messages.len()).hash(&mut h);
                for m in &t.messages {
                    (&m.handle, m.read, m.at).hash(&mut h);
                }
            }
            h.finish()
        };
        if digest != self.digest {
            self.digest = digest;
            self.state.changes += 1;
            // (Kept for the next run, once a burst of them has come.)
            if self.phone.is_some() && self.keep_at.is_none() {
                self.keep_at = Some(Instant::now() + KEEP_AFTER);
            }
        }
        // (Since when the link has stood as it does.)
        if self.published.as_ref().map(|p| p.link) != Some(self.state.link) {
            self.state.since = now();
        }
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
                            let reply = got.recv_timeout(Duration::from_secs(45)).unwrap_or_else(|_| Reply { ok: false, error: Some("the service is busy".into()), ..Default::default() });
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
    /// The loop's watch: quiet while each thing taken up is done with in
    /// time, and at rest however long; it says so when one isn't.
    #[test]
    fn a_loop_that_stops_is_noticed() {
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let f = fired.clone();
        let beat = super::watch(std::time::Duration::from_millis(150), move || {
            f.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let count = || fired.load(std::sync::atomic::Ordering::SeqCst);
        // At rest for longer than the limit: nothing.
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert_eq!(count(), 0);
        // Work done in time, again and again: nothing.
        for _ in 0..5 {
            beat.send(true).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(40));
            beat.send(false).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(count(), 0);
        // Taken up and never done with: said, once.
        beat.send(true).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(count(), 1);
    }

    use super::*;

    #[test]
    fn when_a_message_came() {
        assert_eq!(when("20260812T100000Z"), Some(1_786_528_800));
        assert_eq!(when("20260812T120000+0200"), Some(1_786_528_800));
        assert_eq!(when("20260812T030000-0700"), Some(1_786_528_800));
        // (Without a zone: this computer's, whatever it is.)
        let local = when("20260812T100000").unwrap();
        assert!((local - 1_786_528_800).abs() <= 14 * 3600);
        assert_eq!(when("yesterday"), None);
        assert_eq!(when(""), None);
        let now = 1_786_528_800;
        assert!(just_came("20260812T095600Z", now));
        assert!(!just_came("20260812T095400Z", now), "older than five minutes");
        assert!(!just_came("20260812T100200Z", now), "from the future");
        assert!(just_came("", now), "one that doesn't say when is new to us");
    }
}

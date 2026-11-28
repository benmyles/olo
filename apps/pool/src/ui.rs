//! Token Pool's unified single-window UI (`Accounts`, `Activity`, `Machines`).
//!
//! All three tabs follow one cohesive BeOS framed-list architecture:
//! - a fixed top header / alert bar;
//! - **one** sunken framed list well (`border(true)`, flush scrollbar inside);
//! - a fixed bottom action bar matching the menu bar's commands.
//!
//! Nothing scrolls as an unframed web page; every local vs remote Rex host
//! relationship is shown with native icons (`system`, `network`, `warning`),
//! consistent provider colour encoding (Claude Code = orange, Codex = blue),
//! and full tiling adaptability (including two-column horizontal-third Activity).

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use olo_kit::{
    AppCx, Bounds, Child, ClickEvent, Corner, Cx, Element, FontWeight, Hsla, InteractiveElement as _, IntoElement as _, Menu, MenuBar, MouseButton, ParentElement as _, Pixels, Proxy, ScrollHandle,
    SharedString, Styled as _, View, WindowHandle, WindowOptions, canvas, div, icons, menu, metrics as m, palette as olo_theme, point, px, size, style, theme,
    widgets::{self, Button, CheckBox, Graph, LIGHT, Light, MenuField, ScrollView, Series, StringView, TabView, TextControl, TextField, TextFieldEvent, button_width, level_bar, light, text_field},
};
use olo_token_pool::{
    activity::Activity,
    clients,
    codex_daemon::{self, Followed},
    gateway::{Pool, Shared, SignedInAs},
    hosts::{Hosts, Stands},
    import::{self, Found},
    model::{Account, Outcome, Provider, Window as QuotaWindow, now},
    running::{Process, Running},
    store,
};

use crate::{
    PROVIDERS,
    aggregate::{
        AccountRollup, LiveReq, MachineScope, RecentReq, Tab, account_out_of_quota, account_paid_scope, account_rollup, custody, demo_data, demo_enabled, demo_joined, demo_note, demo_waiting, folder,
        host_brief_sentence, host_stands, host_summary, hue, kept_names, last_checked_at, last_synced_at, live_requests, local_custody_summary, merged_activity, middle_truncate, provider_hue,
        recent_requests, sentence, sorted_accounts, sorted_hosts, sorted_providers, tab_lights,
    },
    client_index, clock, duration, plan_name, span, tokens,
};

const SIGN_IN_WAIT: Duration = Duration::from_secs(20 * 60);
const PROCESS_POLL_EVERY: Duration = Duration::from_secs(2);
const HOSTS_POLL_EVERY: Duration = Duration::from_secs(30);
const GRAPH_HEIGHT: f32 = 152.0;
const DETAIL_ROW: f32 = 16.0;
/// The group's band pinned at the top of the accounts list.
const BAND: f32 = 22.0;
/// A quota's line in an account's row (`inline_quota_row`).
const QUOTA_ROW: f32 = 18.0;
/// The space between the lines of an account's row's body.
const BODY_GAP: f32 = 3.0;

/// A line of an account's row as the list's pinned band sees it: its
/// height, and how much of its top and of its bottom no glyph, bar or
/// button reaches (less than they leave, at every scale: 10.5-point words
/// in 16 points leave nearer 3.5 above and 1.5 below).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Line {
    h: f32,
    top: f32,
    bottom: f32,
}

/// A row's facts (its email, plan, resets).
const FACT_LINE: Line = Line { h: DETAIL_ROW, top: 2.0, bottom: 1.0 };
/// A quota's (its bar 10 points of 18, its words 16).
const QUOTA_LINE: Line = Line { h: QUOTA_ROW, top: 3.0, bottom: 2.0 };
/// Today's (2 points over its words' 16).
const TODAY_LINE: Line = Line { h: 2.0 + DETAIL_ROW, top: 4.0, bottom: 1.0 };

/// Where the list's pinned band can end on an item without cutting a
/// line: the stretches from its top (points down it) no glyph reaches,
/// and how much of its end is clear (a row's padding, and its last line's
/// clear bottom).
#[derive(Clone, Debug, Default, PartialEq)]
struct Clear {
    from_top: Vec<(f32, f32)>,
    end: f32,
}
/// An account's or a host's row: the room above and below its words.
/// (Eight points, and the three on each side of the rule between rows
/// that a separator of the kit's had: the rule is the row's own now,
/// [`under_rule`].)
const ROW_PAD: f32 = 11.0;
const RECENT_TABLE_LIMIT: usize = 60;

/// Time spans for the Activity chart: (label, minutes, bucket minutes, grid seconds).
const SPANS: [(&str, i64, i64, f64); 3] = [("Last Hour", 60, 1, 600.0), ("Last 6 Hours", 360, 5, 3600.0), ("Last 24 Hours", 1440, 15, 4.0 * 3600.0)];

/// Messages to [`Hub`].
pub enum Msg {
    Changed,
    Clients([bool; 3]),
    ClientSet(Provider, bool, Result<(), String>, Option<bool>),
    /// A sign-in read ([`Read`]).
    SignedIn(Read),
    /// What became of a sign-in taken into the pool, its worker's
    /// ([`Taken`]).
    Taken(Taken),
    Done(Result<String, String>),
    Open,
    Command(String),
    SetClient(Provider, bool),
    SignIn(Provider, String, bool),
    CancelSignIn,
    TabChanged(Tab),
    Closed,
    CloseWindows,
}

/// The sign-ins begun ([`Ops::begin`]), and the latest begun or taken for
/// each account: only that one's result is taken, whichever way it was
/// begun (this computer's, or separately; for the account, or one Add
/// Account found to be it by its email), and whatever order the answers
/// come in (Astra's review, 2026-11-28: an older one, answered later,
/// replaced the newer and its "signed in separately"; Astra's and Neon's,
/// again: one added that turned out to be an account signed in again
/// meanwhile was never compared).
#[derive(Default)]
struct Ops {
    begun: u64,
    latest: HashMap<String, u64>,
}

impl Ops {
    /// A sign-in begun, for the account `target` (else one to be found
    /// as it's taken in, or a new one): its number, the latest for that
    /// account now.
    fn begin(&mut self, target: Option<&str>) -> u64 {
        self.begun += 1;
        if let Some(t) = target {
            self.latest.insert(t.to_owned(), self.begun);
        }
        self.begun
    }

    /// Whether a sign-in's result (`op`, for `target`) is still wanted
    /// before it's taken in: one for an account to be found always is
    /// (which it is is known as it's taken, [`Ops::take`]); one signed in
    /// again, only if no sign-in for it has begun since.
    fn current(&self, op: u64, target: Option<&str>) -> bool {
        target.is_none_or(|t| self.latest.get(t) == Some(&op))
    }

    /// Whether sign-in `op` is taken in as account `id`'s, the account it
    /// turned out to be (its target, the one with its email, or a new
    /// one): only if no sign-in for that account began, or was taken,
    /// after it, and then it's that account's latest. Asked as it's
    /// written (`Shared::take_sign_in`, under the pool's lock).
    fn take(&mut self, op: u64, id: &str) -> bool {
        if self.latest.get(id).is_some_and(|&later| later > op) {
            return false;
        }
        self.latest.insert(id.to_owned(), op);
        true
    }

    /// Whether `op`, taken in as `id`'s, is still that account's latest:
    /// what came of it is said only then.
    fn latest_for(&self, op: u64, id: &str) -> bool {
        self.latest.get(id).is_none_or(|&l| l == op)
    }
}

/// Sign-ins read while the pool's accounts weren't open yet (the keyring
/// they wait for was shut, and reading one may just have opened it): kept
/// in the order they came until the accounts are open (Astra's and Neon's
/// reviews, 2026-11-28: one slot, and a second replaced the first), and
/// how many times opening them had failed when the first came
/// (`Pool::open_tries`), so a failure since is said, not "Opening…" for
/// ever.
#[derive(Default)]
struct Held {
    reads: VecDeque<Read>,
    tries_then: Option<u64>,
}

impl Held {
    fn hold(&mut self, read: Read, tries: u64) {
        self.tries_then.get_or_insert(tries);
        self.reads.push_back(read);
    }

    fn is_empty(&self) -> bool {
        self.reads.is_empty()
    }

    /// The accounts as they are now (`loaded`, after `tries` failures to
    /// open): once open, all that's held, in the order it came; until
    /// then, whether opening them failed again since last looked.
    fn looked(&mut self, loaded: bool, tries: u64) -> (Vec<Read>, bool) {
        if loaded {
            self.tries_then = None;
            return (self.reads.drain(..).collect(), false);
        }
        match self.tries_then {
            Some(then) if tries > then => {
                self.tries_then = Some(tries);
                (Vec::new(), true)
            }
            _ => (Vec::new(), false),
        }
    }
}

/// Account `id`, just taken in, into the vault before its sign-in's own
/// copy goes (`discard`): what came of each try `said` (why it isn't
/// saved yet, or nothing), tried again after `wait` while it can't be (the
/// keyring shut, the disk full). Nothing more is said of one removed
/// meanwhile, nor by a copy that handed over, its copy let go (Astra's and
/// Neon's reviews, 2026-11-28: one removed while a save was tried again
/// "joined" once it was).
fn keep_taken(shared: &Shared, id: &str, discard: impl Fn(), mut said: impl FnMut(Option<String>), wait: impl Fn()) {
    let gone = || shared.pool.lock().unwrap().account(id).is_none();
    loop {
        if gone() || shared.drain.retired() {
            return discard();
        }
        let unsaved = shared.keep(Duration::from_secs(15)).err();
        if gone() {
            return discard();
        }
        let done = unsaved.is_none();
        if done {
            discard();
        }
        said(unsaved);
        if done {
            return;
        }
        wait();
    }
}

/// A sign-in read and not taken (cancelled, replaced, or its account
/// signed in again since): its private directory, if it has one, removed,
/// off the window's thread.
fn let_go(read: Read) {
    if let Some((_, dir)) = read.separate
        && !dir.as_os_str().is_empty()
    {
        std::thread::spawn(move || import::discard(&dir));
    }
}

/// A sign-in read, for the account `target` (else a new one).
pub struct Read {
    found: Result<Found, String>,
    target: Option<String>,
    /// The sign-in it was begun as ([`Hub::begin`]): one begun for its
    /// account since is the one taken (Astra's review, 2026-11-28: an
    /// older one, answered later, replaced the newer).
    op: u64,
    /// A separate one's: the wait it was (`Waiting::cancel`), and its
    /// private directory, its only copy, kept until it's in the vault.
    separate: Option<(Arc<AtomicBool>, std::path::PathBuf)>,
    /// The keyring was opened as it was looked for (`Imported::unlocked`).
    unlocked: bool,
}

/// What became of a sign-in taken into the pool ([`Hub::adopt`]'s worker).
pub struct Taken {
    op: u64,
    target: Option<String>,
    outcome: Result<Kept, String>,
}

/// An account signed in, and whether it's in the vault yet.
struct Kept {
    id: String,
    name: String,
    again: bool,
    quit_agy: bool,
    /// Why it isn't in the vault yet (tried again until it is), if not.
    unsaved: Option<String>,
}

/// The program a separate sign-in runs in Terminal, as the person sees it
/// there: what the footer calls it, waiting and after (one name at every
/// width, the one "You can quit agy" uses: Opus's review, 2026-11-28).
fn program(p: Provider) -> &'static str {
    match p {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Antigravity => "agy",
    }
}

struct Waiting {
    provider: Provider,
    target: Option<String>,
    cancel: Arc<AtomicBool>,
    /// When it's given up on: the countdown the footer shows, and the
    /// worker's own end.
    until: Instant,
}

/// Whether a separate sign-in's result (its wait's `token`) is the one
/// waited for now (`current`): not cancelled, not replaced by another.
/// Read just before Cancel or another sign-in, it's dropped as they are.
fn waited_for(current: Option<&Arc<AtomicBool>>, token: &Arc<AtomicBool>) -> bool {
    !token.load(Ordering::Relaxed) && current.is_some_and(|c| Arc::ptr_eq(c, token))
}

/// A separate sign-in in `dir` waited for, until `until`: what it read
/// (looked for every two seconds; a file still being written is read
/// again), why it never will be (one that's there and can't be: at once),
/// or that it timed out. None once cancelled.
fn wait_for_sign_in(p: Provider, dir: &std::path::Path, cancel: &AtomicBool, until: Instant) -> Option<Result<Found, String>> {
    loop {
        std::thread::sleep(Duration::from_secs(2));
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        match import::read_separate(p, dir) {
            Ok(found) => return Some(Ok(found)),
            Err(import::Unread::Never(e)) => return Some(Err(e)),
            Err(import::Unread::NotYet(_)) => {}
        }
        if Instant::now() >= until {
            return Some(Err(format!("The {} sign-in timed out.", program(p))));
        }
    }
}

/// How long is left of a wait, as the footer counts it down: minutes and
/// seconds ("4:12"), rounded up, never under 0:00.
fn left(until: Instant, now: Instant) -> String {
    let s = until.saturating_duration_since(now).as_secs_f64().ceil() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// A note in the window's footer: what was just done, or what went wrong
/// (`error`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub text: String,
    pub error: bool,
    /// How much of `text`'s start is an account's name: all that's cut
    /// short where the room is too little, never what follows (“You can
    /// quit agy.” is the point of that note).
    pub name_len: usize,
    /// Where its quiet end begins, if it has one: what it says in the
    /// text's own ink, that in the note's grey (the wait's time left).
    pub quiet_from: Option<usize>,
}

impl From<(String, bool)> for Note {
    fn from((text, error): (String, bool)) -> Note {
        Note { text, error, name_len: 0, quiet_from: None }
    }
}

impl Note {
    /// That `name` is signed in: joined, or `again`. After a separate
    /// Antigravity sign-in (`quit_agy`), short enough for the footer at
    /// its width, with what's to be done: agy goes on in Terminal once
    /// it's signed in (Claude Code's and Codex's sign-ins end), on a home
    /// that's gone now.
    pub fn signed_in(name: &str, again: bool, quit_agy: bool) -> Note {
        let said = match (again, quit_agy) {
            (false, true) => " joined. You can quit agy.",
            (true, true) => " signed in. You can quit agy.",
            (false, false) => " joined the pool.",
            (true, false) => " is signed in again.",
        };
        Note { text: format!("{name}{said}"), error: false, name_len: name.len(), quiet_from: None }
    }

    /// The separate sign-in of `p` waited for, with `left` of its time
    /// ("Waiting for agy in Terminal… 4:12 left"): the same words at every
    /// width, and the time it has, since it ends (Opus's review,
    /// 2026-11-28). What asks something of the person in the text's ink,
    /// the time in the note's grey: all grey, and its time without "left",
    /// it read as the idle window's "Updated 10:20", a clock's time
    /// (Opus's review, again).
    pub fn waiting(p: Provider, left: &str) -> Note {
        let head = format!("Waiting for {} in Terminal… ", program(p));
        Note { quiet_from: Some(head.len()), ..(format!("{head}{left} left"), false).into() }
    }
}

/// What the demo says when asked to sign in: its accounts are made up, and
/// nothing of the person's (their keyring, Terminal) is touched.
const DEMO_SIGN_IN: &str = "The demo’s accounts are made up: nothing is signed in.";

#[derive(Clone, Default, PartialEq)]
pub struct Told {
    clients: Option<[bool; 3]>,
    note: Option<Note>,
    /// The separate sign-in waited for: whose, for which account, until.
    waiting: Option<(Provider, Option<String>, Instant)>,
    /// The account just signed in, to show (the count: the same one
    /// again is shown again).
    joined: Option<(String, u64)>,
}

#[derive(Default)]
pub struct Open {
    open: AtomicBool,
    tab: AtomicUsize,
}

impl Open {
    fn state(&self) -> String {
        serde_json::json!({
            "open": self.open.load(Ordering::Relaxed),
            "tab": self.tab.load(Ordering::Relaxed),
        })
        .to_string()
    }
}

pub struct Hub {
    pub shared: Arc<Shared>,
    proxy_port: u16,
    proxy: Proxy,
    win: Option<WindowHandle<PoolWindow>>,
    hosts: Arc<Hosts>,
    clients: Option<[bool; 3]>,
    client_tx: std::sync::mpsc::Sender<(Provider, bool)>,
    /// Written only with [`Hub::set_note`], which keeps its time with it.
    note: Option<Note>,
    /// When the note that agy can be quit goes: at the first quota read
    /// after this (a minute after it was said: seen, and not still there
    /// an hour later). That note's own: cleared with any other note.
    note_until: Option<i64>,
    waiting: Option<Waiting>,
    /// This computer's sign-in being read: one at a time (a second click is
    /// neither a second worker nor a second password prompt).
    importing: bool,
    /// Sign-ins read while the pool's accounts weren't open yet ([`Held`]).
    held: Held,
    /// Why the vault's saves fail, as last said ([`Hub::said_save`]).
    save_said: Option<String>,
    /// The sign-ins begun ([`Ops`]): asked by the worker that takes them
    /// in too, as it does.
    ops: Arc<std::sync::Mutex<Ops>>,
    /// The worker that takes sign-ins into the pool, one after another in
    /// the order they came (`Hub::adopt`).
    adopt_tx: std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>>,
    joined: Option<(String, u64)>,
    open: Arc<Open>,
}

impl Hub {
    pub fn new(shared: Arc<Shared>, hosts: Arc<Hosts>, proxy_port: u16, proxy: Proxy) -> Hub {
        // Single serialized worker for client toggles so rapid clicks execute
        // strictly in click order and Antigravity enable gates on proxy CA readiness.
        let (client_tx, client_rx) = std::sync::mpsc::channel::<(Provider, bool)>();
        let worker_shared = shared.clone();
        let worker_proxy = proxy.clone();
        let _ = std::thread::Builder::new().name("pool-clients".into()).spawn(move || {
            while let Ok((p, on)) = client_rx.recv() {
                if p == Provider::Antigravity && on {
                    let ready = worker_shared.wait_proxy_ready(Duration::from_secs(5));
                    if !ready.proxy_ok || !ready.agy_ok {
                        let why = ready.agy_error.or(ready.error).unwrap_or_else(|| "Antigravity proxy isn’t ready yet.".into());
                        worker_proxy.send(Msg::ClientSet(p, on, Err(why), None));
                        continue;
                    }
                }
                let pointed = clients::pointed(p, proxy_port);
                let r = clients::set(p, on, proxy_port);
                let restarted = p == Provider::Codex && r.is_ok() && matches!(codex_daemon::follow(), Ok(Followed::Restarted(_)));
                worker_proxy.send(Msg::ClientSet(p, on, r, if pointed { None } else { Some(restarted) }));
            }
        });
        // (And one taking sign-ins into the pool, in the order they came:
        // two at once, each looking for its account before the other was
        // written, made two of one; Astra's and Neon's reviews,
        // 2026-11-28.)
        let (adopt_tx, adopt_rx) = std::sync::mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let _ = std::thread::Builder::new().name("pool-adopt".into()).spawn(move || {
            while let Ok(job) = adopt_rx.recv() {
                job();
            }
        });
        let mut hub = Hub {
            shared,
            proxy_port,
            proxy,
            win: None,
            hosts,
            clients: None,
            client_tx,
            adopt_tx,
            note: None,
            note_until: None,
            waiting: None,
            importing: false,
            held: Held::default(),
            save_said: None,
            ops: Arc::default(),
            joined: None,
            open: Arc::default(),
        };
        if demo_enabled() {
            hub.demo_told();
        }
        hub
    }

    /// What the demo shows of a sign-in, for pictures: a note
    /// (`OLO_POOL_DEMO_NOTE`), an account just signed in separately
    /// (`OLO_POOL_DEMO_JOINED`), a separate sign-in waited for
    /// (`OLO_POOL_DEMO_WAITING`).
    fn demo_told(&mut self) {
        self.set_note(demo_note().map(Note::from));
        if let Some(id) = demo_joined()
            && let Some(a) = demo_data().pool.account(&id)
        {
            self.set_note(Some(Note::signed_in(&a.name, false, a.provider == Provider::Antigravity)));
            self.joined = Some((id, 1));
        }
        if demo_waiting() {
            // (Begun 2 min 18 s ago, as a picture would find it.)
            self.waiting = Some(Waiting { provider: Provider::Antigravity, target: None, cancel: Arc::default(), until: Instant::now() + SIGN_IN_WAIT - Duration::from_secs(138) });
        }
    }

    fn told(&self) -> Told {
        Told { clients: self.clients, note: self.note.clone(), waiting: self.waiting.as_ref().map(|w| (w.provider, w.target.clone(), w.until)), joined: self.joined.clone() }
    }

    /// The footer's note now `note` (None: none), with no time of its own:
    /// the time that agy can be quit is that note's ([`Hub::signed_in`]
    /// sets it after this), and another note written over it isn't cleared
    /// when it runs out (Astra's and Neon's reviews, 2026-11-28: a host's
    /// error went a minute after a sign-in's note it replaced).
    fn set_note(&mut self, note: Option<Note>) {
        self.note = note;
        self.note_until = None;
    }

    fn tell(&self) {
        if let Some(w) = &self.win {
            w.messenger().send(WinMsg::Told(self.told()));
        }
    }

    pub fn open_tab(&mut self, tab: Option<Tab>) {
        let initial = tab.unwrap_or_else(|| Tab::from_index(self.open.tab.load(Ordering::Relaxed)));
        if let Some(t) = tab {
            self.open.tab.store(t as usize, Ordering::Relaxed);
        }
        let existed = self.win.is_some();
        let (shared, hosts, port, told) = (self.shared.clone(), self.hosts.clone(), self.proxy_port, self.told());
        // (At least a vertical third of the smallest screen beside
        // Deskbar, 349 points of content: narrower than its footers' three
        // buttons, which take two rows there, `footer_rows`.)
        let options = WindowOptions::new("Token Pool", WIDTH, OPENS_TALL).min_size(MIN_WIDTH, 220.0).keep_size("main");
        olo_kit::open_or_raise(&mut self.win, options, move |cx| PoolWindow::new(shared, hosts, port, told, initial, cx));
        self.open.open.store(true, Ordering::Relaxed);
        if existed
            && let Some(t) = tab
            && let Some(w) = &self.win
        {
            w.messenger().send(WinMsg::Tab(t));
        }
        self.read_clients();
    }

    fn read_clients(&self) {
        let (proxy, port) = (self.proxy.clone(), self.proxy_port);
        std::thread::spawn(move || proxy.send(Msg::Clients(PROVIDERS.map(|p| clients::enabled(p, port)))));
    }

    fn set_client(&mut self, p: Provider, on: bool) {
        let _ = self.client_tx.send((p, on));
    }

    fn import_local(&mut self, p: Provider, target: Option<String>) {
        // (The demo touches nothing of the person's: no keyring asked.)
        if demo_enabled() {
            return self.set_note(Some((DEMO_SIGN_IN.to_owned(), false).into()));
        }
        if self.importing {
            return;
        }
        self.importing = true;
        let op = self.ops.lock().unwrap().begin(target.as_deref());
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let imported = import::this_computer(p);
            proxy.send(Msg::SignedIn(Read { found: imported.found, target, op, separate: None, unlocked: imported.unlocked }));
        });
    }

    fn sign_in_separately(&mut self, p: Provider, target: Option<String>) {
        // (The demo touches nothing of the person's: no Terminal started.)
        if demo_enabled() {
            return self.set_note(Some((DEMO_SIGN_IN.to_owned(), false).into()));
        }
        if let Some(w) = self.waiting.take() {
            w.cancel.store(true, Ordering::Relaxed);
        }
        let op = self.ops.lock().unwrap().begin(target.as_deref());
        let cancel = Arc::new(AtomicBool::new(false));
        let until = Instant::now() + SIGN_IN_WAIT;
        self.waiting = Some(Waiting { provider: p, target: target.clone(), cancel: cancel.clone(), until });
        self.set_note(None);
        let proxy = self.proxy.clone();
        // Its directory made, its client found and Terminal started on the
        // worker too (files, and programs looked for): the window waits for
        // none of it, and says what went wrong as it would a sign-in's.
        std::thread::spawn(move || {
            let (dir, command) = match import::begin_separate(p) {
                Ok(begun) => begun,
                Err(e) => return proxy.send(Msg::SignedIn(Read { found: Err(e), target, op, separate: Some((cancel, std::path::PathBuf::new())), unlocked: false })),
            };
            // (Cancelled, or another begun, while it was made ready:
            // Terminal isn't opened for it. Astra's review, 2026-11-28.)
            if cancel.load(Ordering::Relaxed) {
                return import::discard(&dir);
            }
            let mut argv: Vec<std::ffi::OsString> = vec!["olo-terminal".into(), "-e".into()];
            argv.extend(command.into_iter().map(Into::into));
            let found = match olo_xdg::open::spawn(&argv) {
                Ok(()) => wait_for_sign_in(p, &dir, &cancel, until),
                Err(e) => Some(Err(format!("Couldn’t open Terminal: {e}"))),
            };
            match found {
                // (Its directory kept with what it read: its only copy,
                // let go once it's in the vault, `Hub::adopt`.)
                Some(Ok(found)) => proxy.send(Msg::SignedIn(Read { found: Ok(found), target, op, separate: Some((cancel, dir)), unlocked: false })),
                Some(Err(e)) => {
                    import::discard(&dir);
                    proxy.send(Msg::SignedIn(Read { found: Err(e), target, op, separate: Some((cancel, std::path::PathBuf::new())), unlocked: false }));
                }
                None => import::discard(&dir),
            }
        });
    }

    /// `read` taken in; or, the pool's accounts not open yet (the keyring
    /// they wait for was shut: reading this sign-in may just have opened
    /// it), kept until they are, in the order they came, and they're
    /// opened at once.
    fn take_in(&mut self, read: Read) {
        let (loaded, tries) = {
            let p = self.shared.pool.lock().unwrap();
            (p.loaded, p.open_tries)
        };
        if !loaded || !self.held.is_empty() {
            self.shared.open_again();
            self.set_note(Some(("Opening the pool’s accounts to add it…".to_owned(), false).into()));
            self.held.hold(read, tries);
            return;
        }
        self.adopt(read);
    }

    /// What `read` found into the pool, on the worker that takes them in
    /// one after another in the order they came (the vault written to
    /// disk, a capture forgotten: never on the window's thread, Astra's
    /// review, 2026-11-28): the account it is again (its target, else the
    /// one with its email), else a new one, signed in separately if it was
    /// one of those; nothing, if a newer sign-in for that account began or
    /// was taken since. Said only once it's in the vault: a separate one's
    /// own copy (its directory) goes then, and not before.
    fn adopt(&mut self, read: Read) {
        let Read { found, target, op, separate, .. } = read;
        let Ok(found) = found else { return };
        let dir = separate.map(|(_, dir)| dir);
        let quit_agy = dir.is_some() && found.provider == Provider::Antigravity;
        let (shared, proxy, ops) = (self.shared.clone(), self.proxy.clone(), self.ops.clone());
        let job = move || {
            let discard = |dir: &Option<std::path::PathBuf>| {
                if let Some(d) = dir {
                    import::discard(d);
                }
            };
            // What the sign-in says of it, and where it was signed in, set
            // with it; nothing kept for one removed as it was read.
            let said = SignedInAs { email: found.email.clone(), plan: found.plan.clone(), org: found.org.clone(), separate: dir.is_some() };
            let mut a = Account::new(store::new_id(), found.name(), found.provider);
            a.email = found.email;
            a.plan = found.plan;
            a.org = found.org;
            a.separate = dir.is_some();
            let (id, name, again) = match shared.take_sign_in(target.as_deref(), a, found.credential, said, |id| ops.lock().unwrap().take(op, id)) {
                Ok(Some(taken)) => taken,
                // (A newer sign-in for its account is its: nothing said.)
                Ok(None) => return discard(&dir),
                Err(e) => {
                    discard(&dir);
                    return proxy.send(Msg::Taken(Taken { op, target, outcome: Err(e) }));
                }
            };
            // Into the vault before its own copy goes, on a thread of its
            // own (the next sign-in taken in meanwhile).
            std::thread::spawn(move || {
                let said = |unsaved| {
                    let kept = Kept { id: id.clone(), name: name.clone(), again, quit_agy, unsaved };
                    proxy.send(Msg::Taken(Taken { op, target: target.clone(), outcome: Ok(kept) }));
                };
                keep_taken(&shared, &id, || discard(&dir), said, || std::thread::sleep(Duration::from_secs(30)));
            });
        };
        if let Err(job) = self.adopt_tx.send(Box::new(job)) {
            // (Its worker gone, a panic: taken in here, as before one was.)
            tracing::warn!("pool: the worker that takes sign-ins in is gone");
            std::thread::spawn(job.0);
        }
    }

    /// The vault's saves failing (`Pool::save_error`: the disk full, the
    /// keyring shut) said once as they start to, and that note gone once
    /// one goes through again (Astra's review, 2026-11-28: it was the
    /// gateway's error, "Gateway Stopped", and stayed).
    fn said_save(&mut self, error: Option<String>) {
        if error == self.save_said {
            return;
        }
        match &error {
            Some(e) => self.set_note(Some((e.clone(), true).into())),
            None if self.note.as_ref().is_some_and(|n| Some(&n.text) == self.save_said.as_ref()) => self.set_note(None),
            None => {}
        }
        self.save_said = error;
        // (Told now: a change of the pool's goes no further, mostly.)
        self.tell();
    }

    /// Said that `name` (`id`) is signed in, and the account shown.
    fn signed_in(&mut self, id: &str, name: &str, again: bool, quit_agy: bool) {
        self.set_note(Some(Note::signed_in(name, again, quit_agy)));
        self.note_until = quit_agy.then(|| now() + 60);
        let n = self.joined.as_ref().map_or(1, |(_, n)| n + 1);
        self.joined = Some((id.to_owned(), n));
    }

    fn sync_all_hosts(&mut self) {
        let list = self.hosts.list();
        let mut started = 0usize;
        let mut first_err: Option<String> = None;
        for h in list.iter().filter(|h| h.wanted && h.doing.is_none()) {
            match self.hosts.act(&h.id, "sync") {
                Ok(()) => started += 1,
                Err(e) => {
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
            }
        }
        self.set_note(Some(
            if let Some(e) = first_err {
                (e, true)
            } else if started == 0 {
                ("No remote hosts to sync right now.".to_owned(), false)
            } else if started == 1 {
                ("Syncing 1 remote host…".to_owned(), false)
            } else {
                (format!("Syncing {started} remote hosts…"), false)
            }
            .into(),
        ));
    }

    fn command(&mut self, command: &str) {
        match command {
            "tab-accounts" => return self.open_tab(Some(Tab::Accounts)),
            "tab-activity" | "clients" => return self.open_tab(Some(Tab::Activity)),
            "tab-machines" | "hosts" => return self.open_tab(Some(Tab::Machines)),
            "sync-hosts" => {
                self.sync_all_hosts();
                return;
            }
            "add-host" => {
                olo_kit::app_menu::open_settings("rex");
                return;
            }
            _ => {}
        }
        let provider = command.rsplit(':').next().and_then(Provider::parse);
        // (Begun from the menu on another tab: Accounts shown, where its
        // wait, Cancel Sign-In and what came of it are. Astra's review,
        // 2026-11-28.)
        match (command.split(':').next(), provider) {
            (Some("import"), Some(p)) => {
                self.open_tab(Some(Tab::Accounts));
                self.import_local(p, None);
            }
            (Some("separate"), Some(p)) => {
                self.open_tab(Some(Tab::Accounts));
                self.sign_in_separately(p, None);
            }
            _ => {}
        }
    }

    pub fn message(&mut self, message: Box<dyn std::any::Any + Send>) {
        if let Some(launch) = message.downcast_ref::<olo_kit::instance::Launch>() {
            if let Some(tab) = requested_tab(&launch.args) {
                self.open_tab(Some(tab));
            } else if !launch.args.iter().any(|a| a == "--service") {
                self.open_tab(None);
            }
            return;
        }
        let Ok(msg) = message.downcast::<Msg>() else { return };
        match *msg {
            Msg::Changed => {
                if let Some(w) = &self.win {
                    w.messenger().send(WinMsg::Changed);
                }
                let save_error = self.shared.pool.lock().unwrap().save_error.clone();
                self.said_save(save_error);
                // (Looked at only while there's something to see to.)
                if self.held.is_empty() && self.note_until.is_none() {
                    return;
                }
                let (loaded, synced, tries, error) = {
                    let p = self.shared.pool.lock().unwrap();
                    (p.loaded, last_synced_at(&p), p.open_tries, p.error.clone())
                };
                // Sign-ins read while the accounts weren't open, taken in
                // now they are, in the order they came (one whose account
                // was signed in again since, dropped); or, they failed to
                // open again: why, said, and not "Opening…" for ever (the
                // sign-ins wait on, taken in once they do).
                let (opened, failed) = self.held.looked(loaded, tries);
                for read in opened {
                    if self.ops.lock().unwrap().current(read.op, read.target.as_deref()) {
                        self.adopt(read);
                    } else {
                        let_go(read);
                    }
                }
                if failed {
                    self.set_note(Some((error.unwrap_or_else(|| "The pool’s accounts couldn’t be opened.".to_owned()), true).into()));
                }
                // That agy can be quit: gone with the first quota read after
                // a minute.
                if let Some(until) = self.note_until
                    && synced.is_some_and(|t| t > until)
                {
                    self.set_note(None);
                }
            }
            Msg::Open => {
                olo_kit::instance::listen(crate::APP_ID, self.proxy.clone());
                self.open_tab(None);
                return;
            }
            Msg::Clients(on) => self.clients = Some(on),
            Msg::ClientSet(prov, on, r, restarted) => {
                if r.is_ok() {
                    if let Some(c) = self.clients.as_mut() {
                        c[client_index(prov)] = on;
                    }
                    self.shared.set_paused(prov, !on);
                }
                self.set_note(Some(
                    match r {
                        Ok(()) => {
                            if prov == Provider::Antigravity && restarted.is_some() {
                                if on {
                                    let is_fish = std::env::var("SHELL").is_ok_and(|s| s.ends_with("/fish") || s == "fish");
                                    if is_fish {
                                        let fish_p = std::env::var_os("XDG_CONFIG_HOME")
                                            .map(std::path::PathBuf::from)
                                            .filter(|p| !p.as_os_str().is_empty())
                                            .map(|c| folder(&c.join("fish/conf.d/olo-pool-agy.fish")))
                                            .unwrap_or_else(|| "~/.config/fish/conf.d/olo-pool-agy.fish".into());
                                        (format!("Antigravity uses the pool in new fish shells (or run: source {fish_p})."), false)
                                    } else {
                                        let sh_p = folder(&clients::agy_sh_path());
                                        (format!("Antigravity uses the pool in new shells (or run: . {sh_p})."), false)
                                    }
                                } else {
                                    ("Antigravity is back to its own sign-in in new shells.".to_owned(), false)
                                }
                            } else {
                                let when = match (prov, restarted) {
                                    (_, None) => "from its next request",
                                    (Provider::Codex, Some(true)) => "from now (its service restarted)",
                                    (Provider::Codex, Some(false)) => "from now",
                                    (Provider::Claude | Provider::Antigravity, Some(_)) => "from its next start",
                                };
                                let verb = if on { "uses the pool" } else { "is back to its own sign-in" };
                                (format!("{} {verb} {when}.", prov.client()), false)
                            }
                        }
                        Err(e) => (e, true),
                    }
                    .into(),
                ));
                self.read_clients();
            }
            Msg::Done(r) => {
                self.set_note(Some(
                    match r {
                        Ok(m) => (m, false),
                        Err(e) => (e, true),
                    }
                    .into(),
                ));
            }
            Msg::SignedIn(read) => {
                let waited = match &read.separate {
                    // A separate sign-in's: only the one waited for. One
                    // cancelled or replaced as it was read is dropped
                    // whole: no account added or signed in again, no note
                    // over another's.
                    Some((token, _)) => waited_for(self.waiting.as_ref().map(|w| &w.cancel), token),
                    // This computer's: one more may be read now. (A
                    // separate sign-in waited for meanwhile stays waited for,
                    // its Cancel Sign-In there.) The keyring opened as it
                    // was looked for: the pool's own accounts, kept in it
                    // too, are opened now, whatever was found (Astra's
                    // review, 2026-11-28).
                    None => {
                        self.importing = false;
                        if read.unlocked {
                            self.shared.open_again();
                        }
                        true
                    }
                };
                if !waited {
                    return let_go(read);
                }
                if read.separate.is_some() {
                    self.waiting = None;
                }
                // One begun for its account since, either way, is the one
                // taken: this one's dropped (Astra's review, 2026-11-28).
                let current = self.ops.lock().unwrap().current(read.op, read.target.as_deref());
                if !current {
                    let_go(read);
                } else if let Err(e) = &read.found {
                    let e = e.clone();
                    self.set_note(Some((e, true).into()));
                } else {
                    self.take_in(read);
                }
            }
            Msg::Taken(Taken { op, target, outcome }) => {
                // (What came of one whose account was signed in again since
                // isn't said over the newer one's.)
                match outcome {
                    Err(e) if self.ops.lock().unwrap().current(op, target.as_deref()) => self.set_note(Some((e, true).into())),
                    Err(_) => {}
                    Ok(k) => {
                        let latest = self.ops.lock().unwrap().latest_for(op, &k.id);
                        if latest {
                            match &k.unsaved {
                                None => self.signed_in(&k.id, &k.name, k.again, k.quit_agy),
                                Some(e) => {
                                    // In the pool, and shown, but not said
                                    // to be kept until it is. (Chosen once:
                                    // each try that fails again leaves the
                                    // list where the person has it.)
                                    let why = e.trim_end_matches('.');
                                    self.set_note(Some((format!("{} isn’t saved yet ({why}): tried again every 30 seconds.", k.name), true).into()));
                                    if self.joined.as_ref().is_none_or(|(id, _)| *id != k.id) {
                                        let n = self.joined.as_ref().map_or(1, |(_, n)| n + 1);
                                        self.joined = Some((k.id.clone(), n));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Msg::Command(command) => self.command(&command),
            Msg::SetClient(p, on) => self.set_client(p, on),
            Msg::SignIn(p, id, local) => {
                if local {
                    self.import_local(p, Some(id));
                } else {
                    self.sign_in_separately(p, Some(id));
                }
            }
            Msg::CancelSignIn => {
                if let Some(w) = self.waiting.take() {
                    w.cancel.store(true, Ordering::Relaxed);
                }
                self.set_note(None);
            }
            Msg::TabChanged(tab) => {
                self.open.tab.store(tab as usize, Ordering::Relaxed);
                return;
            }
            Msg::Closed => {
                self.win = None;
                self.open.open.store(false, Ordering::Relaxed);
                return;
            }
            Msg::CloseWindows => {
                if let Some(w) = self.win.take() {
                    w.close();
                }
                return;
            }
        }
        self.tell();
    }
}

pub fn requested_tab(args: &[String]) -> Option<Tab> {
    if let Some(i) = args.iter().position(|a| a == "--tab")
        && let Some(val) = args.get(i + 1)
        && let Some(t) = Tab::parse_name(val)
    {
        return Some(t);
    }
    if args.iter().any(|a| a == "--hosts-window") {
        return Some(Tab::Machines);
    }
    if let Ok(env_tab) = std::env::var("OLO_POOL_DEMO_TAB")
        && let Some(t) = Tab::parse_name(&env_tab)
    {
        return Some(t);
    }
    None
}

/// The Accounts menu (and Add Account…'s): each client's sign-in, this
/// computer's or a separate one. None begun while another is `waiting`:
/// one at a time, its Cancel Sign-In in the footer (Opus's review,
/// 2026-11-28).
fn add_menu(waiting: bool) -> Menu {
    let mut menu = Menu::new("Accounts");
    for (i, p) in PROVIDERS.into_iter().enumerate() {
        if i > 0 {
            menu = menu.separator();
        }
        let key = p.key();
        menu = menu
            .item(format!("Add This Computer’s {} Sign-In", p.client()), format!("import:{key}"))
            .enabled(!waiting)
            .item(format!("Sign In to {} Separately…", p.client()), format!("separate:{key}"))
            .enabled(!waiting);
    }
    menu
}

/// Crisp 16×16 native Olo icon element (`system`, `network`, `warning`, etc.).
fn icon_box(name: &'static str, size_pt: f32) -> Element {
    canvas(move |g, b| {
        icons::paint(g, b, name);
    })
    .paint_key(style::key((name, size_pt.to_bits())))
    .w(px(size_pt))
    .h(px(size_pt))
    .flex_shrink_0()
    .into_element()
}

/// 6×6 rounded provider swatch (`Claude Code` orange, `Codex` blue).
fn swatch_dot(color: Hsla, size_pt: f32) -> Element {
    let k = [color.h, color.s, color.l, color.a].map(f32::to_bits);
    canvas(move |g, b| {
        style::dot(g, b.center(), size_pt, color);
    })
    .paint_key(style::key((k, size_pt.to_bits())))
    .w(px(size_pt))
    .h(px(size_pt))
    .flex_shrink_0()
    .into_element()
}

/// Provider identity colour: shifts Antigravity purple ~20° warmer toward magenta
/// and ~10% lighter in dark mode so its chart line and dots separate unmistakably
/// from Codex blue while keeping light mode unchanged.
fn provider_color(p: &olo_kit::palette::Palette, prov: Provider) -> Hsla {
    if prov == Provider::Antigravity && p.mode == olo_theme::ThemeMode::Dark { style::c([0xd2, 0x7c, 0xf2, 0xff]) } else { style::c(p.hue(provider_hue(prov))) }
}

fn client_color(p: &olo_kit::palette::Palette, client: &str) -> Hsla {
    if client == "Antigravity" && p.mode == olo_theme::ThemeMode::Dark { style::c([0xd2, 0x7c, 0xf2, 0xff]) } else { style::c(p.hue(hue(client))) }
}

/// Readable secondary text colour (meets ≥4.5:1 contrast on both panel and list well).
fn secondary_ink(p: &olo_kit::palette::Palette, over: olo_theme::Rgba) -> Hsla {
    let target = if p.mode == olo_theme::ThemeMode::Dark { olo_theme::mix(p.panel_text, over, 0.28) } else { olo_theme::mix(p.panel_text, over, 0.36) };
    style::c(olo_theme::readable(target, over, 4.6))
}

/// How a footer note is inked on the panel: set small (a row's details'
/// size), so further from the panel than other secondary words, 5.5:1 or
/// more; an error's red no less than 4.5:1 there.
fn note_ink(p: &olo_kit::palette::Palette, error: bool) -> olo_theme::Rgba {
    if error { olo_theme::readable(p.failure, p.panel_background, 4.5) } else { olo_theme::readable(olo_theme::mix(p.panel_text, p.panel_background, 0.3), p.panel_background, 5.5) }
}

/// How much more room there is between a footer button and the footer's
/// words (its note after Edit Account…, its time before Refresh Quota)
/// than between the buttons: 16 points then (the words aren't a button).
const NOTE_GAP: f32 = 4.0;

/// The window's width when it opens.
const WIDTH: f32 = 640.0;

/// The narrowest it goes: a vertical third of the smallest screen
/// beside Deskbar (349 points of content), and a little more.
const MIN_WIDTH: f32 = 346.0;

/// The room round the tab view, less in a narrow or short window.
fn outer_pad(vp: olo_kit::Size<Pixels>) -> f32 {
    if f32::from(vp.height) < 340.0 || f32::from(vp.width) < 460.0 { 6.0 } else { 10.0 }
}

/// How wide a tab's page is in a window `vp` big: inside the room round
/// the tab view and the tab view's own inset.
fn page_width(vp: olo_kit::Size<Pixels>) -> f32 {
    f32::from(vp.width) - 2.0 * outer_pad(vp) - 2.0 * m::WINDOW_INSET
}

/// How tall a tab's page is in a window `vp` big: under the menu bar,
/// inside the room round the tab view, its tabs and its inset.
fn page_height(vp: olo_kit::Size<Pixels>) -> f32 {
    f32::from(vp.height) - olo_kit::menu::BAR_HEIGHT - 2.0 * outer_pad(vp) - widgets::TAB_HEIGHT - 2.0 * m::WINDOW_INSET
}

/// Activity's Recent Requests at least, short: its heading and the
/// table's head and two rows.
const RECENT_LEAST: f32 = m::LABEL_ROW + 6.0 + 3.0 * (DETAIL_ROW + 6.0) + 2.0;

/// A finished request's status as its row says it: how long it took,
/// "Failed · 503", or what came of it.
fn status_said(r: &RecentReq) -> String {
    match r.request.outcome {
        Outcome::Completed => duration(r.request.ms),
        Outcome::Failed if r.request.status != 0 => format!("Failed · {}", r.request.status),
        other => other.label().to_owned(),
    }
}

/// A stepped series (each stretch's start and end at its count, as
/// `Activity::series` makes it) as one point a stretch, at its middle:
/// drawn the same at every scale. (The kit's graph keeps a point a
/// device pixel; a step's two corners a thousandth of a second apart,
/// it kept the higher at 1× and 1.5×, sloped, and both at 2×, square:
/// Opus's and Neon's reviews, 2026-11-28.)
fn midpoints(steps: &VecDeque<(f64, f64)>) -> VecDeque<(f64, f64)> {
    // (Each stretch's start, then its end.)
    let (starts, ends) = (steps.iter().step_by(2), steps.iter().skip(1).step_by(2));
    starts.zip(ends).map(|(start, end)| ((start.0 + end.0) / 2.0, start.1)).collect()
}

/// Activity's chart at least, short: its heading, its plot (`plot`
/// tall) and its legend (`legend`), 6 apart.
fn chart_least(plot: f32, legend: f32) -> f32 {
    m::LABEL_ROW + 6.0 + plot + 6.0 + legend
}

/// The plot's least, short: a sparkline's.
const PLOT_LEAST: f32 = 40.0;

/// Whether Activity's chart has room in one column `page_h` tall, its
/// legend `legend` tall, over Recent Requests' least, the Machine and
/// Span pop-ups over both, 8 apart. (Shorter, from 306 points to its
/// least of 220, the table went past the window's foot: Astra's and
/// Neon's reviews, 2026-11-28.)
fn activity_chart_fits(page_h: f32, legend: f32, recent: f32) -> bool {
    m::CONTROL_HEIGHT + 8.0 + chart_least(PLOT_LEAST, legend) + 8.0 + recent <= page_h
}

/// What Recent Requests keeps at least, short and in one column: what it
/// `needs`, but never more than the page has under the pop-ups.
fn recent_kept(needs: f32, page_h: f32) -> f32 {
    needs.min(page_h - m::CONTROL_HEIGHT - 8.0).max(0.0)
}

/// A request chosen in Recent Requests: its details' row under the
/// table, and the gap over it.
const PICKED_ROW: f32 = m::CONTROL_HEIGHT + 2.0 + 6.0;

/// The gateway's "stopped" box before it's first laid out: about its
/// height with a line of words (and the gap under it).
const ALERT_GUESS: f32 = 112.0;

/// What Activity's page has, its window `vp` big: its tab's page less
/// the gateway's "stopped" box over the tabs (`alert`, as tall as it
/// was laid out; None where the gateway runs) and its gap. (Budgeted
/// against the whole page, the chart and the table's least went past
/// the window's foot under the box: Astra's and Neon's reviews,
/// 2026-11-28.)
fn activity_page(vp: olo_kit::Size<Pixels>, alert: Option<f32>) -> f32 {
    page_height(vp) - alert.map_or(0.0, |h| (if h > 0.5 { h } else { ALERT_GUESS }) + 10.0)
}

/// The chart's plot at least, short and in one column, `page_h` tall,
/// its legend `legend`: as tall as the kit's `Graph` needs to draw its
/// axes and scale to what it shows (`GRAPH_AXES`) where Recent Requests
/// keeps its least under it; else as short as it goes. (At 64 points, a
/// quarter's, it was a bare line, its peak at half its height, its zeros
/// on the well's edge: Opus's and Neon's reviews, 2026-11-28.)
fn activity_plot_least(page_h: f32, legend: f32, recent: f32) -> f32 {
    let room = page_h - (m::CONTROL_HEIGHT + 8.0 + chart_least(0.0, legend) + 8.0 + recent);
    if room >= GRAPH_AXES { GRAPH_AXES } else { PLOT_LEAST }
}

/// How tall a row of items `widths` wide is, wrapped in `room`, `gap`
/// between them across and down, each a detail's row tall: Activity's
/// legend in one column, on one line or more.
fn wrapped_height(widths: &[f32], room: f32, gap: f32) -> f32 {
    let (mut lines, mut x) = (1usize, 0.0f32);
    for &w in widths {
        if x > 0.0 && x + gap + w > room {
            lines += 1;
            x = w;
        } else {
            x += if x > 0.0 { gap + w } else { w };
        }
    }
    lines as f32 * DETAIL_ROW + (lines - 1) as f32 * gap
}

/// The height from which the kit's `Graph` draws its axes and labels
/// (shorter, a sparkline).
const GRAPH_AXES: f32 = 76.0;

/// A tab's footer, as Accounts' and Machines' are: `lead` (Add …,
/// Edit/Remove …) from the left, `status` (what's said between) filling
/// the rest, and `last` (the global action) at the right, `gap` apart.
/// On one row where they fit in the `width` they have, as they always
/// were; else `last` on a row of its own under the others, `status`
/// before it (at 349 points, a vertical third beside Deskbar, Refresh
/// Quota ran 47 points out of the window; at 530, a quarter, Sync All
/// 48 out of its column: QA, 2026-11-28).
fn footer_rows(lead: Vec<Element>, status: Element, last: Element, gap: f32, two_rows: bool) -> Element {
    let row = || div().w_full().flex().flex_row().items_center().gap(px(gap)).h(px(m::CONTROL_HEIGHT));
    let mut first = row();
    for b in lead {
        first = first.child(b);
    }
    if !two_rows {
        return first.child(status).child(last).into_element();
    }
    div().w_full().flex().flex_col().gap(px(m::TIGHT)).child(first).child(row().child(status).child(last)).into_element()
}

/// Machines' This Computer beside its Remote Rex Hosts (a window `w` by
/// `h`): in a short window wide enough for both columns, the hosts' at
/// least a list's least; else one over the other.
fn machines_two_col(w: f32, h: f32) -> bool {
    h < 520.0 && w >= 500.0
}

/// Whether a footer's buttons (`labels`, the first at least
/// `first_least` wide) are too wide for one row `width` wide: `gap`
/// between each two, and one more for the status between the last two
/// ([`footer_rows`]).
fn footer_wraps(labels: &[&str], first_least: f32, gap: f32, width: f32) -> bool {
    let buttons: f32 = labels.iter().enumerate().map(|(i, l)| if i == 0 { button_width(l, false).max(first_least) } else { button_width(l, false) }).sum();
    buttons + gap * labels.len() as f32 > width
}

/// Its height when it opens: with its tab and frame on the smallest
/// screen, 800 points, with room below (740 ran its bottom off it at 2×),
/// its list's foot between two lines of the accounts as they open, the
/// middle of the gap over a quota's bar (at 720 it cut "All models"' bar
/// in half, the first thing seen: Opus's and Neon's reviews, 2026-11-28).
const OPENS_TALL: f32 = 712.0;

/// Narrower than this, a footer note has a row of its own, above the
/// buttons. Beside three, a joined note needs about 600 points to keep
/// some of its name before " joined. You can quit agy." (606 for all of
/// "Workspace Dev"), and from 480, where it moved before, to about 550 it
/// showed nothing at all.
const NOTE_ROW: f32 = 600.0;

/// The note's own row, below `NOTE_ROW`: its capitals 10 (and a half)
/// under the list, its baseline 6 over the buttons, so it reads as their
/// caption (Opus's review, 2026-11-28: 17 and 15, it belonged to neither;
/// then 10 and 10, midway between them). Whole points, its baseline at 18
/// (capitals of 7.5): at 27.5, 1× set them 9 and 11. Always there below
/// `NOTE_ROW`, with when quota was read where there's no note, so the
/// list is as tall in every state (Opus's review, again: it came and
/// went, the list's height with it, and at 400 the time was nowhere).
const NOTE_ROW_UNDER: f32 = 6.0;
const NOTE_ROW_BASELINE: f32 = 18.0;

/// How far a tip's words are set in from its edge (the kit's: `TIGHT` and
/// a point, `tip::PAD_X`): a tip moved left by it has its words where the
/// note's are.
const TIP_PAD: f32 = m::TIGHT + 1.0;

/// How wide the kit's tip is for `text`, as the kit sizes it (its own
/// `tip_size`, which it keeps to itself; docs/TODO.md): one line as wide
/// as it is, up to 360 points, its frame and insets included; past that
/// wrapped, as narrow as its longest line then is; on whole points. A
/// tip's place over the footer's note is chosen by it: by the most a tip
/// may be, a wrapped one, much narrower, was placed against the list's
/// end, far from its note (Opus's and Neon's reviews, 2026-11-28).
fn tip_width(text: &str) -> f32 {
    const MAX_WIDTH: f32 = 360.0;
    let font = style::ui_font(FontWeight::NORMAL);
    let measure = |l: &str| f32::from(style::text_width(l.to_owned(), style::PLAIN_SIZE, font.clone()));
    let one_line = text.lines().map(measure).fold(0.0, f32::max);
    let frame = 2.0;
    let most = MAX_WIDTH - 2.0 * TIP_PAD - frame;
    let mut inner = one_line.ceil().min(most);
    let mut wrapper = olo_kit::text_system().line_wrapper(font.clone(), px(style::PLAIN_SIZE));
    let mut wraps = |width: f32| text.lines().any(|l| measure(l) <= most && wrapper.wrap_line(&[olo_kit::LineFragment::text(l)], px(width)).count() > 0);
    while inner < most && wraps(inner) {
        inner += 1.0;
    }
    if inner >= most {
        let lines = olo_kit::tip::paragraph_height(text, inner);
        let mut longest_at = |width: f32| {
            let mut longest: f32 = 0.0;
            for l in text.lines() {
                let mut from = 0;
                let ends: Vec<usize> = wrapper.wrap_line(&[olo_kit::LineFragment::text(l)], px(width)).map(|b| b.ix).chain([l.len()]).collect();
                for to in ends {
                    longest = longest.max(measure(l[from..to].trim_end()));
                    from = to;
                }
            }
            longest
        };
        loop {
            let mut fitted = longest_at(inner).ceil().min(inner);
            while fitted < inner && olo_kit::tip::paragraph_height(text, fitted) > lines {
                fitted += 1.0;
            }
            if fitted >= inner {
                break;
            }
            inner = fitted;
        }
    }
    (inner + 2.0 * TIP_PAD + frame).ceil()
}

/// Where the footer's note was last drawn, as its drawing found it: cut or
/// whole, and its place across the window. What the tip over it is made
/// from, at the next look (the room it's given is known only as it's drawn).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Drawn {
    cut: bool,
    left: f32,
    width: f32,
}

/// What of a footer note shows in `room`, as `width` measures and `cut`
/// cuts (to a width, with "…": where a word ends if asked, else anywhere):
/// all of its text if it fits; else, its text's first `name_len` bytes an
/// account's name, the name cut short, at least a letter of it, and all
/// that follows; else, one with no name, cut where a word ends; else
/// nothing (never a lone "…", nor "… joined", which names no one: Neon's
/// review, 2026-11-28).
fn fitted(note: &Note, room: f32, width: impl Fn(&str) -> f32, mut cut: impl FnMut(&str, f32, bool) -> String) -> Option<String> {
    let text = note.text.as_str();
    if width(text) <= room {
        return Some(text.to_owned());
    }
    if note.name_len > 0 && text.is_char_boundary(note.name_len) {
        let (name, rest) = text.split_at(note.name_len);
        let left = room - width(rest);
        if left <= 0.0 {
            return None;
        }
        let short = cut(name, left, false);
        if !short.chars().any(|c| c != '…' && !c.is_whitespace()) {
            return None;
        }
        let whole = format!("{short}{rest}");
        return (width(&whole) <= room).then_some(whole);
    }
    let short = cut(text, room, true);
    (short != "…" && !short.is_empty() && width(&short) <= room).then_some(short)
}

/// Where the footer's note goes: beside the buttons, on their labels'
/// baseline; or on a row of its own above them (`NOTE_ROW_UNDER`).
#[derive(Clone, Copy, PartialEq)]
enum NotePlace {
    Beside,
    Row,
}

/// The footer's note in the room it's given: in a row's details' size,
/// from the left, where `place` says; cut as `fitted` cuts it, and always
/// whole to agents. Where it was drawn, and whether cut, kept in `drawn`
/// (`invalidate` asked when that changed): the tip with its whole is the
/// window's, above it ([`PoolWindow::note_tip`]), never the kit's cut-text
/// tip, which opens below, on the window's frame and Refresh Quota (Opus's
/// and Neon's reviews, 2026-11-28).
fn footer_note(note: &Note, ink: olo_theme::Rgba, place: NotePlace, drawn: Rc<Cell<Drawn>>, invalidate: impl Fn() + 'static) -> Element {
    let text = SharedString::from(note.text.clone());
    let key = style::key((&note.text, note.name_len, note.quiet_from, ink, place == NotePlace::Row));
    let note = note.clone();
    let height = match place {
        NotePlace::Beside => m::CONTROL_HEIGHT,
        NotePlace::Row => NOTE_ROW_BASELINE + NOTE_ROW_UNDER,
    };
    canvas(move |g, b| {
        let font = style::ui_font(FontWeight::NORMAL);
        let width = |t: &str| f32::from(style::text_width(t.to_owned(), style::SMALL_SIZE, font.clone()));
        let shown = fitted(&note, f32::from(b.size.width), width, |t, w, words| {
            if words { style::fit_words(g, t.to_owned().into(), style::SMALL_SIZE, &font, px(w)).to_string() } else { style::fit(g, t.to_owned().into(), style::SMALL_SIZE, &font, px(w)).to_string() }
        });
        let baseline = match place {
            NotePlace::Beside => style::baseline_at(g, b.center().y, style::PLAIN_SIZE),
            // (Its baseline `NOTE_ROW_BASELINE` down.)
            NotePlace::Row => style::baseline_at(g, b.top() + px(NOTE_ROW_BASELINE - style::SMALL_SIZE * style::CAP_HEIGHT / 2.0), style::SMALL_SIZE),
        };
        let at = match (&shown, note.quiet_from) {
            // (Its quiet end, whole, in the note's grey after the rest in
            // the text's own ink.)
            (Some(shown), Some(from)) if shown == &note.text && note.text.is_char_boundary(from) => {
                let loud = style::c(theme::palette().panel_text);
                let head = style::label_on_baseline(g, b.left(), baseline, style::Align::Left, note.text[..from].to_owned(), style::SMALL_SIZE, font.clone(), loud);
                let tail =
                    style::label_on_baseline(g, b.left() + px(width(&note.text[..from])), baseline, style::Align::Left, note.text[from..].to_owned(), style::SMALL_SIZE, font.clone(), style::c(ink));
                head.union(&tail)
            }
            (Some(shown), Some(_)) => style::label_on_baseline(g, b.left(), baseline, style::Align::Left, shown.clone(), style::SMALL_SIZE, font.clone(), style::c(theme::palette().panel_text)),
            (Some(shown), None) => style::label_on_baseline(g, b.left(), baseline, style::Align::Left, shown.clone(), style::SMALL_SIZE, font.clone(), style::c(ink)),
            (None, _) => b,
        };
        g.agent(olo_kit::agent::Node::label(text.clone(), at));
        let now = Drawn { cut: shown.as_deref() != Some(text.as_ref()), left: f32::from(b.left()), width: f32::from(b.size.width) };
        let was = drawn.get();
        if now.cut != was.cut || (now.left - was.left).abs() > 0.5 || (now.width - was.width).abs() > 0.5 {
            drawn.set(now);
            invalidate();
        }
    })
    .paint_key(key)
    .w_full()
    .min_w_0()
    .h(px(height))
    .into_element()
}

/// The footer's time ("Updated 10:42") as its note is set: a row's
/// details' size and ink, on the buttons' labels' baseline, from the right
/// (its end `NOTE_GAP` in, 16 points before Refresh Quota). It was a
/// button's size, paler, 13 points off: the footer had two styles of the
/// same thing (Opus's review, 2026-11-28).
fn footer_time(text: String, ink: olo_theme::Rgba) -> Element {
    let key = style::key((&text, ink));
    let text = SharedString::from(text);
    canvas(move |g, b| {
        let baseline = style::baseline_at(g, b.center().y, style::PLAIN_SIZE);
        let at = style::label_on_baseline(g, b.right(), baseline, style::Align::Right, text.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL), style::c(ink));
        g.agent(olo_kit::agent::Node::label(text.clone(), at));
    })
    .paint_key(key)
    .w_full()
    .min_w_0()
    .h(px(m::CONTROL_HEIGHT))
    .into_element()
}

fn outcome_color(p: &olo_kit::palette::Palette, o: Outcome) -> Hsla {
    match o {
        Outcome::Completed => style::c(p.success),
        Outcome::RateLimited => style::c(p.warning),
        Outcome::Failed => style::c(p.failure),
        Outcome::Cancelled => secondary_ink(p, p.list_background),
    }
}

fn recent_details(r: &RecentReq) -> String {
    let req = &r.request;
    let mut s = format!("{} · {} · {} on {}", req.outcome.label(), r.machine_name, req.model, r.account);
    if !req.client.is_empty() {
        s.push_str(&format!(" · {}", req.client));
    }
    s.push_str(&format!(" · {} · {}", clock(req.at), duration(req.ms)));
    if req.status != 0 && req.outcome != Outcome::Completed {
        s.push_str(&format!(" · HTTP {}", req.status));
    }
    if req.outcome == Outcome::Completed || req.usage.input_tokens + req.usage.output_tokens > 0 {
        s.push_str(&format!(" · {} in · {} out", tokens(req.usage.input_tokens), tokens(req.usage.output_tokens)));
    }
    s
}

fn live_details(o: &LiveReq, now_ms: i64) -> String {
    let mut s = format!("Running · {} · {} on {}", o.machine_name, o.model, o.account);
    if !o.client.is_empty() {
        s.push_str(&format!(" · {}", o.client));
    }
    if o.started_ms > 0 {
        s.push_str(&format!(" · {} so far", duration((now_ms - o.started_ms).max(0) as u64)));
    }
    s
}

/// Crisp 16pt/14pt neutral-slate machine glyph (`is_local`: desktop tower; remote: stacked server nodes).
/// Uses the neutral slate ramp (#5E6E84 light / #8C9DB3 dark) so list rows never introduce red/green/blue
/// colour noise or disappear into dark backgrounds.
/// A row of a list under the rule that parts it from the row before: one
/// line, the row's own top edge. A chosen row's tint runs up to the rule
/// on both sides of it. Between rows stood the kit's separator, a groove
/// in six points of the list's ground: a tinted row ended three points
/// short of the rule, a strip of white between them (two pixels at 1x,
/// three at 1.25x; both reviewers, 2026-11-26: "should meet the
/// horizontal divider line flush", "end the band exactly on the rule").
/// Where in an account's row (points down it from its top, its rule above
/// not counted) the list's pinned group band can end without cutting a
/// line: from its top to its title's glyphs (`title`: how clear its
/// title's line is above and below, nothing where a button fills it),
/// between each two of its lines (`lines`, its body's), and from its last
/// line's glyphs to its end; past a line whose height isn't known before
/// layout, only its end's padding.
fn row_cuts(title: (f32, f32), lines: &[Option<Line>]) -> Clear {
    let mut from_top = vec![(0.0, ROW_PAD + title.0)];
    let mut y = ROW_PAD + m::CONTROL_HEIGHT;
    let mut clear = y - title.1;
    let mut end = ROW_PAD + title.1;
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            y += BODY_GAP;
        }
        let Some(l) = line else {
            from_top.push((clear, y));
            return Clear { from_top, end: ROW_PAD };
        };
        from_top.push((clear, y + l.top));
        y += l.h;
        clear = y - l.bottom;
        end = ROW_PAD + l.bottom;
    }
    Clear { from_top, end }
}

/// How far the first line under the pinned band's ledge (its last point,
/// a line of the bevel's shadow) is kept from it where there's room: 7
/// points (about 8 to its words, whose glyphs leave a point more than a
/// line's margins say): in the gap between two of a row's quotas, the
/// band ends where the one above ends. Put just over a line's words, they
/// touched the ledge and read as cut (Opus's and Neon's reviews,
/// 2026-11-28: at least 5, the tile's 9 ideal). At 5 or 6, the band's
/// name and the first line's stood 24 points apart at 1×, three off the
/// list's 21, and the layout review took them for rows out of step; 4
/// further off, they're plainly apart.
const UNDER_LEDGE: f32 = 7.0;

/// Where the list should stand (its offset: how far down its content its
/// view's top is) to show the item `row` whole below the band pinned at
/// its top (`band` points, its ledge the point after), 8 points under it,
/// if it isn't shown so: from `at`, the least way. The band's edge is
/// then where no line is cut on the item it ends on (`clear`: each item's,
/// its bounds in `items`; Opus's review, 2026-11-28: the first thing under
/// the tabs was half a line, in amber), in a gap between lines, as near
/// its middle as leaves `UNDER_LEDGE` under the ledge to the line after it
/// (Opus's and Neon's reviews, again: the line's words touched the
/// ledge); in the nearest such gap, up or down, that shows the row,
/// taking at most what leaves 4 points of the row's own end (`ROW_PAD`)
/// under its last line. Where none does, as near as no line is cut, the
/// gap's end; where nothing does, as far as shows the row. None where
/// it's shown as it is.
fn reveal_offset(row: (f32, f32), at: f32, view: f32, max: f32, band: f32, items: &[(f32, f32)], clear: &[Clear]) -> Option<f32> {
    const UNDER: f32 = 8.0;
    let (top, bottom) = row;
    let max = max.max(0.0);
    let shows = |o: f32| (-0.25..=max + 0.25).contains(&o) && top >= o + band - 0.5 && bottom - (ROW_PAD - 4.0) <= o + view + 0.5;
    if top >= at + band - 0.5 && bottom + UNDER <= at + view + 0.5 {
        return None;
    }
    let want = (if bottom + UNDER > at + view { bottom + UNDER - view } else { top - band }).clamp(0.0, max);
    let edge = want + band;
    let Some(i) = items.iter().position(|&(t, b)| t <= edge && edge < b) else { return Some(want) };
    let stretches_of = |j: usize| {
        let (t, b) = items[j];
        let c = clear.get(j).cloned().unwrap_or_default();
        c.from_top.iter().map(|&(s, e)| (t + s, t + e)).chain([(b - c.end, b)]).collect::<Vec<_>>()
    };
    // The gaps of the item the edge is on, and of those either side of it
    // (an edge near an item's end may do better in the next's top).
    let gaps: Vec<(f32, f32)> = (i.saturating_sub(1)..(i + 2).min(items.len())).flat_map(stretches_of).collect();
    // Where in each the edge is best: its middle, or higher, its ledge
    // `UNDER_LEDGE` over the line after it; none where it's too short.
    let best = |&(s, e): &(f32, f32)| {
        let room = e - 1.0 - UNDER_LEDGE;
        (room >= s).then(|| ((s + e) / 2.0).min(room))
    };
    if gaps.iter().any(|&(s, e)| s - 0.25 <= edge && edge <= e - 1.0 - UNDER_LEDGE + 0.25) {
        return Some(want);
    }
    let nearest = |edges: &mut dyn Iterator<Item = f32>| {
        let (mut up, mut down) = (f32::MIN, f32::MAX);
        for y in edges {
            if y < edge {
                up = up.max(y);
            } else if y > edge {
                down = down.min(y);
            }
        }
        match (shows(up - band), shows(down - band)) {
            (true, true) => Some(if want - (up - band) <= (down - band) - want { up - band } else { down - band }),
            (true, false) => Some(up - band),
            (false, true) => Some(down - band),
            (false, false) => None,
        }
    };
    if let Some(o) = nearest(&mut gaps.iter().filter_map(best)) {
        return Some(o);
    }
    // (No gap with room for that within reach: no line cut, at least.)
    let own = stretches_of(i);
    if own.iter().any(|&(s, e)| s - 0.25 <= edge && edge <= e + 0.25) {
        return Some(want);
    }
    Some(nearest(&mut own.iter().flat_map(|&(s, e)| [s, e])).unwrap_or(want))
}

fn under_rule(p: &olo_kit::palette::Palette, row: Element) -> Element {
    // (The groove's dark line by day, as it was. By night that is the
    // list's own darkness, 1.0 to 1, and it was the groove's light line
    // that showed: a step of the list's words into its ground.)
    let rule = if p.mode == olo_theme::ThemeMode::Dark { style::c(olo_theme::mix(p.list_background, p.list_item_text, 0.16)) } else { style::shade(p, p.panel_background, 1.2) };
    div().w_full().border_t_1().border_color(rule).child(row).into_element()
}

fn neutral_machine_icon(is_local: bool, size_pt: f32) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let body = if dark { style::c([0x8c, 0x9d, 0xb3, 0xff]) } else { style::c([0x5e, 0x6e, 0x84, 0xff]) };
        let top = if dark { style::c([0xb6, 0xc5, 0xd8, 0xff]) } else { style::c([0x84, 0x95, 0xab, 0xff]) };
        let edge = if dark { style::c([0x3b, 0x4b, 0x5c, 0xff]) } else { style::c([0x3b, 0x47, 0x56, 0xff]) };
        let slot = if dark { style::c([0x1d, 0x2c, 0x38, 0xff]) } else { style::c([0xea, 0xef, 0xf5, 0xff]) };
        // The icon's own square, on device pixels, in the middle of the
        // box it is given: a line's height (below), so that it stands on
        // the same middle as the words beside it at every scale. It was
        // a box of its own size (14 points: 17.5 px at 1.25x), centred
        // in the row by layout, and its shape was cut from whatever that
        // box rounded to: a pixel off its label one way or the other as
        // the row moved (two low at 1.25x, Opus 5.5, 2026-11-26).
        let r = style::snap_centred(g, b.center(), size_pt, size_pt);
        if is_local {
            // Crisp desktop tower centred in `r`
            let w = (f32::from(r.size.width) * 0.64).round().max(8.0);
            let h = (f32::from(r.size.height) * 0.84).round().max(10.0);
            let box_r = style::snap_centred(g, r.center(), w, h);
            let inner = style::inset(g, box_r, 1.0);
            style::fill_rect(g, inner, body);
            let cap = Bounds { origin: inner.origin, size: size(inner.size.width, px(2.0)) };
            style::fill_rect(g, cap, top);
            let bay = Bounds { origin: point(inner.left() + px(1.5), inner.top() + px(3.5)), size: size((inner.size.width - px(3.0)).max(px(2.0)), px(1.5)) };
            style::fill_rect(g, bay, slot);
            let led = style::snap_centred(g, point(inner.right() - px(2.5), inner.bottom() - px(2.5)), 2.0, 2.0);
            style::fill_rect(g, led, slot);
            style::frame_rect(g, box_r, edge);
        } else {
            // Two crisp stacked server blades (remote Rex host)
            let w = (f32::from(r.size.width) * 0.86).round().max(10.0);
            let bh = ((f32::from(r.size.height) * 0.36).round()).max(4.0);
            let x = (f32::from(r.center().x) - w * 0.5).round();
            let y1 = (f32::from(r.center().y) - bh - 1.0).round();
            let y2 = (f32::from(r.center().y) + 1.0).round();
            for y in [y1, y2] {
                let blade = style::snap(g, Bounds { origin: point(px(x), px(y)), size: size(px(w), px(bh)) });
                let inner = style::inset(g, blade, 1.0);
                style::fill_rect(g, inner, body);
                style::hline(g, inner.left(), inner.right(), inner.top(), top);
                let led = style::snap_centred(g, point(inner.left() + px(2.5), inner.center().y), 1.5, 1.5);
                style::fill_rect(g, led, slot);
                style::frame_rect(g, blade, edge);
            }
        }
    })
    .paint_key(style::key(("mach", is_local, size_pt.to_bits())))
    .w(px(size_pt))
    .h(px(DETAIL_ROW))
    .flex_shrink_0()
    .into_element()
}

/// Crisp 3/4-arc busy spinner glyph in neutral secondary ink matching 8pt status dots at 1×.
fn spinner_arc(across: f32) -> Element {
    canvas(move |g, b| {
        let p = theme::palette();
        let ink = secondary_ink(&p, p.list_background);
        let c = b.center();
        let rad = (across * 0.5 - 0.75).max(2.2);
        let mut path = olo_kit::PathBuilder::stroke(px(1.5));
        let steps = 18;
        let start = -std::f32::consts::FRAC_PI_2;
        let sweep = std::f32::consts::TAU * 0.72;
        for i in 0..=steps {
            let a = start + sweep * (i as f32 / steps as f32);
            let pt = point(c.x + px(rad * a.cos()), c.y + px(rad * a.sin()));
            if i == 0 {
                path.move_to(pt);
            } else {
                path.line_to(pt);
            }
        }
        if let Ok(built) = path.build() {
            g.paint_path(built, ink);
        }
    })
    .paint_key(style::key(("spinner", across.to_bits())))
    .w(px(across))
    .h(px(across))
    .flex_shrink_0()
    .into_element()
}

/// Left-anchored BeOS diagonal barber-pole indeterminate progress bar inside a sunken well.
fn barber_bar(width_pt: f32, height_pt: f32, color: Hsla) -> Element {
    let ck = style::ink_key(Some(color));
    canvas(move |g, b| {
        let p = theme::palette();
        let inner = style::inset(g, b, 2.0);
        let bg = style::c(p.document_background);
        style::fill_rect(g, inner, bg);
        g.with_content_mask(Some(olo_kit::ContentMask { bounds: inner }), |g| {
            let h = f32::from(inner.size.height);
            let w = f32::from(inner.size.width);
            let pitch = 10.0f32;
            let stripe_w = 5.0f32;
            let mut x = -h;
            while x < w + h {
                let mut poly = olo_kit::PathBuilder::fill();
                let p0 = point(inner.left() + px(x), inner.bottom());
                let p1 = point(inner.left() + px(x + stripe_w), inner.bottom());
                let p2 = point(inner.left() + px(x + stripe_w + h), inner.top());
                let p3 = point(inner.left() + px(x + h), inner.top());
                poly.add_polygon(&[p0, p1, p2, p3], true);
                if let Ok(built) = poly.build() {
                    g.paint_path(built, color);
                }
                x += pitch;
            }
        });
        style::sunken(g, b, &p, false);
    })
    .paint_key(style::key(("barber", width_pt.to_bits(), height_pt.to_bits(), ck)))
    .w(px(width_pt))
    .h(px(height_pt))
    .flex_shrink_0()
    .into_element()
}

/// Extract the actual loopback port from a bind/listen error message so the
/// Gateway Stopped banner and This Computer section always blame the exact
/// listener port that failed (17891 vs 17892).
fn error_port(raw_error: &str, fallback: u16) -> u16 {
    if let Some(rest) = raw_error.split("127.0.0.1:").nth(1) {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(p) = digits.parse::<u16>()
            && p != 0
        {
            return p;
        }
    }
    fallback
}

/// BeOS-native full-width Gateway Stopped alert rendered above the tab bar on every tab.
fn gateway_stopped_alert(p: &olo_kit::palette::Palette, port: u16, raw_error: &str, cx: &mut Cx<PoolWindow>) -> Element {
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let bg = if dark { style::c(olo_theme::mix(p.panel_background, p.failure, 0.14)) } else { style::c([0xfd, 0xec, 0xec, 0xff]) };
    let border = Hsla { a: 0.40, ..style::c(p.failure) };
    let blamed_port = error_port(raw_error, port);
    let body = if raw_error.to_ascii_lowercase().contains("address already in use") {
        format!("Another app is using port {blamed_port}, so Claude\u{a0}Code, Codex, and Antigravity can’t reach the pool.")
    } else {
        raw_error.to_owned()
    };
    div()
        .w_full()
        .p(px(16.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div().w_full().flex().flex_row().items_start().gap(px(10.0)).child(icon_box("stop", 32.0)).child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(StringView::new("Gateway stopped").bold(true).height(m::LABEL_ROW))
                    .child(StringView::new(body).size(style::SMALL_SIZE).wrap(true)),
            ),
        )
        .child(
            widgets::button_row()
                .w_full()
                .h(px(m::CONTROL_HEIGHT))
                .child(Button::new("Change Port…").id("pool-change-port").on_click(|_: &ClickEvent, _| olo_kit::app_menu::open_settings("ai")))
                .child(Button::new("Try Again").id("pool-retry-gateway").default(true).on_click(cx.listener(|v, _: &ClickEvent, _| {
                    if !demo_enabled() {
                        v.shared.sync_soon(None);
                    }
                }))),
        )
        .into_element()
}

/// Warning-tinted inline confirmation box matching BeOS alert conventions:
/// 32pt native `warning` icon, bold primary question, clean explanation, and
/// right-aligned buttons below (never overlapping safe action buttons).
fn warning_confirm(p: &olo_kit::palette::Palette, question: String, consequence: String, reassurance: Option<String>, cancel_btn: Button, confirm_btn: Button) -> Element {
    let (bg, border) = if p.mode == olo_theme::ThemeMode::Dark {
        (style::c(olo_theme::mix(p.list_background, p.panel_background, 0.85)), style::c([0xd4, 0x9b, 0x24, 0xff]))
    } else {
        (style::c([0xff, 0xf4, 0xcc, 0xff]), style::c([0xe2, 0xc2, 0x5a, 0xff]))
    };
    let mut copy =
        div().flex_1().min_w_0().flex().flex_col().gap(px(6.0)).child(StringView::new(question).bold(true).wrap(true)).child(StringView::new(consequence).size(style::SMALL_SIZE).wrap(true));
    if let Some(extra) = reassurance {
        copy = copy.child(StringView::new(extra).size(style::SMALL_SIZE).color(secondary_ink(p, p.panel_background)).wrap(true));
    }
    div()
        .w_full()
        .p(px(10.0))
        .bg(bg)
        .border_1()
        .border_color(border)
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(div().w_full().flex().flex_row().items_start().gap(px(10.0)).child(icon_box("warning", 32.0)).child(copy))
        .child(widgets::button_row().w_full().h(px(m::CONTROL_HEIGHT)).child(cancel_btn).child(confirm_btn))
        .into_element()
}

/// Neutral slate fill for quota and indeterminate progress bars so provider identity
/// colours (Claude orange, Codex blue) never collide with generic meters.
fn neutral_bar_color(p: &olo_kit::palette::Palette) -> Hsla {
    style::neutral_bar(p)
}

/// Compute ONE shared quota-label column width across every account in the list
/// so every meter bar in Claude Code, Codex, and Antigravity starts on one exact
/// vertical axis and no scoped bucket label ("Gemini Pro", "Weekly") ever truncates.
fn shared_quota_label_width(accounts: &[Account], narrow: bool) -> f32 {
    let font = style::ui_font(FontWeight::NORMAL);
    let max_w = accounts
        .iter()
        .flat_map(|a| a.windows.iter())
        .map(|w| {
            let l = if w.label.is_empty() { plan_name(w.name.trim_end_matches("_window")) } else { w.label.clone() };
            f32::from(style::text_width(l, style::SMALL_SIZE, font.clone()))
        })
        .fold(0.0f32, f32::max);
    let max_cap = if narrow { 92.0 } else { 124.0 };
    // (Narrow, the longest a gap's width from its bar, as every column
    // is: its bars as wide as they can be, about 100 points in a third,
    // not 61; Opus's review, 2026-11-28.)
    let slack = if narrow { QUOTA_GAP_NARROW } else { 12.0 };
    (max_w + slack).ceil().clamp(48.0, max_cap)
}

/// The gap between a quota line's columns, narrow (else 8).
const QUOTA_GAP_NARROW: f32 = 6.0;

/// A quota's reset, as its line says it: when (and how long until),
/// narrow how long only; "not started" where unused; else nothing.
fn quota_reset_text(w: &QuotaWindow, t: i64, narrow: bool) -> String {
    if let Some(r) = w.resets_at.filter(|&r| r > t) {
        if narrow { span(r - t) } else { format!("resets {} ({})", clock(r), span(r - t)) }
    } else if w.used <= 0.0 {
        "not started".to_owned()
    } else {
        String::new()
    }
}

/// The resets' column, one for every account as the labels' is
/// ([`shared_quota_label_width`]): narrow, as wide as the widest reset
/// it shows (62 points held "not started" where none did); else the
/// whole "resets Thu 00:09 (2h 10m)".
fn shared_quota_reset_width(accounts: &[Account], t: i64, narrow: bool) -> f32 {
    if !narrow {
        return 158.0;
    }
    let font = style::ui_font(FontWeight::NORMAL);
    let widest = accounts.iter().flat_map(|a| a.windows.iter()).map(|w| f32::from(style::text_width(quota_reset_text(w, t, true), style::SMALL_SIZE, font.clone()))).fold(0.0f32, f32::max);
    widest.ceil().clamp(36.0, 62.0)
}

/// An account's facts (its address, its plan, its banked resets), " · "
/// between: all of them where they fit `room` (measured by `width`);
/// else as many whole ones from the first as fit, "…" after them; else
/// all (the first then cut where a word ends). Never a fact cut in two:
/// "· 2 banked re…" in a third (Neon's review, 2026-11-28; CLAUDE.md,
/// whole trailing segments dropped, not cut).
fn facts_shown(facts: &[String], room: f32, width: impl Fn(&str) -> f32) -> String {
    let all = facts.join(" · ");
    if width(&all) <= room {
        return all;
    }
    (1..facts.len()).rev().map(|n| format!("{}…", facts[..n].join(" · "))).find(|t| width(t) <= room).unwrap_or(all)
}

/// [`facts_shown`] on a line of their own, in `ink`, as wide as it's
/// given; whole to agents, and in a tip (`key`) where some are left off
/// in the `room` it's laid out for.
fn facts_line(key: String, facts: Vec<String>, room: f32, ink: Hsla) -> Element {
    let all = SharedString::from(facts.join(" · "));
    let measure = |t: &str| f32::from(style::text_width(t.to_owned(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL)));
    let left_off = facts_shown(&facts, room, measure) != all.as_ref();
    let paint_key = style::key((&facts, style::ink_key(Some(ink))));
    let whole = all.clone();
    let line = canvas(move |g, b| {
        let font = style::ui_font(FontWeight::NORMAL);
        let shown = facts_shown(&facts, f32::from(b.size.width), |t| f32::from(style::text_width(t.to_owned(), style::SMALL_SIZE, font.clone())));
        let at = style::label_fit_words(g, b.left(), b.center().y, style::Align::Left, shown, style::SMALL_SIZE, font, ink, b.size.width);
        g.agent(olo_kit::agent::Node::label(whole.clone(), at));
    })
    .paint_key(paint_key)
    .w_full()
    .min_w_0()
    .h(px(DETAIL_ROW))
    .into_element();
    if left_off { olo_kit::tip::on(line, key, all) } else { line }
}

/// Compact inline single-row quota meter with fixed right-aligned percentage column
/// and left-aligned reset text (`5-hour | [====] |  42%  resets Thu 00:09 (2h 10m)`).
/// Bar colour uses neutral slate (<80%) or warning amber (≥80% / exhausted); failure red
/// is reserved strictly for actionable errors (`Signed out`, `failed`, `Can't sync`).
fn inline_quota_row(p: &olo_kit::palette::Palette, w: &QuotaWindow, t: i64, (label_w, reset_w): (f32, f32), narrow: bool) -> Element {
    let label = if w.label.is_empty() { plan_name(w.name.trim_end_matches("_window")) } else { w.label.clone() };
    let pct = w.used.clamp(0.0, 999.0);
    let pct_text = format!("{pct:.0}%");
    let reset_text = quota_reset_text(w, t, narrow);
    let (bar_color, pct_color) = if w.exhausted(t) || pct >= 80.0 { (style::c([0xe3, 0xa0, 0x08, 0xff]), style::c(p.warning)) } else { (neutral_bar_color(p), style::c(p.panel_text)) };
    let fraction = (w.used / 100.0) as f32;
    let bar_key = style::key((fraction.to_bits(), style::ink_key(Some(bar_color))));
    let bar_el = canvas(move |g, b| {
        let pal = theme::palette();
        level_bar(g, b, fraction, Some(bar_color), &pal);
    })
    .paint_key(bar_key)
    .flex_1()
    .min_w(px(60.0))
    .max_w(px(280.0))
    .h(px(10.0));

    let sec = secondary_ink(p, p.list_background);
    div()
        .w_full()
        .h(px(18.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(if narrow { QUOTA_GAP_NARROW } else { 8.0 }))
        .child(div().w(px(label_w)).flex_shrink_0().child(StringView::new(label).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)))
        .child(bar_el)
        .child(div().w(px(32.0)).flex_shrink_0().flex().justify_end().child(StringView::new(pct_text).size(style::SMALL_SIZE).color(pct_color).height(DETAIL_ROW)))
        .child(div().w(px(reset_w)).flex_shrink_0().child(StringView::new(reset_text).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)))
        .into_element()
}

#[derive(Clone, PartialEq)]
pub enum Pick {
    Live(String),
    Recent(RecentReq),
}

#[derive(Clone)]
pub enum WinMsg {
    Changed,
    Told(Told),
    Tick,
    /// A second of a sign-in's wait gone (`PoolWindow::count_down`).
    Countdown,
    HostsTick,
    Tab(Tab),
    SelectAccount(String),
    SelectHost(String),
    ViewHostActivity(String),
    ScopeIndex(usize),
    Span(usize),
    Pick(Pick),
    ClearPick,
    AddMenu(Bounds<Pixels>),
    AccountMenu(String, Bounds<Pixels>),
    AccountContextMenu(String, olo_kit::Point<Pixels>),
    HostContextMenu(String, olo_kit::Point<Pixels>),
    Account(String, String),
    Renamed(bool),
    RemoveAccount(Option<String>),
    ResetCredit(Option<String>),
    FoundRunning(Box<Running>),
    RestartCodex,
    CodexRestarted(Result<i32, String>),
    HostDo(String, &'static str),
    HostAskRemove(Option<String>),
    SyncAllHosts,
}

/// The account just signed in, being brought into view: asked for, then
/// placed by a layout (its row's index and the list's length then, and
/// when). Kept by its id until a layout that still has it where it was
/// shows it: one that moved it (a quota read, an account more, the window
/// put in a tile as it opened) is waited out, not given up on (Astra's
/// review, 2026-11-28), for `REVEAL_FOR` from its first placing.
enum Reveal {
    Asked(String),
    Placed(String, (usize, usize), Instant),
}

/// How long the account just signed in is kept in view while the window
/// settles (it may open at one size and be put in a tile of another).
const REVEAL_FOR: Duration = Duration::from_millis(1500);

pub struct PoolWindow {
    /// The gateway's "stopped" box over the tabs, as tall as it was last
    /// laid out (0 before it is): what Activity has less of its page.
    alert_h: f32,
    shared: Arc<Shared>,
    hosts: Arc<Hosts>,
    proxy_port: u16,
    told: Told,
    tab: Tab,
    // Accounts tab
    accounts_scroll: ScrollHandle,
    selected_account: Option<String>,
    /// The account just signed in, being brought into view (`Reveal`).
    reveal: RefCell<Option<Reveal>>,
    /// Where the footer's note was last drawn (`footer_note`): the tip
    /// over a cut one is made from it.
    note_drawn: Rc<Cell<Drawn>>,
    /// The wait's countdown is ticking (once a second, only while a
    /// sign-in is waited for: nothing while idle).
    counting: bool,
    /// Its opening size looked at, against the screen's room.
    sized: bool,
    renaming: Option<(String, Child<TextField>)>,
    removing_account: Option<String>,
    resetting_credit: Option<String>,
    // Activity tab
    scope: MachineScope,
    span_idx: usize,
    activity_picked: Option<Pick>,
    activity_scroll: ScrollHandle,
    // Machines tab
    hosts_scroll: ScrollHandle,
    selected_host: Option<String>,
    running: Option<Running>,
    scanning: Option<Arc<AtomicBool>>,
    restarting_codex: bool,
    codex_note: Option<(String, bool)>,
    host_asking_remove: Option<String>,
    host_refused: Option<(String, String)>,
}

impl PoolWindow {
    fn new(shared: Arc<Shared>, hosts: Arc<Hosts>, proxy_port: u16, told: Told, tab: Tab, cx: &mut Cx<Self>) -> PoolWindow {
        if !demo_enabled() {
            hosts.refresh();
            cx.after(HOSTS_POLL_EVERY, WinMsg::HostsTick);
        }
        // (An account signed in as it opened, shown: the demo's.)
        let joined = told.joined.as_ref().map(|(id, _)| id.clone());
        let mut w = PoolWindow {
            shared,
            hosts,
            proxy_port,
            told,
            tab,
            accounts_scroll: ScrollHandle::new(),
            selected_account: joined.clone(),
            reveal: RefCell::new(joined.map(Reveal::Asked)),
            note_drawn: Rc::default(),
            counting: false,
            sized: false,
            renaming: None,
            removing_account: None,
            resetting_credit: None,
            scope: MachineScope::All,
            span_idx: 0,
            activity_picked: None,
            activity_scroll: ScrollHandle::new(),
            hosts_scroll: ScrollHandle::new(),
            alert_h: 0.0,
            selected_host: None,
            running: None,
            scanning: None,
            restarting_codex: false,
            codex_note: None,
            host_asking_remove: None,
            host_refused: None,
        };
        if tab == Tab::Machines {
            w.start_scanner(cx);
        }
        w.schedule_tick(cx);
        w.count_down(cx);
        w
    }

    /// The wait's countdown ticked on, a second from now, while a sign-in
    /// is waited for (the demo's too); none otherwise.
    fn count_down(&mut self, cx: &mut Cx<Self>) {
        if self.countdown_shown() && !self.counting {
            self.counting = true;
            cx.after(Duration::from_secs(1), WinMsg::Countdown);
        }
    }

    /// Whether the wait's countdown is in sight: on the Accounts tab, no
    /// other note over it. Ticked on only then (Astra's review, 2026-11-28:
    /// each second redrew a tab that doesn't show it), and from the wait's
    /// own end when it's back in sight.
    fn countdown_shown(&self) -> bool {
        self.tab == Tab::Accounts && self.told.note.is_none() && self.told.waiting.is_some()
    }

    fn start_scanner(&mut self, cx: &mut Cx<Self>) {
        if demo_enabled() || self.scanning.is_some() {
            return;
        }
        let flag = Arc::new(AtomicBool::new(true));
        self.scanning = Some(flag.clone());
        let (port, tx) = (self.proxy_port, cx.messenger());
        let _ = std::thread::Builder::new().name("pool-clients-scan".into()).spawn(move || {
            while flag.load(Ordering::Relaxed) {
                tx.send(WinMsg::FoundRunning(Box::new(olo_token_pool::running::scan(port))));
                for _ in 0..10 {
                    if !flag.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(PROCESS_POLL_EVERY / 10);
                }
            }
        });
    }

    fn stop_scanner(&mut self) {
        if let Some(flag) = self.scanning.take() {
            flag.store(false, Ordering::Relaxed);
        }
    }

    fn switch_tab(&mut self, tab: Tab, cx: &mut Cx<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab = tab;
        if tab == Tab::Machines {
            self.start_scanner(cx);
        } else {
            self.stop_scanner();
        }
        self.count_down(cx);
        olo_kit::proxy().send(Msg::TabChanged(tab));
    }

    fn schedule_tick(&self, cx: &mut Cx<Self>) {
        if demo_enabled() {
            return;
        }
        let busy = !self.shared.snapshot().open.is_empty() || self.hosts.list().iter().any(|h| h.status.as_ref().and_then(|s| s["open"].as_array()).is_some_and(|o| !o.is_empty()));
        cx.after(Duration::from_secs(if busy { 1 } else { 20 }), WinMsg::Tick);
    }

    #[allow(clippy::type_complexity)]
    fn snapshot(&self) -> (Pool, Activity, Vec<Stands>, Option<Running>, Option<[bool; 3]>, HashMap<String, Vec<String>>) {
        if demo_enabled() {
            let d = demo_data();
            return (d.pool, d.activity, d.hosts, Some(d.running), Some(d.uses), d.unaccepted);
        }
        let pool = self.shared.snapshot();
        let activity = self.shared.activity(Clone::clone);
        let hosts = self.hosts.list();
        let mut unaccepted = HashMap::new();
        for h in &hosts {
            let u = self.shared.unaccepted_handovers(&h.id);
            if !u.is_empty() {
                unaccepted.insert(h.id.clone(), u);
            }
        }
        (pool, activity, hosts, self.running.clone(), self.told.clients, unaccepted)
    }

    fn menus(&self) -> Vec<Menu> {
        let hosts = if demo_enabled() { demo_data().hosts } else { self.hosts.list() };
        let sorted = sorted_hosts(&hosts);
        let has_wanted_host = sorted.iter().any(|h| h.wanted);
        let sel_host = self.selected_host.as_deref().or_else(|| sorted.first().map(|h| h.id.as_str()));
        let can_show_act = sel_host.and_then(|id| sorted.iter().find(|h| h.id == id)).is_some_and(|h| h.wanted || h.status.is_some());
        let has_daemon = if demo_enabled() { demo_data().running.daemon.is_some() } else { self.running.as_ref().and_then(|r| r.daemon.as_ref()).is_some() };
        vec![
            Menu::new("Pool").app_items(&["ai", "rex", "deskbar"]).item("Refresh Quota", "refresh").key("r").item("Reconnect Local Clients", "capture").close_item(),
            Menu::new("View")
                .item("Accounts", "tab-accounts")
                .key("1")
                .checked(self.tab == Tab::Accounts)
                .item("Activity", "tab-activity")
                .key("2")
                .checked(self.tab == Tab::Activity)
                .item("Machines", "tab-machines")
                .key("3")
                .checked(self.tab == Tab::Machines),
            add_menu(self.told.waiting.is_some()),
            Menu::new("Machines")
                .item("Add Host…", "add-host")
                .item("Show Activity", "show-host-activity")
                .enabled(can_show_act)
                .item("Sync All", "sync-hosts")
                .shift_key("s")
                .enabled(has_wanted_host)
                .separator()
                .item("Restart Codex Service", "restart-codex")
                .enabled(has_daemon),
        ]
    }

    fn scope_options(hosts: &[Stands]) -> Vec<(MachineScope, SharedString)> {
        let mut opts = vec![(MachineScope::All, olo_kit::words::tr("All Machines").into()), (MachineScope::Local, olo_kit::words::tr("This Computer").into())];
        for h in hosts {
            if h.wanted || h.status.is_some() {
                opts.push((MachineScope::Host(h.id.clone()), h.name.clone().into()));
            }
        }
        opts
    }

    fn effective_port(&self) -> u16 {
        let bound = self.shared.bound_port();
        if bound != 0 {
            bound
        } else if self.proxy_port != 0 {
            self.proxy_port
        } else {
            olo_token_pool::intercept::PORT
        }
    }

    /// An account's menu; its sign-ins not begun while another is
    /// `waiting` ([`add_menu`]).
    fn account_menu_items(a: &Account, waiting: bool) -> Menu {
        Menu::new("Account")
            .item("Rename…", "rename")
            .item("Refresh Quota", "refresh")
            .item("Claim Banked Reset…", "reset")
            .enabled(a.credit().is_some())
            .separator()
            .item("Sign In Again in Terminal…", "sign-in")
            .enabled(!waiting)
            .item(format!("Use This Computer’s {} Sign-In", a.provider.client()), "sign-in-local")
            .enabled(!waiting)
            .separator()
            .item("Remove from Pool…", "remove")
    }

    fn account_command(&mut self, id: &str, command: &str, cx: &mut Cx<Self>) {
        let pool = if demo_enabled() { demo_data().pool } else { self.shared.snapshot() };
        let Some(a) = pool.account(id).cloned() else { return };
        self.selected_account = Some(a.id.clone());
        match command {
            "rename" => {
                let field = text_field(cx, |f| f.name("Name").with_text(a.name.clone()));
                field.update(cx, |f, cx| f.caret_to_end(cx));
                field.focus(cx);
                cx.subscribe(&field, |_, e: &TextFieldEvent, cx| {
                    if matches!(e, TextFieldEvent::Entered) {
                        cx.send(WinMsg::Renamed(true));
                    }
                });
                self.renaming = Some((a.id, field));
            }
            "refresh" => {
                if !demo_enabled() {
                    self.shared.sync_soon(Some(a.id));
                }
            }
            "sign-in" => olo_kit::proxy().send(Msg::SignIn(a.provider, a.id, false)),
            "sign-in-local" => olo_kit::proxy().send(Msg::SignIn(a.provider, a.id, true)),
            "remove" => self.removing_account = Some(a.id),
            "reset" => self.resetting_credit = Some(a.id),
            _ => {}
        }
    }

    // ---- Tab 1: Accounts --------------------------------------------------

    fn accounts_tab(&self, pool: &Pool, hosts: &[Stands], unaccepted: &HashMap<String, Vec<String>>, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let t = now();
        let vp = cx.viewport_size();
        let narrow = f32::from(vp.width) < 480.0;
        let compact_h = f32::from(vp.height) < 420.0;

        // ONE sunken framed list well for all provider groups & account rows.
        // Sections stay in fixed canonical order (Claude Code, Codex, Antigravity);
        // problems float to the top within each section.
        let providers = sorted_providers(&pool.saved.accounts, t);
        let first_account_id = providers.first().and_then(|&prov| sorted_accounts(&pool.saved.accounts, prov, t).first().map(|a| a.id.as_str()));
        let sel_id = self.selected_account.as_deref().filter(|id| pool.account(id).is_some()).or(first_account_id);
        let wide = ["Sign In…", "Retry Now", "Sync Now", "Install", "Try Again"].iter().map(|l| button_width(l, false)).fold(m::BUTTON_MIN_WIDTH, f32::max);
        let shared_label_w = (shared_quota_label_width(&pool.saved.accounts, narrow), shared_quota_reset_width(&pool.saved.accounts, t, narrow));

        let group_bg = style::c(p.panel_background);
        let make_group_hdr = |prov: Provider, after_another: bool| {
            div()
                .w_full()
                .h(px(22.0))
                .px(px(10.0))
                .bg(group_bg)
                .border_b_1()
                .border_t(px(if after_another { 1.0 } else { 0.0 }))
                .border_color(style::c(p.control_border))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(div().w(px(13.0)).flex_shrink_0().flex().justify_center().child(swatch_dot(provider_color(p, prov), 8.0)))
                .child(StringView::new(prov.client()).size(style::SMALL_SIZE).bold(true).height(DETAIL_ROW))
                .into_element()
        };

        let mut items: Vec<Element> = Vec::new();
        // Where the pinned band can end on each item without cutting a line
        // (`row_cuts`; a band, its top and its end): while a row's being
        // brought into view, and not otherwise (Astra's review, 2026-11-28:
        // a list of them each rebuild).
        let revealing = self.reveal.borrow().is_some();
        let mut cuts: Vec<Clear> = Vec::new();
        let mut group_indices: Vec<(Provider, usize)> = Vec::new();
        let mut sel_ix = None;
        let mut any = false;
        for prov in providers {
            let accounts = sorted_accounts(&pool.saved.accounts, prov, t);
            if accounts.is_empty() {
                continue;
            }
            // (A group after another has its band's own top edge for the
            // rule between them, as a row has: the last row's tint runs
            // to it. A separator of the kit's stood here too.)
            let after_another = any;
            any = true;
            group_indices.push((prov, items.len()));
            items.push(make_group_hdr(prov, after_another));
            if revealing {
                cuts.push(Clear { from_top: vec![(0.0, 0.0)], end: 0.0 });
            }
            for (idx, account) in accounts.iter().enumerate() {
                let pending = account.kept_on.as_deref().and_then(|hid| unaccepted.get(hid)).map(Vec::as_slice).unwrap_or(&[]);
                let rollup = account_rollup(pool, account, hosts);
                let is_sel = sel_id == Some(account.id.as_str());
                if is_sel {
                    sel_ix = Some(items.len());
                }
                let (row, row_cuts) = self.account_row(account, hosts, pending, &rollup, is_sel, wide, shared_label_w, narrow, revealing, p, cx);
                // (A row after another has its rule over it: its lines a
                // point lower.)
                let rule = if idx > 0 { 1.0 } else { 0.0 };
                if let Some(row_cuts) = row_cuts {
                    cuts.push(Clear { from_top: row_cuts.from_top.iter().map(|&(s, e)| (if s > 0.0 { s + rule } else { s }, e + rule)).collect(), end: row_cuts.end });
                }
                items.push(if idx > 0 { under_rule(p, row) } else { row });
            }
        }
        if !any {
            let top_pad = if compact_h { 24.0 } else { 168.0 };
            let empty_box = if pool.loaded {
                div()
                    .w_full()
                    .pt(px(top_pad))
                    .pb(px(24.0))
                    .px(px(24.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(icon_box("token-pool", 48.0))
                    .child(div().pt(px(8.0)).child(StringView::new("No accounts yet").bold(true).height(m::LABEL_ROW)))
                    .child(
                        div().pt(px(4.0)).flex().flex_col().items_center().child(
                            StringView::new("Click Add Account… to add a Claude\u{a0}Code, Codex, or Antigravity subscription.")
                                .size(style::SMALL_SIZE)
                                .color(secondary_ink(p, p.list_background))
                                .wrap(true),
                        ),
                    )
            } else {
                div().w_full().pt(px(top_pad)).pb(px(24.0)).px(px(24.0)).flex().justify_center().child(StringView::new("Opening the pool’s accounts…").color(secondary_ink(p, p.list_background)))
            };
            items.push(empty_box.into_element());
            if revealing {
                cuts.push(Clear::default());
            }
        }

        // The account just signed in, brought into view: once a layout has
        // placed its row (a new one's place is known only then), clear of
        // the group band that sticks at the list's top and 8 points above
        // its bottom, the band's edge between two lines (`reveal_offset`).
        {
            let now = sel_ix.map(|ix| (ix, items.len()));
            let mut reveal = self.reveal.borrow_mut();
            *reveal = match reveal.take() {
                // (Chosen otherwise meanwhile: left where it is. Not in the
                // list yet: looked for again next time.)
                Some(Reveal::Asked(id) | Reveal::Placed(id, ..)) if sel_id != Some(id.as_str()) => pool.account(&id).is_none().then_some(Reveal::Asked(id)),
                // Shown as its last layout placed it: then looked at again
                // as the next one does, while the window settles.
                Some(Reveal::Placed(id, at, t)) if now == Some(at) => {
                    let moved = self.reveal_row(at.0, &cuts, cx);
                    (moved && t.elapsed() < REVEAL_FOR).then(|| {
                        cx.invalidate_at_frame();
                        Reveal::Placed(id, at, t)
                    })
                }
                // Placed by the next layout; laid out again since (moved,
                // or the list changed), placed again.
                Some(Reveal::Asked(id)) => {
                    cx.invalidate_at_frame();
                    now.map(|at| Reveal::Placed(id, at, Instant::now()))
                }
                Some(Reveal::Placed(id, _, t)) if t.elapsed() < REVEAL_FOR => {
                    cx.invalidate_at_frame();
                    now.map(|at| Reveal::Placed(id, at, t))
                }
                _ => None,
            };
        }

        // Sticky active provider header so scrolling past a section banner never
        // leaves orphaned account/Today rows without provider context.
        let active_group = |handle: &ScrollHandle, groups: &[(Provider, usize)]| -> Option<(Provider, usize)> {
            let y = -handle.offset().y;
            let mut found = None;
            for &(prov, idx) in groups {
                if let Some(b) = handle.bounds_for_item(idx)
                    && y > b.top()
                {
                    found = Some((prov, idx));
                }
            }
            found
        };
        let stuck = active_group(&self.accounts_scroll, &group_indices);
        // Rows scrolled under the pinned band: a line of the bevel's shadow
        // under its rule, so they read as passing under a ledge, not cut
        // (Opus's review, 2026-11-28).
        let under = |handle: &ScrollHandle| f32::from(handle.offset().y) < -0.5;
        let scrolled = under(&self.accounts_scroll);
        let mut sv = ScrollView::new(&self.accounts_scroll).background(p.list_background).children(items);
        if let Some(&(first_prov, first_idx)) = group_indices.first() {
            let (sticky_prov, sticky_idx) = stuck.unwrap_or((first_prov, first_idx));
            let band = make_group_hdr(sticky_prov, false);
            sv = if scrolled {
                // (By day the well's inner top edge, its bevel's shadow:
                // near black, it was the heaviest line in the window, a
                // rule more than a ledge; Opus's review, 2026-11-28.)
                let ledge = if p.mode == olo_theme::ThemeMode::Dark { style::c(p.shadow) } else { style::tint(p.panel_background, 1.55) };
                sv.sticky(sticky_idx, BAND + 1.0, div().w_full().flex().flex_col().child(band).child(div().w_full().h(px(1.0)).bg(ledge)).into_element())
            } else {
                sv.sticky(sticky_idx, BAND, band)
            };
            let handle = self.accounts_scroll.clone();
            let groups = group_indices;
            sv = sv.on_scroll(move |_, cx| {
                if active_group(&handle, &groups) != stuck || under(&handle) != scrolled {
                    cx.invalidate_owner();
                }
            });
        }
        let well = div().flex_1().min_h_0().child(sv);

        // The footer, as Machines' is: [Add Account…] [Edit Account…] note …
        // Updated HH:MM [Refresh Quota]. While a sign-in is waited for,
        // Cancel Sign-In stands where Add Account… does, as wide, so nothing
        // moves and Edit Account… stays, at every width; Add Account… comes
        // back as it ends (Opus's and Neon's reviews, 2026-11-28). What was
        // just done (or went wrong) is said, else the sign-in waited for, its
        // time counting down: a note after the buttons, from the left. Else
        // when quota was read, by Refresh Quota.
        let note: Option<Note> = self.told.note.clone().or_else(|| self.told.waiting.as_ref().map(|(provider, _, until)| Note::waiting(*provider, &left(*until, Instant::now()))));
        let sel_owned = sel_id.map(str::to_owned);
        let can_edit = sel_owned.is_some();
        let has_accounts = !pool.saved.accounts.is_empty();
        let edit_id = sel_owned.unwrap_or_default();

        let slot = button_width("Add Account…", false).max(button_width("Cancel Sign-In", false));
        let first = if self.told.waiting.is_some() {
            Button::new("Cancel Sign-In").id("pool-cancel-sign-in").min_width(slot).on_click(|_: &ClickEvent, _| olo_kit::proxy().send(Msg::CancelSignIn))
        } else {
            Button::new("Add Account…").id("pool-add-account").min_width(slot).default(pool.loaded && !has_accounts).on_click(cx.listener(|_, _: &ClickEvent, cx| {
                let at = cx.listener_bounds();
                cx.send(WinMsg::AddMenu(at));
            }))
        };
        let edit = Button::new("Edit Account…").id("pool-account-details").enabled(can_edit).on_click(cx.listener(move |_, _: &ClickEvent, cx| {
            if !edit_id.is_empty() {
                let at = cx.listener_bounds();
                cx.send(WinMsg::AccountMenu(edit_id.clone(), at));
            }
        }));
        let two_rows = footer_wraps(&["Add Account…", "Edit Account…", "Refresh Quota"], slot, m::SPACING, page_width(vp));
        // (The note's row of its own only where the buttons are one row
        // and the window is tall enough for it: in a short window (a
        // quarter) it took the list's room, and over two rows of buttons
        // (a third) it made three, a staircase. There the note, or when
        // quota was read, is on the row with Refresh Quota, as Machines'
        // time is with Sync All: Opus's and Neon's reviews, 2026-11-28.)
        let note_row = f32::from(vp.width) < NOTE_ROW && !two_rows && !compact_h;
        // (The time where it fits whole: before Refresh Quota, after the
        // others on its row.)
        let lead_w = if two_rows { 0.0 } else { slot + m::SPACING + button_width("Edit Account…", false) + m::SPACING };
        let time_room = page_width(vp) - lead_w - button_width("Refresh Quota", false) - m::SPACING - NOTE_GAP;
        let note_el = note.as_ref().map(|n| {
            let place = if note_row { NotePlace::Row } else { NotePlace::Beside };
            let invalidator = cx.invalidator();
            self.note_tip(n, place, footer_note(n, note_ink(p, n.error), place, self.note_drawn.clone(), move || invalidator.invalidate()))
        });
        let updated = last_synced_at(pool).map(|at| format!("Updated {}", clock(at)));
        let (status_box, row_note) = match note_el {
            // (Too narrow for it beside the buttons: a row of its own, just
            // above them.)
            Some(el) if note_row => (div().flex_1().min_w_0(), Some(el)),
            // (On Refresh Quota's row, alone: from the page's edge, ending
            // as far from the button as the time does.)
            Some(el) if two_rows => (div().flex_1().min_w_0().pr(px(NOTE_GAP)).child(el), None),
            Some(el) => (div().flex_1().min_w_0().pl(px(NOTE_GAP)).child(el), None),
            // (That row there with no note too: when quota was read, in a
            // note's place, from the left.)
            None if note_row => {
                let el = match updated {
                    Some(text) => footer_note(&(text, false).into(), note_ink(p, false), NotePlace::Row, Rc::default(), || {}),
                    None => div().w_full().h(px(NOTE_ROW_BASELINE + NOTE_ROW_UNDER)).into_element(),
                };
                (div().flex_1().min_w_0(), Some(el))
            }
            None => {
                let mut b = div().flex_1().min_w_0().pr(px(NOTE_GAP));
                let fits = |t: &String| f32::from(style::text_width(t.clone(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))) <= time_room;
                if let Some(text) = updated.filter(fits) {
                    b = b.child(footer_time(text, note_ink(p, false)));
                }
                (b, None)
            }
        };
        let refresh = Button::new("Refresh Quota").id("pool-refresh").enabled(has_accounts).on_click(cx.listener(|v, _: &ClickEvent, _| {
            if !demo_enabled() {
                v.shared.sync_soon(None);
                v.hosts.refresh();
            }
        }));
        let footer = footer_rows(vec![first.into_element(), edit.into_element()], status_box.into_element(), refresh.into_element(), m::SPACING, two_rows);

        match row_note {
            // (Its row set to its buttons: 10 from the list to its
            // capitals, `NOTE_ROW_UNDER` from its baseline to them.)
            Some(el) => div().flex_1().min_h_0().flex().flex_col().child(well).child(el).child(footer).into_element(),
            None => div().flex_1().min_h_0().flex().flex_col().gap(px(8.0)).child(well).child(footer).into_element(),
        }
    }

    /// The list brought to show the item at `ix` whole ([`reveal_offset`]),
    /// from what its last layout placed (each item's `cuts`): whether it
    /// was moved (not where it stands already).
    fn reveal_row(&self, ix: usize, cuts: &[Clear], cx: &mut Cx<Self>) -> bool {
        let h = &self.accounts_scroll;
        let Some(row) = h.bounds_for_item(ix) else { return false };
        let items: Vec<(f32, f32)> = (0..cuts.len()).map_while(|i| h.bounds_for_item(i)).map(|b| (f32::from(b.top()), f32::from(b.bottom()))).collect();
        let (at, view, max) = (-f32::from(h.offset().y), f32::from(h.bounds().size.height), f32::from(h.max_offset().height));
        match reveal_offset((f32::from(row.top()), f32::from(row.bottom())), at, view, max, BAND, &items, cuts) {
            Some(o) if (o - at).abs() >= 0.5 => {
                h.set_offset(cx, olo_kit::point(px(0.0), px(-o)));
                true
            }
            _ => false,
        }
    }

    /// `el`, the footer's note (`place`d), with its whole in a tip above
    /// it where it was cut (`note_drawn`): its words where the note's
    /// begin, at every width; its end where the list's is (and Refresh
    /// Quota's) where it would end past there, or over the list's scroll
    /// bar (a sliver of the bar's arrow showed beside it); its foot 4
    /// points over the list's, off the window's frame and the buttons
    /// (Opus's and Neon's reviews, 2026-11-28: at 400 it began a third of
    /// the way across, under the pointer, and at 640 it ended a point
    /// under the list's foot).
    fn note_tip(&self, n: &Note, place: NotePlace, el: Element) -> Element {
        let drawn = self.note_drawn.get();
        if !drawn.cut {
            return el;
        }
        let tip = tip_width(&n.text);
        // The list's right, where Refresh Quota's is: its scroller's, and
        // its bar beside that.
        let bar = olo_kit::widgets::SCROLL_BAR;
        let end = f32::from(self.accounts_scroll.bounds().right()) + bar;
        // (The kit's tip stands 4 points over its place: that 4 points
        // over the list's foot, the footer's 8 above a note beside the
        // buttons, the note's own top on its row.)
        let above = match place {
            NotePlace::Beside => 8.0,
            NotePlace::Row => 0.0,
        };
        let reach = if end > bar && drawn.left - TIP_PAD + tip > end - bar {
            // (Too wide to begin there, or ending over the bar: it ends
            // where the list does. Its place the last point before there,
            // in the window's far half, where the kit's tip ends at its
            // place's end.)
            olo_kit::tip::Reach { before: -(end - 1.0 - drawn.left), after: end - (drawn.left + drawn.width), above }
        } else {
            // (Its place drawn in to the note's first point, so the kit's
            // tip begins there, its words over the note's.)
            olo_kit::tip::Reach { before: TIP_PAD, after: 1.0 - drawn.width, above }
        };
        olo_kit::tip::on_above_of(el, "pool-footer-note", n.text.clone(), reach)
    }

    #[allow(clippy::too_many_arguments)]
    fn account_row(
        &self,
        account: &Account,
        hosts: &[Stands],
        unaccepted: &[String],
        rollup: &AccountRollup,
        selected: bool,
        wide: f32,
        label_w: (f32, f32),
        narrow: bool,
        revealing: bool,
        p: &olo_kit::palette::Palette,
        cx: &mut Cx<Self>,
    ) -> (Element, Option<Clear>) {
        let t = now();
        let sec = secondary_ink(p, p.list_background);
        let cust = custody(account, hosts, unaccepted);
        // Each of its body's lines, in order (None: one not known before
        // layout, a wrapped reason or note), and how clear its title's line
        // is: where the list's pinned band can end on it without cutting a
        // line (`row_cuts`): only while a row's being revealed.
        let mut lines: Vec<Option<Line>> = Vec::new();
        let mut line = |l: Option<Line>| {
            if revealing {
                lines.push(l);
            }
        };
        let is_signed_out = account.signed_out.is_some();
        let is_out_of_quota = !is_signed_out && account.enabled && account_out_of_quota(account, t).is_some();
        let asking_inline = self.removing_account.as_deref() == Some(account.id.as_str()) || self.resetting_credit.as_deref() == Some(account.id.as_str());

        // Line 1: checkbox mark + bold account name (+ inline red Signed out / amber Out of quota / grey Paused)
        // Right side: inline Sign In… button when signed out (matching Retry Now on Machines),
        // else crisp machine custody chip with 16pt computer/server icon.
        let (shared, id_toggle) = (self.shared.clone(), account.id.clone());
        let check = CheckBox::new(account.name.clone(), account.enabled).id(format!("use-{}", account.id)).mark_only().on_toggle(move |on: &bool, _| {
            if !demo_enabled() {
                shared.set_enabled(&id_toggle, *on);
            }
        });
        let name_w = f32::from(style::text_width(account.name.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::BOLD))).ceil() + 1.0;
        let mut title_group = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(div().w(px(13.0)).flex_shrink_0().flex().justify_center().child(check))
            // (As wide as it is where there's room; else cut short with
            // "…", its whole in a tip, 12 points clear of what's beside it,
            // the machine's name or Sign In…: a long name ran over "This
            // Computer"; Opus's and Neon's reviews, 2026-11-28.)
            .child(div().w(px(name_w)).flex_shrink().min_w_0().child(StringView::new(account.name.clone()).bold(true).cut(true).height(DETAIL_ROW)));
        if is_signed_out {
            title_group = title_group.child(StringView::new("·").color(sec).height(DETAIL_ROW)).child(StringView::new("Signed out").color(style::c(p.failure)).height(DETAIL_ROW));
        } else if !account.enabled {
            title_group = title_group.child(StringView::new("·").color(sec).height(DETAIL_ROW)).child(StringView::new("Paused").color(sec).height(DETAIL_ROW));
        } else if is_out_of_quota {
            title_group = title_group.child(StringView::new("·").color(sec).height(DETAIL_ROW)).child(StringView::new("Out of quota").color(style::c(p.warning)).height(DETAIL_ROW));
        }

        let line1 = if is_signed_out {
            let (prov, id_sign) = (account.provider, account.id.clone());
            // (Not while another sign-in is waited for: one at a time,
            // Cancel Sign-In in Add Account…'s place. Opus's review,
            // 2026-11-28.)
            let btn = Button::new("Sign In…").id(format!("pool-signin-{}", account.id)).min_width(wide).enabled(self.told.waiting.is_none()).on_click(move |_: &ClickEvent, _| {
                if !demo_enabled() {
                    olo_kit::proxy().send(Msg::SignIn(prov, id_sign.clone(), false));
                }
            });
            div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().justify_between().gap(px(m::SPACING)).child(title_group).child(btn)
        } else {
            let chip_text = if cust.is_pending() { format!("{} · Sync pending", cust.machine_name()) } else { cust.machine_name() };
            let machine_chip = div()
                .flex_shrink_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(neutral_machine_icon(cust.is_local(), 14.0))
                // (On the name's baseline: centred itself, it stood a pixel
                // off level at 2×, the layout review's "near-level".)
                .child(StringView::new(chip_text).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(sec).height(DETAIL_ROW));
            div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().justify_between().gap(px(m::SPACING)).child(title_group).child(machine_chip)
        };

        // Line 2+: indented straight under the bold account name (13pt gutter + 6pt gap = 19pt)
        let mut facts: Vec<String> = Vec::new();
        if let Some(e) = &account.email {
            facts.push(e.clone());
        }
        if let Some(plan) = &account.plan {
            facts.push(format!("{} plan", plan_name(plan)));
        }
        let credits: u32 = account.credits.iter().map(|c| c.remaining).sum();
        if credits > 0 {
            facts.push(format!("{credits} banked reset{}", if credits == 1 { "" } else { "s" }));
        }

        let mut body = div().w_full().pl(px(19.0)).flex().flex_col().gap(px(BODY_GAP));
        if !facts.is_empty() {
            // (The room it has: the list's, less its frame and bar, the
            // row's inset and this indent.)
            // (Signed out: ending before Sign In… over it, never under it,
            // 2 points from its frame: Neon's review, 2026-11-28.)
            let beside_button = if is_signed_out { wide + m::SPACING } else { 0.0 };
            let room = page_width(cx.viewport_size()) - 2.0 * 2.0 - widgets::SCROLL_BAR - 2.0 * 10.0 - 19.0 - beside_button;
            body = body.child(div().w_full().pr(px(beside_button)).child(facts_line(format!("pool-facts-{}", account.id), facts, room, sec)));
            line(Some(FACT_LINE));
        }
        if let Some(why) = &account.signed_out {
            // Reason in readable secondary ink (matches Remote Rex Hosts' error rows!)
            body = body.child(StringView::new(why.clone()).size(style::SMALL_SIZE).color(sec).wrap(true));
            line(None);
        } else {
            if let Some((paid_fam, until)) = account_paid_scope(account, t) {
                let note = match paid_fam {
                    Some(fam) => format!("{fam} waiting for reset at {} · paid usage credits only if nothing else can serve.", clock(until)),
                    None => format!("Waiting for reset at {} · paid usage credits only if nothing else can serve.", clock(until)),
                };
                body = body.child(div().w_full().pb(px(3.0)).child(StringView::new(note).size(style::SMALL_SIZE).color(sec).wrap(true)));
                line(None);
            }
            for w in &account.windows {
                body = body.child(inline_quota_row(p, w, t, label_w, narrow));
                line(Some(QUOTA_LINE));
            }
            if account.windows.is_empty() {
                body = body.child(StringView::new(if account.synced.is_some() { "No quota reported." } else { "Reading quota…" }).size(style::SMALL_SIZE).color(sec));
                line(None);
            }
            // Concise Today line (+ red failed count when failures occurred); local/remote breakdown lives on Activity.
            let u = &rollup.total_today;
            let failed_n = rollup.recent.iter().filter(|r| r.request.outcome == Outcome::Failed).count();
            let mut today_base = if u.requests == 0 {
                "Today: 0 requests".to_owned()
            } else {
                format!("Today: {} request{} · {} in · {} out", u.requests, if u.requests == 1 { "" } else { "s" }, tokens(u.input_tokens), tokens(u.output_tokens))
            };
            if failed_n > 0 {
                today_base.push_str(" · ");
            }
            let mut today_row = div().w_full().pt(px(2.0)).flex().flex_row().items_center().child(StringView::new(today_base).size(style::SMALL_SIZE).color(sec).height(DETAIL_ROW));
            if failed_n > 0 {
                today_row = today_row.child(StringView::new(format!("{failed_n} failed")).size(style::SMALL_SIZE).color(style::c(p.failure)).height(DETAIL_ROW));
            }
            body = body.child(today_row);
            line(Some(TODAY_LINE));
        }

        // Inline edit/confirm dialogs for this account
        if let Some((_, field)) = self.renaming.as_ref().filter(|(id, _)| *id == account.id) {
            body = body.child(div().pt(px(4.0)).flex().items_center().gap(px(m::SPACING)).child(TextControl::new(cx, field).width(180.0)).child(
                widgets::button_row().flex_1().child(Button::new("Cancel").on_click(cx.msg(WinMsg::Renamed(false)))).child(Button::new("Rename").default(true).on_click(cx.msg(WinMsg::Renamed(true)))),
            ));
        } else if self.resetting_credit.as_deref() == Some(account.id.as_str()) {
            let when = account.credit().and_then(|c| c.expires_at).map(|e| format!(" (expires {})", clock(e))).unwrap_or_default();
            body = body.child(div().pt(px(4.0)).child(warning_confirm(
                p,
                format!("Claim one of {}’s banked resets now{when}?", account.name),
                "Its quota refills immediately, and one banked reset is spent.".into(),
                None,
                Button::new("Cancel").on_click(cx.msg(WinMsg::ResetCredit(None))),
                Button::new("Claim Reset").on_click(cx.msg(WinMsg::ResetCredit(Some(account.id.clone())))),
            )));
        } else if self.removing_account.as_deref() == Some(account.id.as_str()) {
            body = body.child(div().pt(px(4.0)).child(warning_confirm(
                p,
                format!("Remove {} from Token Pool?", account.name),
                "Its sign-in stored here is deleted; the subscription account itself isn’t touched.".into(),
                None,
                Button::new("Cancel").on_click(cx.msg(WinMsg::RemoveAccount(None))),
                Button::new("Remove").on_click(cx.msg(WinMsg::RemoveAccount(Some(account.id.clone())))),
            )));
        }

        let sel_tint = if selected && !asking_inline {
            Some(if p.mode == olo_theme::ThemeMode::Dark {
                style::c(olo_theme::mix(p.list_background, p.keyboard_navigation, 0.16))
            } else {
                style::c(olo_theme::mix(p.list_background, p.keyboard_navigation, 0.09))
            })
        } else {
            None
        };
        let id_click = account.id.clone();
        let id_ctx = account.id.clone();
        let content_col = div().flex_1().min_w_0().flex().flex_col().child(line1).child(body);
        let mut row = div()
            .w_full()
            .px(px(10.0))
            .py(px(ROW_PAD))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(m::SPACING))
            .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, cx| cx.send(WinMsg::SelectAccount(id_click.clone()))))
            .on_mouse_down(MouseButton::Right, cx.listener(move |_, e: &olo_kit::MouseDownEvent, cx| cx.send(WinMsg::AccountContextMenu(id_ctx.clone(), e.position))))
            .child(content_col);
        if let Some(bg) = sel_tint {
            row = row.bg(bg);
        }
        // (An editor or a confirmation opened in it: of a height not known
        // before layout.)
        if self.renaming.as_ref().is_some_and(|(id, _)| *id == account.id) || asking_inline {
            line(None);
        }
        // (A signed-out row's Sign In… fills its title's line.)
        let title_clear = if is_signed_out { (0.0, 0.0) } else { (5.0, 4.0) };
        (row.into_element(), revealing.then(|| row_cuts(title_clear, &lines)))
    }

    // ---- Tab 2: Activity --------------------------------------------------

    fn activity_tab(&self, pool: &Pool, local_activity: &Activity, hosts: &[Stands], p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let vp = cx.viewport_size();
        let vp_w = f32::from(vp.width);
        let vp_h = f32::from(vp.height);
        let narrow = vp_w < 480.0;
        let compact_h = vp_h < 520.0;
        let two_col = compact_h && vp_w >= 560.0;
        // Short two-column tiles (quarter / horizontal half / horizontal third): merge
        // Machine + Span popups onto the left column's top row aligned with "Recent Requests"
        // on the right so chart + table both keep generous vertical height!
        let merged_two_col = two_col && vp_h < 480.0;
        // Short and too narrow for two columns (a quarter beside Deskbar,
        // 530 × 357): the legend on one line, and the chart and the table
        // sharing the height there is, each with a least of its own (the
        // chart's 112 points and a legend of three rows first left the
        // table nothing, its head past the window's foot: QA, 2026-11-28).
        let short_one_col = compact_h && !two_col && vp_h < 420.0;
        // (Shorter still, the table alone under the pop-ups: its Span
        // still chooses the requests it lists.)
        let page_h = activity_page(vp, pool.error.as_ref().map(|_| self.alert_h));
        // Quarter / narrow vertical: pair Machine + Span on the top bar so "Requests per Minute"
        // stands cleanly without colliding with the span popup.
        let pair_top_popups = !two_col && (vp_w < 500.0 || short_one_col);
        let sec_panel = secondary_ink(p, p.panel_background);

        let opts = Self::scope_options(hosts);
        let scope_idx = opts.iter().position(|(s, _)| s == &self.scope).unwrap_or(0);
        let active_scope = opts[scope_idx].0.clone();
        let scope_labels: Vec<SharedString> = opts.iter().map(|(_, l)| l.clone()).collect();
        // Keep "Machine:" label unless space is tight in merged horizontal-third or very narrow.
        let show_machine_lbl = vp_w >= 360.0 && !merged_two_col && !pair_top_popups;
        let scope_w = if merged_two_col || pair_top_popups {
            132.0
        } else if !show_machine_lbl {
            130.0
        } else if narrow {
            172.0
        } else {
            196.0
        };
        let mut scope_pick = MenuField::new("pool-scope", scope_labels, scope_idx).width(scope_w).on_change(cx.msg_with(|i: &usize| WinMsg::ScopeIndex(*i)));
        if show_machine_lbl {
            scope_pick = scope_pick.label("Machine:");
        }

        let (span_name, span_mins, step_mins, grid_secs) = SPANS[self.span_idx];
        let span_pick = MenuField::new("pool-span", SPANS.iter().map(|s| s.0.into()).collect(), self.span_idx)
            .width(if narrow || merged_two_col || pair_top_popups { 112.0 } else { 124.0 })
            .on_change(cx.msg_with(|i: &usize| WinMsg::Span(*i)));

        let t_ms = crate::now_ms();
        let live = live_requests(pool, hosts, &active_scope);
        let live_badge = match live.len() {
            0 => "Idle".to_owned(),
            n => format!("{n} running"),
        };

        let picked_line: Option<String> = match &self.activity_picked {
            Some(Pick::Live(key)) => live.iter().find(|o| &o.key == key).map(|o| live_details(o, t_ms)),
            Some(Pick::Recent(r)) => Some(recent_details(r)),
            None => None,
        };

        // Requests per Minute chart: crisp lines (no overlapping muddy wash),
        // draw series largest-total-first so smaller curves (Codex) stay visible on top,
        // and use plain `HH:MM` X-axis ticks (weekday only on `00:00` midnight).
        let t = now();
        let since = t - span_mins * 60;
        let combined = merged_activity(local_activity, hosts, &active_scope);
        let clients_list: Vec<String> = combined.clients();
        let lines: Vec<VecDeque<(f64, f64)>> = clients_list.iter().map(|cl| midpoints(&combined.series(cl, t, span_mins, step_mins))).collect();
        let totals: Vec<_> = clients_list.iter().map(|cl| combined.totals(cl, since)).collect();
        let shown: Vec<usize> = (0..clients_list.len()).filter(|&i| i < PROVIDERS.len() || totals[i].requests > 0).collect();
        let mut draw_order = shown.clone();
        draw_order.sort_by(|&a, &b| totals[b].requests.cmp(&totals[a].requests).then_with(|| a.cmp(&b)));
        let series: Vec<Series> = draw_order.iter().map(|&i| Series::new(clients_list[i].clone(), client_color(p, &clients_list[i]), Rc::new(lines[i].clone())).fill(false)).collect();
        let fmt = |v: f64| if v >= 10.0 || (v - v.round()).abs() < 1e-6 { format!("{v:.0}/min") } else { format!("{v:.1}/min") };
        let span_secs = (span_mins * 60) as f64;
        let graph = Graph::new(format!("pool-activity-{span_name}"), series, t as f64 + span_secs * 0.015)
            .span(span_secs)
            .grid(grid_secs)
            .unit(fmt)
            .y_labels(true)
            .x_labels(|sec| {
                use chrono::{Local, TimeZone, Timelike};
                let Some(at) = Local.timestamp_opt(sec as i64, 0).single() else { return String::new() };
                if at.hour() == 0 && at.minute() == 0 { at.format("%a %H:%M").to_string() } else { at.format("%H:%M").to_string() }
            })
            .smooth(false);

        // Stacked 18pt legend with fixed provider column whenever 3 entries would wrap
        // into an orphan 2+1 line; in wide+short horizontal-third (`two_col && left_w >= 380`)
        // use one concise horizontal row so the sparkline keeps full vertical breathing room.
        let left_w = (vp_w * 0.41).clamp(244.0, 480.0);
        let stack_legend = if two_col { left_w < 380.0 } else { vp_w < 860.0 && !short_one_col };
        let compact_inline_legend = (two_col && !stack_legend) || short_one_col;
        let mut legend_row = if stack_legend {
            div().w_full().pt(px(2.0)).flex().flex_col().gap(px(0.0))
        } else if short_one_col {
            // (Wrapping where even that is too wide.)
            div().w_full().flex().flex_row().flex_wrap().items_center().gap(px(12.0))
        } else if compact_inline_legend {
            div().w_full().h(px(18.0)).flex().flex_row().items_center().gap(px(12.0))
        } else {
            div().w_full().pt(px(2.0)).flex().flex_row().flex_wrap().items_center().gap(px(16.0))
        };
        // (Each item's width, as its words are laid out: how many lines
        // the legend takes where it wraps.)
        let small_w = |t: &str| f32::from(style::text_width(t.to_owned(), style::SMALL_SIZE, style::ui_font(FontWeight::NORMAL))).ceil();
        let mut legend_widths: Vec<f32> = Vec::new();
        for &i in &shown {
            let x = totals[i];
            let req_word = if compact_inline_legend { format!("{}", x.requests) } else { format!("{} request{}", x.requests, if x.requests == 1 { "" } else { "s" }) };
            let tok_tail =
                if !two_col && !short_one_col && x.input_tokens + x.output_tokens > 0 { format!(" · {} in · {} out", tokens(x.input_tokens), tokens(x.output_tokens)) } else { String::new() };
            let stats_w = if x.failed > 0 {
                small_w(&format!("{req_word} · ")) + small_w(&format!("{} failed", x.failed)) + if tok_tail.is_empty() { 0.0 } else { small_w(&tok_tail) }
            } else {
                small_w(&format!("{req_word}{tok_tail}"))
            };
            legend_widths.push(7.0 + 6.0 + small_w(&clients_list[i]) + 5.0 + stats_w);
            let name_el = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .child(swatch_dot(client_color(p, &clients_list[i]), 7.0))
                .child(StringView::new(clients_list[i].clone()).size(style::SMALL_SIZE).height(DETAIL_ROW));
            let name_col = if stack_legend { name_el.w(px(88.0)).flex_shrink_0() } else { name_el };
            let mut item = if stack_legend {
                div().w_full().h(px(18.0)).flex().flex_row().items_center().gap(px(6.0)).child(name_col)
            } else {
                div().flex().flex_row().items_center().gap(px(5.0)).child(name_col)
            };
            if x.failed > 0 {
                let mut stats = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(StringView::new(format!("{req_word} · ")).size(style::SMALL_SIZE).color(sec_panel).height(DETAIL_ROW))
                    .child(StringView::new(format!("{} failed", x.failed)).size(style::SMALL_SIZE).color(style::c(p.failure)).height(DETAIL_ROW));
                if !tok_tail.is_empty() {
                    stats = stats.child(StringView::new(tok_tail).size(style::SMALL_SIZE).color(sec_panel).height(DETAIL_ROW));
                }
                item = item.child(stats);
            } else {
                item = item.child(StringView::new(format!("{req_word}{tok_tail}")).size(style::SMALL_SIZE).color(sec_panel).height(DETAIL_ROW));
            }
            legend_row = legend_row.child(item);
        }
        // (Short and in one column: the chart over the table where both
        // have their least, its legend as many lines as it wraps to; else
        // the table alone under the pop-ups, its Span still choosing the
        // requests it lists.)
        let legend_h = wrapped_height(&legend_widths, page_width(vp), 12.0);
        // (A request chosen: its details' row under the table, in what
        // the table keeps: it left the table less than a row; Neon's
        // review, 2026-11-28.)
        let recent_need = RECENT_LEAST + if picked_line.is_some() { PICKED_ROW } else { 0.0 };
        let no_chart = short_one_col && !activity_chart_fits(page_h, legend_h, recent_need);

        let (top_bar_opt, chart_hdr) = if merged_two_col {
            // Horizontal-third: both popups share the left column's top row,
            // aligned on the exact same baseline as "Recent Requests" on the right.
            let hdr = div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().justify_between().gap(px(6.0)).child(scope_pick).child(span_pick);
            (None, hdr)
        } else if pair_top_popups {
            // Quarter / narrow: Machine + Span share the top bar so the chart heading never collides.
            let top = div().w_full().flex().flex_row().items_center().justify_between().gap(px(6.0)).child(scope_pick).child(span_pick);
            let hdr = div().w_full().h(px(m::LABEL_ROW)).flex().flex_row().items_center().child(StringView::new("Requests per Minute").bold(true).height(m::LABEL_ROW));
            (Some(top), hdr)
        } else {
            let top = div().w_full().flex().flex_row().items_center().justify_between().gap(px(m::SPACING)).child(scope_pick);
            let hdr = div()
                .w_full()
                .h(px(m::CONTROL_HEIGHT))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(StringView::new("Requests per Minute").bold(true).height(m::CONTROL_HEIGHT))
                .child(span_pick);
            (Some(top), hdr)
        };

        let mut chart_section = div().w_full().flex().flex_col().gap(px(6.0)).child(chart_hdr);
        if two_col || short_one_col {
            // (Its least all it holds, short and in one column: at 56, the
            // plot and the legend ran over Recent Requests' heading, "123"
            // 5 points from it; the layout lint, 2026-11-28.)
            let (plot_least, least) = if short_one_col {
                let plot = activity_plot_least(page_h, legend_h, recent_need);
                (plot, chart_least(plot, legend_h))
            } else {
                (PLOT_LEAST, 56.0)
            };
            chart_section = chart_section.flex_1().min_h(px(least)).child(div().flex_1().min_h(px(plot_least)).w_full().child(graph)).child(legend_row);
        } else {
            let gh = if compact_h { 112.0 } else { GRAPH_HEIGHT };
            chart_section = chart_section.child(div().w_full().h(px(gh)).child(graph)).child(legend_row);
        }

        // Recent Requests table: compute exact inner content width (subtracting outer pad,
        // TabView inset, well border, vertical scrollbar, and row padding), drop Machine
        // FIRST when all 7 columns don't fit, and cap Model on wide/zoomed windows so
        // Account absorbs slack right beside Model instead of floating across a void.
        // (The page's own width: 8 points more had a column the table
        // hadn't room for; Astra's review, 2026-11-28.)
        let well_w = if two_col { page_width(vp) - left_w - 20.0 } else { page_width(vp) };
        let inner_w = well_w - 4.0 - widgets::SCROLL_BAR - 2.0 * m::TIGHT;
        let recent = recent_requests(pool, hosts, &active_scope, since, RECENT_TABLE_LIMIT);
        let has_hosts = hosts.iter().any(|h| h.wanted || h.status.is_some());
        let recent_well = self.recent_requests_well(&live, &recent, &active_scope, has_hosts, inner_w, t_ms, span_name, p, cx);

        let recent_hdr_h = if two_col { m::CONTROL_HEIGHT } else { m::LABEL_ROW };
        let recent_hdr = div()
            .w_full()
            .h(px(recent_hdr_h))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(StringView::new("Recent Requests").bold(true).height(recent_hdr_h))
            .child(StringView::new(live_badge).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(sec_panel).height(recent_hdr_h));

        let picked_bar = picked_line.map(|detail| {
            div()
                .w_full()
                .pt(px(2.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(6.0))
                .child(div().flex_1().min_w_0().child(StringView::new(detail).size(style::SMALL_SIZE).color(sec_panel).cut(true).height(DETAIL_ROW)))
                .child(Button::new("Clear").id("pool-clear-pick").on_click(cx.msg::<ClickEvent>(WinMsg::ClearPick)))
        });

        // Wide + short horizontal third or quarter: side-by-side 2-column layout with 20pt gutter
        if two_col {
            let left_col = div().relative().w(px(left_w)).flex_shrink_0().h_full().flex().flex_col().child(canvas(olo_kit::review::own_grid).paint_key(0).absolute().inset_0()).child(chart_section);
            let mut right_col = div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(6.0)).child(recent_hdr).child(recent_well);
            if let Some(bar) = picked_bar {
                right_col = right_col.child(bar);
            }
            let body = div().flex_1().min_h_0().flex().flex_row().gap(px(20.0)).child(left_col).child(right_col);
            return match top_bar_opt {
                Some(top_bar) => div().flex_1().min_h_0().flex().flex_col().gap(px(6.0)).child(top_bar).child(body).into_element(),
                None => body.into_element(),
            };
        }

        // (Short: its heading, and the table's head and two rows, at
        // least; never more than the page has under the pop-ups.)
        let recent_least = if short_one_col { recent_kept(recent_need, page_h) } else { 0.0 };
        let mut recent_section = div().flex_1().min_h(px(recent_least)).flex().flex_col().gap(px(6.0)).child(recent_hdr).child(recent_well);
        if let Some(bar) = picked_bar {
            recent_section = recent_section.child(bar);
        }

        let mut root = div().flex_1().min_h_0().flex().flex_col().gap(px(if compact_h { 8.0 } else { 20.0 }));
        if let Some(top_bar) = top_bar_opt {
            root = root.child(top_bar);
        }
        if !no_chart {
            root = root.child(chart_section);
        }
        root.child(recent_section).into_element()
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn recent_requests_well(
        &self,
        live: &[LiveReq],
        recent: &[RecentReq],
        active_scope: &MachineScope,
        has_hosts: bool,
        inner_w: f32,
        t_ms: i64,
        span_name: &str,
        p: &olo_kit::palette::Palette,
        cx: &mut Cx<Self>,
    ) -> Element {
        let sec_panel = secondary_ink(p, p.panel_background);
        let sec_list = secondary_ink(p, p.list_background);
        let time_w = 38.0;
        let machine_w = 96.0;
        let account_w = 88.0;
        let in_w = 42.0;
        let out_w = 38.0;
        // (As wide as what it says, "Status" and the widest status shown,
        // a running one's as it could count up to a minute: 96 points kept
        // 25 empty beside "gemini-4-neon-hi…" cut short; Opus's and
        // Neon's reviews, 2026-11-28.)
        let small = style::ui_font(FontWeight::NORMAL);
        let said_w = |t: &str| f32::from(style::text_width(t.to_owned(), style::SMALL_SIZE, small.clone())).ceil();
        let mut status_w = f32::from(style::text_width("Status".to_owned(), style::SMALL_SIZE, style::ui_font(FontWeight::BOLD))).ceil();
        if !live.is_empty() {
            status_w = status_w.max(said_w("Running · 88.8s"));
        }
        for o in live.iter().filter(|o| o.started_ms > 0) {
            status_w = status_w.max(said_w(&format!("Running · {}", duration((t_ms - o.started_ms).max(0) as u64))));
        }
        for r in recent {
            status_w = status_w.max(said_w(&status_said(r)));
        }
        let status_w = status_w.min(96.0);
        // Exact column sums (including 1pt divider + 10pt gaps per boundary, model_min = 128pt):
        // 7 cols: 592pt; 6 cols (no Machine): 485pt; 4 cols (no Machine/In/Out): 383pt; 3 cols: 284pt.
        let show_machine_col = *active_scope == MachineScope::All && has_hosts && inner_w >= 596.0;
        let show_io_cols = inner_w >= 488.0;
        let show_account_col = inner_w >= 386.0;
        let wide_account_flex = show_account_col && inner_w > 700.0;

        let hdr_div = || div().w(px(1.0)).h(px(12.0)).bg(style::c(p.control_border)).flex_shrink_0();
        let hdr_label = |label: &'static str| StringView::new(label).size(style::SMALL_SIZE).bold(true).color(sec_panel).height(DETAIL_ROW);
        let mut header_row = div()
            .w_full()
            .pl(px(m::TIGHT))
            .pr(px(m::TIGHT + widgets::SCROLL_BAR))
            .pt(px(3.0))
            .pb(px(2.0))
            .bg(style::c(p.panel_background))
            .border_b_1()
            .border_color(style::c(p.control_border))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .child(div().w(px(time_w)).flex_shrink_0().child(hdr_label("Time")));
        if show_machine_col {
            header_row = header_row.child(hdr_div()).child(div().w(px(machine_w)).flex_shrink_0().child(hdr_label("Machine")));
        }
        header_row = header_row.child(hdr_div()).child(if wide_account_flex {
            div().w(px(176.0)).pr(px(8.0)).flex_shrink_0().child(hdr_label("Model"))
        } else {
            div().flex_1().min_w(px(116.0)).pr(px(8.0)).child(hdr_label("Model"))
        });
        if show_account_col {
            header_row = header_row.child(hdr_div()).child(if wide_account_flex {
                div().flex_1().min_w(px(96.0)).child(hdr_label("Account"))
            } else {
                div().w(px(account_w)).flex_shrink_0().child(hdr_label("Account"))
            });
        }
        if show_io_cols {
            header_row = header_row
                .child(hdr_div())
                .child(div().w(px(in_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(hdr_label("In")))
                .child(hdr_div())
                .child(div().w(px(out_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(hdr_label("Out")));
        }
        header_row = header_row.child(hdr_div()).child(div().w(px(status_w)).flex_shrink_0().flex().justify_end().child(hdr_label("Status")));

        let sel_bg = style::c(style::selection_ground(p, p.list_background, true));
        let sel_fg = style::c(style::selection_text(p, p.panel_text, true));
        let mut rows = div().w_full().pb(px(4.0)).flex().flex_col();

        let col_gap = || div().w(px(1.0)).flex_shrink_0();

        for o in live {
            let selected = matches!(&self.activity_picked, Some(Pick::Live(k)) if k == &o.key);
            let fg = if selected { sel_fg } else { style::c(p.panel_text) };
            let dim_fg = if selected { sel_fg } else { sec_list };
            let start_clock = if o.started_ms > 0 { clock(o.started_ms / 1000) } else { "now".into() };
            let key = o.key.clone();
            let mut line = div()
                .w_full()
                .pl(px(m::TIGHT))
                .pr(px(m::TIGHT))
                .py(px(3.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, cx| cx.send(WinMsg::Pick(Pick::Live(key.clone())))));
            if selected {
                line = line.bg(sel_bg);
            }
            line = line.child(div().w(px(time_w)).flex_shrink_0().child(StringView::new(start_clock).size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)));
            if show_machine_col {
                line = line.child(col_gap()).child(
                    div()
                        .w(px(machine_w))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(neutral_machine_icon(o.is_local, 14.0))
                        .child(div().flex_1().min_w_0().child(StringView::new(o.machine_name.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW))),
                );
            }
            let model_cell = div()
                .pr(px(8.0))
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(swatch_dot(provider_color(p, o.provider), 6.0))
                .child(div().flex_1().min_w_0().child(StringView::new(o.model.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW)));
            line = line.child(col_gap()).child(if wide_account_flex { model_cell.w(px(176.0)).flex_shrink_0() } else { model_cell.flex_1().min_w(px(116.0)) });
            if show_account_col {
                let acct_txt = StringView::new(o.account.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW);
                line = line.child(col_gap()).child(if wide_account_flex {
                    div().flex_1().min_w(px(96.0)).overflow_hidden().child(acct_txt)
                } else {
                    div().w(px(account_w)).flex_shrink_0().overflow_hidden().child(acct_txt)
                });
            }
            if show_io_cols {
                line = line
                    .child(col_gap())
                    .child(div().w(px(in_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new("—").size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)))
                    .child(col_gap())
                    .child(div().w(px(out_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new("—").size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)));
            }
            let elapsed = if o.started_ms > 0 { format!("Running · {}", duration((t_ms - o.started_ms).max(0) as u64)) } else { "Running".to_owned() };
            line = line
                .child(col_gap())
                .child(div().w(px(status_w)).flex_shrink_0().overflow_hidden().flex().justify_end().child(StringView::new(elapsed).size(style::SMALL_SIZE).color(fg).height(DETAIL_ROW)));
            rows = rows.child(line);
        }

        for r in recent {
            let selected = matches!(&self.activity_picked, Some(Pick::Recent(pk)) if pk.key == r.key);
            let fg = if selected { sel_fg } else { style::c(p.panel_text) };
            let dim_fg = if selected { sel_fg } else { sec_list };
            let status_text = status_said(r);
            let status_fg = match r.request.outcome {
                Outcome::Completed => dim_fg,
                Outcome::Failed if r.request.status != 0 => {
                    if selected {
                        sel_fg
                    } else {
                        style::c(p.failure)
                    }
                }
                other => {
                    if selected {
                        sel_fg
                    } else {
                        outcome_color(p, other)
                    }
                }
            };
            let picked = r.clone();
            let mut line = div()
                .w_full()
                .pl(px(m::TIGHT))
                .pr(px(m::TIGHT))
                .py(px(3.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, cx.listener(move |_, _, cx| cx.send(WinMsg::Pick(Pick::Recent(picked.clone())))));
            if selected {
                line = line.bg(sel_bg);
            }
            line = line.child(div().w(px(time_w)).flex_shrink_0().child(StringView::new(clock(r.request.at)).size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)));
            if show_machine_col {
                line = line.child(col_gap()).child(
                    div()
                        .w(px(machine_w))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .child(neutral_machine_icon(r.is_local, 14.0))
                        .child(div().flex_1().min_w_0().child(StringView::new(r.machine_name.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW))),
                );
            }
            let model_cell = div()
                .pr(px(8.0))
                .overflow_hidden()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(swatch_dot(provider_color(p, r.provider), 6.0))
                .child(div().flex_1().min_w_0().child(StringView::new(r.request.model.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW)));
            line = line.child(col_gap()).child(if wide_account_flex { model_cell.w(px(176.0)).flex_shrink_0() } else { model_cell.flex_1().min_w(px(116.0)) });
            if show_account_col {
                let acct_txt = StringView::new(r.account.clone()).size(style::SMALL_SIZE).color(fg).cut(true).height(DETAIL_ROW);
                line = line.child(col_gap()).child(if wide_account_flex {
                    div().flex_1().min_w(px(96.0)).overflow_hidden().child(acct_txt)
                } else {
                    div().w(px(account_w)).flex_shrink_0().overflow_hidden().child(acct_txt)
                });
            }
            if show_io_cols {
                let in_s = if r.request.usage.input_tokens > 0 { tokens(r.request.usage.input_tokens) } else { "—".into() };
                let out_s = if r.request.usage.output_tokens > 0 { tokens(r.request.usage.output_tokens) } else { "—".into() };
                line = line
                    .child(col_gap())
                    .child(div().w(px(in_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new(in_s).size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)))
                    .child(col_gap())
                    .child(div().w(px(out_w)).pr(px(4.0)).flex_shrink_0().flex().justify_end().child(StringView::new(out_s).size(style::SMALL_SIZE).color(dim_fg).height(DETAIL_ROW)));
            }
            line = line
                .child(col_gap())
                .child(div().w(px(status_w)).flex_shrink_0().overflow_hidden().flex().justify_end().child(StringView::new(status_text).size(style::SMALL_SIZE).color(status_fg).height(DETAIL_ROW)));
            rows = rows.child(line);
        }

        if live.is_empty() && recent.is_empty() {
            let empty_msg = format!("No requests in the {}.", span_name.to_ascii_lowercase());
            rows = rows.child(div().w_full().py(px(32.0)).flex().justify_center().child(StringView::new(empty_msg).color(sec_list)));
        }

        let well_frame = canvas(|g, b| {
            let p = theme::palette();
            style::sunken(g, b, &p, false);
        })
        .paint_key(style::key("recent-well"))
        .absolute()
        .inset_0();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .p(px(2.0))
            .bg(style::c(p.list_background))
            .flex()
            .flex_col()
            .child(header_row)
            .child(div().flex_1().min_h_0().child(ScrollView::new(&self.activity_scroll).border(false).background(p.list_background).child(rows)))
            .child(well_frame)
            .into_element()
    }

    // ---- Tab 3: Machines --------------------------------------------------

    fn machines_tab(&self, pool: &Pool, hosts: &[Stands], running: Option<&Running>, uses: Option<[bool; 3]>, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let vp = cx.viewport_size();
        let vp_w = f32::from(vp.width);
        let vp_h = f32::from(vp.height);
        let narrow = vp_w < 480.0;
        let compact_h = vp_h < 520.0;
        let tight_h = vp_h < 290.0;
        let sec_panel = secondary_ink(p, p.panel_background);

        // Sort remote hosts by severity FIRST: errors -> in progress -> healthy -> not installed!
        let ordered_hosts = sorted_hosts(hosts);
        let local_section = self.this_computer_section(pool, &ordered_hosts, running, uses, narrow, compact_h, tight_h, p, cx);
        let sel_host = self.selected_host.as_deref().filter(|id| ordered_hosts.iter().any(|h| h.id == *id)).or_else(|| ordered_hosts.first().map(|h| h.id.as_str()));
        let sel_stands = sel_host.and_then(|id| ordered_hosts.iter().find(|h| h.id == id));

        let left_w = if vp_w >= 860.0 { (vp_w * 0.34).clamp(240.0, 360.0) } else { (vp_w * 0.38).clamp(212.0, 300.0) };
        // This Computer beside the hosts in a short window wide enough for
        // both (a quarter, a horizontal third); else over them. (Beside
        // them in a short third, 346 points, the hosts had 70: Astra's and
        // Neon's reviews, 2026-11-28.)
        let two_col = machines_two_col(vp_w, vp_h);
        // (Short and too narrow for two columns: This Computer and the
        // hosts scroll as one, the bar beside them.)
        let one_scroll = compact_h && !two_col;
        let hosts_col_w = if two_col {
            vp_w - left_w - 44.0
        } else if one_scroll {
            vp_w - 28.0 - widgets::SCROLL_BAR
        } else {
            vp_w - 28.0
        };

        // Remote Rex Hosts heading + red error count matching Activity's red failures
        let err_n = ordered_hosts.iter().filter(|h| h.error.is_some()).count();
        let mut count_row = div().flex().flex_row().items_center();
        if !ordered_hosts.is_empty() {
            let mut cfg_txt = format!("{} configured", ordered_hosts.len());
            if err_n > 0 {
                cfg_txt.push_str(" · ");
            }
            count_row = count_row.child(StringView::new(cfg_txt).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(sec_panel).height(m::LABEL_ROW));
            if err_n > 0 {
                let err_txt = if err_n == 1 { "1 error".to_owned() } else { format!("{err_n} errors") };
                count_row = count_row.child(StringView::new(err_txt).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(style::c(p.failure)).height(m::LABEL_ROW));
            }
        }
        let hosts_hdr = div().w_full().flex().flex_row().items_center().justify_between().child(StringView::new("Remote Rex Hosts").bold(true).height(m::LABEL_ROW)).child(count_row);

        // ONE sunken framed list well for Remote Rex Hosts (takes all remaining height!)
        let wide = ["Sign In…", "Retry Now", "Sync Now", "Install", "Try Again"].iter().map(|l| button_width(l, false)).fold(m::BUTTON_MIN_WIDTH, f32::max);
        let mut host_list = div().w_full().flex().flex_col();
        if !self.hosts.carried() && !demo_enabled() {
            host_list = host_list.child(
                div()
                    .w_full()
                    .p(px(10.0))
                    .child(StringView::new("This Olo was built without the Token Pool for hosts, so none can be installed.").size(style::SMALL_SIZE).wrap(true).color(style::c(p.warning))),
            );
        }
        if ordered_hosts.is_empty() {
            let top_pad = if compact_h { 20.0 } else { 96.0 };
            host_list = host_list.child(
                div()
                    .w_full()
                    .pt(px(top_pad))
                    .pb(px(24.0))
                    .px(px(24.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(icon_box("network", 48.0))
                    .child(div().pt(px(8.0)).child(StringView::new("No remote hosts yet").bold(true).height(m::LABEL_ROW)))
                    .child(
                        div()
                            .pt(px(4.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(StringView::new("Click Add Host… to install Token Pool on a Rex host").size(style::SMALL_SIZE).color(secondary_ink(p, p.list_background)).height(DETAIL_ROW))
                            .child(StringView::new("so your machines share accounts.").size(style::SMALL_SIZE).color(secondary_ink(p, p.list_background)).height(DETAIL_ROW)),
                    ),
            );
        } else {
            for (idx, h) in ordered_hosts.iter().enumerate() {
                let kept = kept_names(pool, Some(&h.id));
                let is_sel = sel_host == Some(h.id.as_str());
                let row = self.remote_host_row(h, &kept, is_sel, wide, hosts_col_w, p, cx);
                host_list = host_list.child(if idx > 0 { under_rule(p, row) } else { row });
            }
        }

        // Consistent footer matching Accounts order & labels: [Add Host…] [Remove Host…] … Updated HH:MM [Sync All]
        let any_syncable = ordered_hosts.iter().any(|h| h.wanted && h.doing.is_none());
        let asking_this = sel_host.is_some() && self.host_asking_remove.as_deref() == sel_host;
        let can_remove = !asking_this && sel_stands.is_some_and(|h| h.wanted && h.doing.is_none());
        let rm_id = sel_stands.map(|h| h.id.clone()).unwrap_or_default();
        let checked_text = last_checked_at(&ordered_hosts).map(|at| format!("Updated {}", clock(at))).unwrap_or_default();

        // (Its column's own width: beside This Computer in a short window,
        // the 20-point gutter between.)
        let gap = if two_col { 6.0 } else { m::SPACING };
        let footer_w = if two_col { page_width(vp) - left_w - 20.0 } else { page_width(vp) };
        let two_rows = footer_wraps(&["Add Host…", "Remove Host…", "Sync All"], 0.0, gap, footer_w);
        let mut status_box = div().flex_1().min_w_0().flex().justify_end();
        // (Whole, beside Sync All, where it fits in what its row leaves:
        // never past the buttons beside it; Astra's and Neon's reviews,
        // 2026-11-28.)
        let lead_w = if two_rows { 0.0 } else { ["Add Host…", "Remove Host…"].iter().map(|l| button_width(l, false) + gap).sum() };
        let checked_room = footer_w - lead_w - button_width("Sync All", false) - gap;
        let checked_w = f32::from(style::text_width(checked_text.clone(), style::PLAIN_SIZE, style::ui_font(FontWeight::NORMAL)));
        if !checked_text.is_empty() && checked_w <= checked_room && (hosts_col_w >= 440.0 || two_rows) {
            status_box = status_box.child(StringView::new(checked_text).color(sec_panel).height(m::CONTROL_HEIGHT));
        }

        let footer = footer_rows(
            vec![
                Button::new("Add Host…").id("pool-add-host").on_click(|_: &ClickEvent, _| olo_kit::app_menu::open_settings("rex")).into_element(),
                Button::new("Remove Host…").id("pool-remove-host").enabled(can_remove).on_click(cx.msg::<ClickEvent>(WinMsg::HostAskRemove(Some(rm_id)))).into_element(),
            ],
            status_box,
            Button::new("Sync All").id("pool-sync-all").enabled(any_syncable).on_click(cx.msg::<ClickEvent>(WinMsg::SyncAllHosts)).into_element(),
            gap,
            two_rows,
        );

        if one_scroll {
            // A short window too narrow for This Computer beside the hosts
            // (a third of a screen, 349 × 357; the window's least, 346 ×
            // 220): the two scroll as one over the footer, the hosts' list
            // in a well of its own height. (Beside each other the hosts had
            // 70 points: Astra's and Neon's reviews, 2026-11-28; one over
            // the other, This Computer left the list no room at all, its
            // footer past the window's foot: the layout lint.)
            let ground = style::c(p.list_background);
            let frame = canvas(move |g, b| {
                let p = theme::palette();
                let b = style::snap(g, b);
                style::fill_rect(g, b, ground);
                style::sunken(g, b, &p, false);
            })
            .paint_key(style::key(style::ink_key(Some(ground))))
            .absolute()
            .inset_0();
            // (Inside the well's two lines, on device pixels.)
            let scale = cx.scale_factor();
            let e = 2.0 * style::line_pixels(scale) / scale;
            let hosts_box = div().relative().w_full().child(frame).child(div().p(px(e)).child(host_list));
            let both = div().w_full().flex().flex_col().gap(px(10.0)).pr(px(m::TIGHT)).child(local_section).child(div().w_full().flex().flex_col().gap(px(6.0)).child(hosts_hdr).child(hosts_box));
            let page = ScrollView::new(&self.hosts_scroll).border(false).background(p.panel_background).child(both);
            return div().flex_1().min_h_0().flex().flex_col().gap(px(10.0)).child(div().flex_1().min_h_0().child(page)).child(footer).into_element();
        }

        let hosts_well = div().flex_1().min_h_0().child(ScrollView::new(&self.hosts_scroll).background(p.list_background).child(host_list));
        if two_col {
            // In short tiles (quarter / horizontal third), both columns share one top heading line
            // with a generous 20pt gutter, and host buttons sit right beneath Remote Rex Hosts!
            // (The footer a spacing under the list, as Accounts' is: at 6
            // it sat on the list's frame; Neon's review, 2026-11-28.)
            let hosts_col = div().flex_1().min_w_0().h_full().flex().flex_col().gap(px(6.0)).child(hosts_hdr).child(hosts_well).child(div().pt(px(m::SPACING - 6.0)).child(footer));
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_row()
                .gap(px(20.0))
                .child(div().relative().w(px(left_w)).flex_shrink_0().h_full().flex().flex_col().child(canvas(olo_kit::review::own_grid).paint_key(0).absolute().inset_0()).child(local_section))
                .child(hosts_col)
                .into_element();
        }

        let hosts_section = div().flex_1().min_h_0().flex().flex_col().gap(px(6.0)).child(hosts_hdr).child(hosts_well);

        div().flex_1().min_h_0().flex().flex_col().gap(px(10.0)).child(div().w_full().pb(px(10.0)).child(local_section)).child(hosts_section).child(footer).into_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn this_computer_section(
        &self,
        pool: &Pool,
        hosts: &[Stands],
        running: Option<&Running>,
        uses: Option<[bool; 3]>,
        narrow: bool,
        compact_h: bool,
        tight_h: bool,
        p: &olo_kit::palette::Palette,
        cx: &mut Cx<Self>,
    ) -> Element {
        let sec = secondary_ink(p, p.panel_background);
        // Unified left column matching Remote Rex Hosts below: 13pt gutter + 5pt gap = 18pt exact!
        const INDENT: f32 = 18.0;

        let port = self.effective_port();
        let (gw_lamp, gw_status, gw_err, gw_detail) = match &pool.error {
            Some(e) => {
                let blamed = error_port(e, port);
                let reason = if e.to_ascii_lowercase().contains("address already in use") { format!("Port {blamed} is in use by another app.") } else { e.clone() };
                (Light::Down, "Stopped".to_owned(), true, Some(reason))
            }
            None if compact_h => (Light::On, format!("127.0.0.1:{port}"), false, None),
            None => (Light::On, format!("Listening on 127.0.0.1:{port}"), false, None),
        };

        // Bold section heading matching "Remote Rex Hosts" (same x-offset, no BBox frame!)
        let hdr = div().w_full().flex().flex_row().items_center().justify_between().gap(px(8.0)).child(StringView::new("This Computer").bold(true).height(m::LABEL_ROW));

        // 1. Gateway row + named account custody (wrapped cleanly at " · " when narrow/compact)
        let mut gw_block = div().w_full().flex().flex_col().gap(px(2.0)).child(
            div()
                .w_full()
                .h(px(20.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(div().w(px(13.0)).flex_shrink_0().flex().justify_center().child(light(gw_lamp)))
                .child(StringView::new("Gateway").bold(true).height(DETAIL_ROW))
                .child(StringView::new("·").color(sec).height(DETAIL_ROW))
                .child(div().flex_1().min_w_0().child(StringView::new(gw_status).color(if gw_err { style::c(p.failure) } else { sec }).cut(true).height(DETAIL_ROW))),
        );
        if let Some(reason) = gw_detail {
            gw_block = gw_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(reason).size(style::SMALL_SIZE).color(sec).wrap(true)));
        } else if !tight_h && let Some(custody_line) = local_custody_summary(pool, hosts) {
            if (narrow || compact_h)
                && let Some((left, right)) = custody_line.split_once(" · ")
            {
                let cap_right = sentence(right);
                // (Two sentences: a tight spacing between them, more than
                // between a sentence's lines; Opus's and Neon's reviews,
                // 2026-11-28.)
                gw_block = gw_block
                    .child(div().w_full().pl(px(INDENT)).child(StringView::new(left.to_owned()).size(style::SMALL_SIZE).color(sec).wrap(true)))
                    .child(div().w_full().pl(px(INDENT)).pt(px(m::TIGHT - 2.0)).child(StringView::new(cap_right).size(style::SMALL_SIZE).color(sec).wrap(true)));
            } else {
                gw_block = gw_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(custody_line).size(style::SMALL_SIZE).color(sec).wrap(true)));
            }
        }

        // 2. Client routing toggles grouped under a concise "Use the pool" label so
        //    quarter & horizontal-half never cramp before the right-aligned running count.
        let use_pool_hdr = div().w_full().child(StringView::new("Use the pool").size(style::SMALL_SIZE).color(sec).height(DETAIL_ROW));

        let claude_on = uses.is_some_and(|c| c[client_index(Provider::Claude)]);
        let claude_procs = running.map(|r| r.claude.as_slice()).unwrap_or(&[]);
        let claude_count = match claude_procs.len() {
            0 => "Idle".to_owned(),
            n => format!("{n} running"),
        };
        let status_cell = |txt: String, col: Hsla| {
            div().w(px(58.0)).flex_shrink_0().flex().justify_end().child(StringView::new(txt).size(style::SMALL_SIZE).beside(style::PLAIN_SIZE).color(col).height(m::MARK_ROW))
        };
        let mut claude_block = div().w_full().flex().flex_col().gap(px(2.0)).child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(div().flex_1().min_w_0().child(CheckBox::new("Claude Code", claude_on).enabled(uses.is_some()).on_toggle(|on: &bool, _| {
                    if !demo_enabled() {
                        olo_kit::proxy().send(Msg::SetClient(Provider::Claude, *on));
                    }
                })))
                .child(status_cell(claude_count, sec)),
        );
        if !tight_h {
            for proc in claude_procs {
                let Process { started, cwd, connections, .. } = proc;
                let raw_place = cwd.as_deref().map(folder).unwrap_or_else(|| "?".into());
                let place = if narrow { middle_truncate(&raw_place, 18) } else { raw_place };
                let wrong = claude_on && connections.direct > 0 && connections.pool == 0;
                let line = if wrong { format!("{place} · bypassing pool · since {}", clock(*started)) } else { format!("{place} · since {}", clock(*started)) };
                claude_block = claude_block
                    .child(div().w_full().pl(px(INDENT)).child(StringView::new(line).size(style::SMALL_SIZE).color(if wrong { style::c(p.failure) } else { sec }).cut(true).height(DETAIL_ROW)));
            }
        }

        // 3. Codex client row: quiet healthy row (no permanent Restart button, no right-hand dot!).
        // Show Restart inline ONLY when daemon needs attention (stale / misrouted); otherwise in Machines menu.
        let codex_on = uses.is_some_and(|c| c[client_index(Provider::Codex)]);
        let daemon = running.and_then(|r| r.daemon.as_ref());
        let needs_restart = daemon.is_some_and(|d| d.stale || (codex_on && d.connections.direct > 0 && d.connections.pool == 0));
        let codex_check =
            CheckBox::new("Codex", codex_on).height(if needs_restart && !narrow && !compact_h { m::CONTROL_HEIGHT } else { m::MARK_ROW }).enabled(uses.is_some()).on_toggle(|on: &bool, _| {
                if !demo_enabled() {
                    olo_kit::proxy().send(Msg::SetClient(Provider::Codex, *on));
                }
            });

        let mut codex_block = div().w_full().flex().flex_col().gap(px(2.0));
        match daemon {
            None => {
                codex_block = codex_block.child(
                    div().w_full().flex().flex_row().items_center().justify_between().gap(px(12.0)).child(div().flex_1().min_w_0().child(codex_check)).child(status_cell("Stopped".to_owned(), sec)),
                );
            }
            Some(d) => {
                // Consistent noun matching Claude Code above ("1 running", "2 running", "Idle")
                let conn_count = match d.connections.pool + d.connections.direct {
                    0 => "Idle".to_owned(),
                    n => format!("{n} running"),
                };
                if needs_restart && !narrow && !compact_h {
                    codex_block = codex_block.child(
                        div().w_full().h(px(m::CONTROL_HEIGHT)).flex().flex_row().items_center().gap(px(8.0)).child(div().flex_1().min_w_0().child(codex_check)).child(
                            Button::new(if self.restarting_codex { "Restarting…" } else { "Restart" })
                                .id("pool-restart-codex")
                                .enabled(!self.restarting_codex)
                                .on_click(cx.msg::<ClickEvent>(WinMsg::RestartCodex)),
                        ),
                    );
                } else {
                    codex_block = codex_block.child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(12.0))
                            .child(div().flex_1().min_w_0().child(codex_check))
                            .child(status_cell(conn_count, if needs_restart { style::c(p.failure) } else { sec })),
                    );
                }
                if !tight_h {
                    if d.stale {
                        codex_block = codex_block.child(
                            div()
                                .w_full()
                                .pl(px(INDENT))
                                .child(StringView::new("Started before pool settings changed · restart recommended").size(style::SMALL_SIZE).color(style::c(p.failure)).wrap(true)),
                        );
                    } else if codex_on && d.connections.direct > 0 && d.connections.pool == 0 {
                        codex_block = codex_block
                            .child(div().w_full().pl(px(INDENT)).child(StringView::new("Service bypassing pool · restart recommended").size(style::SMALL_SIZE).color(style::c(p.failure)).wrap(true)));
                    } else if let Ok(threads) = &d.threads
                        && !threads.is_empty()
                    {
                        for th in threads {
                            let title = if th.title.is_empty() { "A conversation".to_owned() } else { format!("“{}”", th.title) };
                            let line = match th.cwd.as_deref() {
                                Some(dir) => {
                                    let f = folder(dir);
                                    let place = if narrow { middle_truncate(&f, 18) } else { f };
                                    format!("{place} · since {} · {title}", clock(d.started))
                                }
                                None => format!("since {} · {title}", clock(d.started)),
                            };
                            codex_block = codex_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(line).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)));
                        }
                    } else {
                        codex_block = codex_block.child(
                            div()
                                .w_full()
                                .pl(px(INDENT))
                                .child(StringView::new(format!("Service running · since {}", clock(d.started))).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW)),
                        );
                    }
                    if let Some((msg, err)) = &self.codex_note {
                        codex_block =
                            codex_block.child(div().w_full().pl(px(INDENT)).child(StringView::new(msg.clone()).size(style::SMALL_SIZE).color(if *err { style::c(p.failure) } else { sec }).wrap(true)));
                    }
                    if needs_restart && (narrow || compact_h) {
                        codex_block = codex_block.child(
                            div().w_full().pt(px(4.0)).flex().justify_end().child(
                                Button::new(if self.restarting_codex { "Restarting…" } else { "Restart" })
                                    .id("pool-restart-codex")
                                    .enabled(!self.restarting_codex)
                                    .on_click(cx.msg::<ClickEvent>(WinMsg::RestartCodex)),
                            ),
                        );
                    }
                }
            }
        }

        // 4. Antigravity client row + running `agy` processes (matches Claude Code above)
        let agy_on = uses.is_some_and(|c| c[client_index(Provider::Antigravity)]);
        let agy_procs = running.map(|r| r.agy.as_slice()).unwrap_or(&[]);
        let agy_count = match agy_procs.len() {
            0 => "Idle".to_owned(),
            n => format!("{n} running"),
        };
        let mut agy_block = div().w_full().flex().flex_col().gap(px(2.0)).child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .child(div().flex_1().min_w_0().child(CheckBox::new("Antigravity", agy_on).enabled(uses.is_some()).on_toggle(|on: &bool, _| {
                    if !demo_enabled() {
                        olo_kit::proxy().send(Msg::SetClient(Provider::Antigravity, *on));
                    }
                })))
                .child(status_cell(agy_count, sec)),
        );
        if !tight_h {
            for proc in agy_procs {
                let Process { started, cwd, connections, .. } = proc;
                let raw_place = cwd.as_deref().map(folder).unwrap_or_else(|| "?".into());
                let place = if narrow { middle_truncate(&raw_place, 18) } else { raw_place };
                let wrong = agy_on && connections.direct > 0 && connections.pool == 0;
                let line = if wrong { format!("{place} · bypassing pool · since {}", clock(*started)) } else { format!("{place} · since {}", clock(*started)) };
                agy_block = agy_block
                    .child(div().w_full().pl(px(INDENT)).child(StringView::new(line).size(style::SMALL_SIZE).color(if wrong { style::c(p.failure) } else { sec }).cut(true).height(DETAIL_ROW)));
            }
        }

        let mut content = div().w_full().flex().flex_col().gap(px(if tight_h { 4.0 } else { 7.0 })).child(div().w_full().pb(px(if tight_h { 1.0 } else { 5.0 })).child(gw_block));
        if !tight_h {
            content = content.child(use_pool_hdr);
        }
        content = content.child(claude_block).child(codex_block).child(agy_block);
        if let Some(note) = &self.told.note {
            content = content.child(StringView::new(note.text.clone()).size(style::SMALL_SIZE).color(if note.error { style::c(p.failure) } else { sec }).wrap(true));
        }

        div().w_full().flex().flex_col().gap(px(6.0)).child(hdr).child(content).into_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn remote_host_row(&self, s: &Stands, kept: &[String], selected: bool, wide: f32, hosts_col_w: f32, p: &olo_kit::palette::Palette, cx: &mut Cx<Self>) -> Element {
        let sec = secondary_ink(p, p.list_background);
        let (lamp, state) = host_stands(s);
        // Busy indicator uses crisp neutral 3/4-arc spinner (never hollow "off" circle or orange!)
        let lamp_el = lamp.map_or_else(|| spinner_arc(LIGHT), light);
        let asking = self.host_asking_remove.as_deref() == Some(s.id.as_str());

        // State-driven quiet row buttons: ONLY show a row button when action is actually needed!
        // Healthy hosts have ZERO noisy buttons (double-click / footer / right-click drive actions).
        let row_btn: Option<Button> = if asking || s.doing.is_some() || (s.hopeless && !s.wanted) {
            None
        } else if s.wanted && s.error.is_some() {
            Some(Button::new("Retry Now").id(format!("pool-host-{}-retry", s.id)).min_width(wide).on_click(cx.msg::<ClickEvent>(WinMsg::HostDo(s.id.clone(), "sync"))))
        } else if s.wanted && !s.in_step {
            Some(Button::new("Sync Now").id(format!("pool-host-{}-sync", s.id)).min_width(wide).on_click(cx.msg::<ClickEvent>(WinMsg::HostDo(s.id.clone(), "sync"))))
        } else if !s.wanted {
            Some(
                Button::new(if s.error.is_some() { "Try Again" } else { "Install" })
                    .id(format!("pool-host-{}-install", s.id))
                    .min_width(wide)
                    .enabled(self.hosts.carried() || demo_enabled())
                    .on_click(cx.msg::<ClickEvent>(WinMsg::HostDo(s.id.clone(), "install"))),
            )
        } else {
            None
        };

        // No redundant cluster icon on every row: status dot in 13pt gutter + 5pt gap = 18pt left edge,
        // so host name and sub-lines share ONE crisp alignment matching Gateway above!
        let state_color = if s.error.is_some() && s.doing.is_none() { style::c(p.failure) } else { sec };
        let title_group = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(5.0))
            .child(div().w(px(13.0)).flex_shrink_0().flex().justify_center().child(lamp_el))
            .child(StringView::new(s.name.clone()).bold(true).height(DETAIL_ROW))
            .child(StringView::new("·").color(sec).height(DETAIL_ROW))
            .child(div().flex_1().min_w_0().child(StringView::new(state).color(state_color).cut(true).height(DETAIL_ROW)));

        // Width-driven layout: keep action button inline whenever the hosts column has ≥360pt
        // (including horizontal third!), wrapping only in genuinely narrow columns.
        let narrow_row = hosts_col_w < 360.0;
        let line1 = div().w_full().h(px(if narrow_row { 20.0 } else { m::CONTROL_HEIGHT })).flex().flex_row().items_center().child(title_group);
        let (right_btn, wrapped_btn) = match row_btn {
            Some(btn) if narrow_row => (None, Some(btn)),
            Some(btn) => (Some(btn), None),
            None => (None, None),
        };

        // Sub-lines aligned straight under the host name (18pt exact!)
        let mut under = div().w_full().pl(px(18.0)).flex().flex_col().gap(px(3.0));
        if asking {
            under = under.child(div().pt(px(4.0)).child(warning_confirm(
                p,
                olo_kit::tr!("Remove Token Pool from {}?", s.name).to_string(),
                "Claude\u{a0}Code, Codex, and Antigravity on this host will return to their own sign-ins.".into(),
                Some("Any sign-ins kept there are copied back to This Computer first.".into()),
                Button::new("Cancel").id(format!("pool-host-{}-cancel", s.id)).on_click(cx.msg::<ClickEvent>(WinMsg::HostAskRemove(None))),
                Button::new("Remove").id(format!("pool-host-{}-confirm-remove", s.id)).on_click(cx.msg::<ClickEvent>(WinMsg::HostDo(s.id.clone(), "remove"))),
            )));
        } else {
            under = under.child(StringView::new(s.address.clone()).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW));
            if s.doing.is_some() {
                under = under.child(div().w_full().h(px(14.0)).flex().items_center().child(barber_bar(if narrow_row { 140.0 } else { 200.0 }, 8.0, neutral_bar_color(p))));
            } else {
                let refused = self.host_refused.as_ref().filter(|(id, _)| *id == s.id).map(|(_, why)| why.clone());
                match (refused.or_else(|| s.error.clone()), s.wanted) {
                    // Red ONLY on the dot & "Can't sync"; explanation stays readable secondary text with consistent punctuation!
                    (Some(why), true) => {
                        let msg = if why.contains("sign-in") { why } else { format!("{} · Olo keeps trying.", host_brief_sentence(&why)) };
                        under = under.child(StringView::new(msg).size(style::SMALL_SIZE).color(sec).wrap(true));
                    }
                    (Some(why), false) => {
                        under = under.child(StringView::new(why).size(style::SMALL_SIZE).color(sec).wrap(true));
                    }
                    (None, _) => {
                        if let Some(status) = &s.status {
                            let summary = host_summary(status, kept, s.heard_at, hosts_col_w < 460.0);
                            under = under.child(StringView::new(summary).size(style::SMALL_SIZE).color(sec).cut(true).height(DETAIL_ROW));
                        }
                    }
                }
            }
            if let Some(btn) = wrapped_btn {
                under = under.child(div().w_full().pt(px(6.0)).flex().justify_end().child(btn));
            }
        }

        // Clear selection tint when inline Remove confirmation is open (no yellow card on blue tint!)
        let sel_tint = if selected && !asking {
            Some(if p.mode == olo_theme::ThemeMode::Dark {
                style::c(olo_theme::mix(p.list_background, p.keyboard_navigation, 0.16))
            } else {
                style::c(olo_theme::mix(p.list_background, p.keyboard_navigation, 0.09))
            })
        } else {
            None
        };
        let id_click = s.id.clone();
        let can_jump = s.wanted || s.status.is_some();
        let id_ctx = s.id.clone();
        let content_col = div().flex_1().min_w_0().flex().flex_col().child(line1).child(under);
        let mut row = div()
            .w_full()
            .px(px(10.0))
            .py(px(ROW_PAD))
            .flex()
            .flex_row()
            .items_start()
            .gap(px(m::SPACING))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, e: &olo_kit::MouseDownEvent, cx| {
                    if e.click_count >= 2 && can_jump {
                        cx.send(WinMsg::ViewHostActivity(id_click.clone()));
                    } else {
                        cx.send(WinMsg::SelectHost(id_click.clone()));
                    }
                }),
            )
            .on_mouse_down(MouseButton::Right, cx.listener(move |_, e: &olo_kit::MouseDownEvent, cx| cx.send(WinMsg::HostContextMenu(id_ctx.clone(), e.position))))
            .child(content_col);
        if let Some(btn) = right_btn {
            row = row.child(btn);
        }
        if let Some(bg) = sel_tint {
            row = row.bg(bg);
        }
        row.into_element()
    }
}

impl View for PoolWindow {
    type Message = WinMsg;

    fn message(&mut self, message: WinMsg, cx: &mut Cx<Self>) {
        match message {
            WinMsg::Changed => {}
            WinMsg::Told(told) => {
                if told == self.told {
                    return;
                }
                // An account just signed in: chosen, and brought into
                // view, so the note and its row say the same.
                if told.joined != self.told.joined
                    && let Some((id, _)) = &told.joined
                {
                    self.selected_account = Some(id.clone());
                    *self.reveal.borrow_mut() = Some(Reveal::Asked(id.clone()));
                }
                self.told = told;
                self.count_down(cx);
            }
            WinMsg::Tick => self.schedule_tick(cx),
            WinMsg::Countdown => {
                self.counting = false;
                self.count_down(cx);
            }
            WinMsg::HostsTick => {
                if !demo_enabled() {
                    self.hosts.refresh();
                    cx.after(HOSTS_POLL_EVERY, WinMsg::HostsTick);
                }
            }
            WinMsg::Tab(tab) => self.switch_tab(tab, cx),
            WinMsg::SelectAccount(id) => self.selected_account = Some(id),
            WinMsg::SelectHost(id) => self.selected_host = Some(id),
            WinMsg::ViewHostActivity(host_id) => {
                if !host_id.is_empty() {
                    self.scope = MachineScope::Host(host_id);
                    self.activity_picked = None;
                    self.switch_tab(Tab::Activity, cx);
                }
            }
            WinMsg::ScopeIndex(idx) => {
                let hosts = if demo_enabled() { demo_data().hosts } else { self.hosts.list() };
                let opts = Self::scope_options(&hosts);
                if let Some((sc, _)) = opts.get(idx) {
                    self.scope = sc.clone();
                    self.activity_picked = None;
                }
            }
            WinMsg::Span(i) => self.span_idx = i.min(SPANS.len() - 1),
            WinMsg::Pick(pick) => {
                self.activity_picked = if self.activity_picked.as_ref() == Some(&pick) { None } else { Some(pick) };
            }
            WinMsg::ClearPick => self.activity_picked = None,
            WinMsg::AddMenu(at) => {
                let chosen = cx.msg_with(|command: &str| WinMsg::Account(String::new(), command.to_owned()));
                menu::popup(cx, add_menu(self.told.waiting.is_some()), at, Corner::BottomLeft, chosen);
                return;
            }
            WinMsg::AccountMenu(id, at) => {
                let pool = if demo_enabled() { demo_data().pool } else { self.shared.snapshot() };
                let Some(a) = pool.account(&id).cloned() else { return };
                let m = Self::account_menu_items(&a, self.told.waiting.is_some());
                let chosen = cx.msg_with(move |command: &str| WinMsg::Account(id.clone(), command.to_owned()));
                menu::popup(cx, m, at, Corner::BottomLeft, chosen);
                return;
            }
            WinMsg::AccountContextMenu(id, pos) => {
                self.selected_account = Some(id.clone());
                let pool = if demo_enabled() { demo_data().pool } else { self.shared.snapshot() };
                let Some(a) = pool.account(&id).cloned() else { return };
                let m = Self::account_menu_items(&a, self.told.waiting.is_some());
                let chosen = cx.msg_with(move |command: &str| WinMsg::Account(id.clone(), command.to_owned()));
                menu::context_menu(cx, m, pos, chosen);
                return;
            }
            WinMsg::HostContextMenu(id, pos) => {
                self.selected_host = Some(id.clone());
                let hosts = if demo_enabled() { demo_data().hosts } else { self.hosts.list() };
                let Some(h) = hosts.iter().find(|x| x.id == id) else { return };
                let mut m = Menu::new(&h.name);
                if h.wanted || h.status.is_some() {
                    m = m.item("Show Activity", "activity");
                }
                if h.wanted && h.doing.is_none() {
                    m = m.item("Sync Now", "sync").separator().item("Remove…", "remove");
                } else if !h.wanted && !h.hopeless && h.doing.is_none() {
                    m = m.item("Install Token Pool", "install");
                }
                let chosen = cx.msg_with(move |cmd: &str| match cmd {
                    "activity" => WinMsg::ViewHostActivity(id.clone()),
                    "sync" => WinMsg::HostDo(id.clone(), "sync"),
                    "install" => WinMsg::HostDo(id.clone(), "install"),
                    "remove" => WinMsg::HostAskRemove(Some(id.clone())),
                    _ => WinMsg::Changed,
                });
                menu::context_menu(cx, m, pos, chosen);
                return;
            }
            WinMsg::Account(id, command) if id.is_empty() => olo_kit::proxy().send(Msg::Command(command)),
            WinMsg::Account(id, command) => self.account_command(&id, &command, cx),
            WinMsg::Renamed(rename) => {
                if let Some((id, field)) = self.renaming.take()
                    && rename
                {
                    let name = field.read(cx).text().trim().to_owned();
                    if !name.is_empty() && !demo_enabled() {
                        self.shared.rename(&id, &name);
                    }
                }
            }
            WinMsg::RemoveAccount(id) => {
                if let Some(id) = id
                    && !demo_enabled()
                {
                    let name = self.shared.snapshot().account(&id).map(|a| a.name.clone()).unwrap_or_default();
                    olo_kit::proxy().send(Msg::Done(self.shared.remove(&id).map(|()| format!("{name} left the pool."))));
                }
                self.removing_account = None;
            }
            WinMsg::ResetCredit(id) => {
                if let Some(id) = id
                    && !demo_enabled()
                {
                    let name = self.shared.snapshot().account(&id).map(|a| a.name.clone()).unwrap_or_default();
                    let (shared, proxy) = (self.shared.clone(), olo_kit::proxy());
                    self.shared.spawn(async move {
                        let r = shared.redeem(&id).await.map(|()| format!("{name}’s quota was reset."));
                        proxy.send(Msg::Done(r));
                    });
                }
                self.resetting_credit = None;
            }
            WinMsg::FoundRunning(r) => self.running = Some(*r),
            WinMsg::RestartCodex => {
                if !demo_enabled() {
                    self.restarting_codex = true;
                    self.codex_note = None;
                    cx.work(|| codex_daemon::restart_now().map_err(|e| e.to_string()), WinMsg::CodexRestarted);
                }
            }
            WinMsg::CodexRestarted(r) => {
                self.restarting_codex = false;
                self.codex_note = Some(match r {
                    Ok(pid) => (format!("Restarted Codex service (pid {pid})."), false),
                    Err(e) => (e, true),
                });
            }
            WinMsg::HostAskRemove(host) => self.host_asking_remove = host,
            WinMsg::HostDo(id, what) => {
                self.host_asking_remove = None;
                if !demo_enabled() {
                    self.host_refused = self.hosts.act(&id, what).err().map(|why| (id, why));
                }
            }
            WinMsg::SyncAllHosts => {
                if !demo_enabled() {
                    olo_kit::proxy().send(Msg::Command("sync-hosts".into()));
                }
            }
        }
        cx.invalidate();
    }

    fn command(&mut self, name: &str, cx: &mut Cx<Self>) -> bool {
        match name {
            "close" => cx.close_window(),
            "refresh" => {
                if !demo_enabled() {
                    self.shared.sync_soon(None);
                    self.hosts.refresh();
                }
            }
            "capture" => {
                if !demo_enabled() {
                    self.shared.capture(true);
                }
            }
            "restart-codex" => {
                self.message(WinMsg::RestartCodex, cx);
            }
            "show-host-activity" => {
                let hosts = if demo_enabled() { demo_data().hosts } else { self.hosts.list() };
                let ordered = sorted_hosts(&hosts);
                let sel = self.selected_host.as_deref().filter(|id| ordered.iter().any(|h| h.id == *id)).or_else(|| ordered.iter().find(|h| h.wanted || h.status.is_some()).map(|h| h.id.as_str()));
                if let Some(id) = sel.map(str::to_owned) {
                    self.message(WinMsg::ViewHostActivity(id), cx);
                }
            }
            "tab-accounts" => {
                self.switch_tab(Tab::Accounts, cx);
                cx.invalidate();
            }
            "tab-activity" | "clients" => {
                self.switch_tab(Tab::Activity, cx);
                cx.invalidate();
            }
            "tab-machines" | "hosts" => {
                self.switch_tab(Tab::Machines, cx);
                cx.invalidate();
            }
            _ => olo_kit::proxy().send(Msg::Command(name.to_owned())),
        }
        true
    }

    fn window_closing(&mut self, _: &mut Cx<Self>) {
        self.stop_scanner();
        olo_kit::proxy().send(Msg::Closed);
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        // Opened at a size it was left at before, no taller than the screen
        // has room for now: a window kept at 740 points ran off the
        // smallest screen, though it opens at 720 (Astra's review,
        // 2026-11-28). Its width, and its height on a taller screen, as
        // they were; asked once, as it first shows.
        // (Never one app_server holds the size of, a tile or a zoom: its
        // size is the tile's, and the ask was never answered. The room of
        // the screen it's on, not of the shortest: a window kept taller on
        // a taller screen stays so; Neon's review, 2026-11-28.)
        if !self.sized {
            self.sized = true;
            let vp = cx.viewport_size();
            let room = (olo_kit::display_height(cx) - 64.0).max(240.0);
            if !cx.held_state().held() && f32::from(vp.height) > room + 0.5 {
                cx.resize_window(olo_kit::size(vp.width, px(room)));
            }
        }
        let p = theme::palette();
        let p = &*p;
        let (pool, activity, hosts, running, uses, unaccepted) = self.snapshot();
        let lights = tab_lights(&pool, &hosts, running.as_ref(), uses);

        let page = match self.tab {
            Tab::Accounts => self.accounts_tab(&pool, &hosts, &unaccepted, p, cx),
            Tab::Activity => self.activity_tab(&pool, &activity, &hosts, p, cx),
            Tab::Machines => self.machines_tab(&pool, &hosts, running.as_ref(), uses, p, cx),
        };

        let tabs = TabView::new("pool-tabs", vec!["Accounts".into(), "Activity".into(), "Machines".into()], self.tab as usize)
            .lights(lights)
            .on_select(cx.msg_with(|i: &usize| WinMsg::Tab(Tab::from_index(*i))))
            .child(page);

        let vp = cx.viewport_size();
        let bar = MenuBar::of(self, cx, |v, _| v.menus());
        let mut content = div().flex_1().min_h_0().p(px(outer_pad(vp))).flex().flex_col().gap(px(10.0));
        if let Some(err) = pool.error.as_deref() {
            // (Its height kept: Activity's page is its tab's less it.)
            let alert = gateway_stopped_alert(p, self.effective_port(), err, cx);
            content = content.child(div().w_full().child(alert).on_bounds_changed(cx.listener(|v: &mut PoolWindow, b: &Bounds<Pixels>, cx| {
                let h = f32::from(b.size.height);
                if (v.alert_h - h).abs() > 0.5 {
                    v.alert_h = h;
                    cx.invalidate();
                }
            })));
        }
        content = content.child(div().flex_1().min_h_0().child(tabs));
        div().size_full().flex().flex_col().child(bar).child(content)
    }
}

pub fn start(app: &mut AppCx, hub: Hub, service: bool) {
    let restored: Option<serde_json::Value> = olo_kit::restored_state().and_then(|s| serde_json::from_str(&s).ok());
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_tab = requested_tab(&args);
    let restored_tab = restored.as_ref().and_then(|r| {
        if let Some(i) = r["tab"].as_u64() {
            Some(Tab::from_index(i as usize))
        } else if r["hosts"] == true {
            Some(Tab::Machines)
        } else if r["clients"] == true {
            Some(Tab::Activity)
        } else {
            None
        }
    });
    let open = match &restored {
        Some(r) => r["open"] == true || r["hosts"] == true || r["clients"] == true,
        None => !service || arg_tab.is_some(),
    };
    let (shared, flags, proxy) = (hub.shared.clone(), hub.open.clone(), hub.proxy.clone());
    let hub = Rc::new(RefCell::new(hub));
    if open {
        hub.borrow_mut().open_tab(arg_tab.or(restored_tab));
    }
    olo_kit::message_handler(app, move |message, _| hub.borrow_mut().message(message));
    let saved = flags.clone();
    olo_kit::on_upgrade(move |_| Some(saved.state()));

    let due = shared.clone();
    olo_kit::on_upgrade_due(move || {
        if hand_over(&due, &flags, &proxy) {
            return;
        }
        due.drain.begin();
        let weak = Arc::downgrade(&due);
        let _ = std::thread::Builder::new().name("pool-drain".into()).spawn(move || {
            while let Some(s) = weak.upgrade() {
                if !s.drain.draining() {
                    break;
                }
                if s.drain.overdue() {
                    s.drain.end();
                    break;
                }
                s.nudge();
                drop(s);
                std::thread::sleep(Duration::from_millis(500));
            }
        });
    });

    olo_kit::upgrade_waits_while(move || {
        if shared.drain.retired() {
            return true;
        }
        let b = shared.drain.busy();
        if !b && shared.drain.draining() {
            shared.drain.hand_over();
        }
        b
    });
}

pub const KEPT_ENV: &str = "OLO_POOL_KEPT";
pub const KEEPER_ENV: &str = "OLO_POOL_KEEPER";

fn hand_over(shared: &Arc<Shared>, open: &Open, proxy: &Proxy) -> bool {
    use std::os::unix::process::CommandExt as _;
    let state = open.state();
    let Some(listeners) = shared.drain.for_a_copy() else { return false };
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = std::path::PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(std::env::args_os().skip(1)).env(olo_token_pool::drain::LISTENERS_ENV, listeners).env(olo_xdg::NEW_INSTANCE, "1").env(KEPT_ENV, "1");
    let finishing = shared.drain.finishing();
    match &finishing {
        Some(fd) => cmd.env(olo_token_pool::drain::FINISHING_ENV, std::os::fd::AsRawFd::as_raw_fd(fd).to_string()),
        None => cmd.env(olo_token_pool::drain::PREDECESSOR_ENV, std::process::id().to_string()),
    };
    olo_kit::restore::hand_over(&mut cmd, Some(&state));
    olo_kit::instance::close();
    let claims = shared.claims_closed(Duration::from_secs(120));
    if claims.is_none() {
        tracing::warn!("pool: a reset claim is still under way after two minutes; handing over beside it");
    }
    shared.flush();
    let started = cmd.spawn();
    drop(finishing);
    shared.drain.sealed();
    let child = match started {
        Ok(c) => c,
        Err(e) => {
            shared.drain.finished();
            tracing::warn!("pool: couldn’t start the new version beside this one ({e}); upgrading the old way");
            olo_kit::instance::listen(crate::APP_ID, proxy.clone());
            return false;
        }
    };
    tracing::info!(pid = child.id(), "pool: the new version takes every new request; finishing what this one has");
    shared.drain.retire();
    drop(claims);
    proxy.send(Msg::CloseWindows);
    let weak = Arc::downgrade(shared);
    let kept = std::env::var_os(KEPT_ENV).is_some();
    let _ = std::thread::Builder::new().name("pool-handed-over".into()).spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        loop {
            match weak.upgrade() {
                Some(s) if s.drain.busy() => {
                    drop(s);
                    std::thread::sleep(Duration::from_millis(250));
                }
                _ => break,
            }
        }
        olo_kit::restore::mark_waiting(false);
        if let Some(s) = weak.upgrade() {
            s.drain.finished();
        }
        if kept {
            tracing::info!("pool: finished; the new version has it all");
            olo_kit::restore::mark_waiting(false);
            std::process::exit(0);
        }
        tracing::info!("pool: finished; staying as the keeper of the new version");
        let e = std::process::Command::new(&exe).env(KEEPER_ENV, "1").env_remove(KEPT_ENV).exec();
        tracing::warn!("pool: couldn’t become the keeper ({e}); ending");
        std::process::exit(0);
    });
    drop(child);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Machines' two columns only where the hosts have a list's room: a
    /// quarter (530 × 357) and a horizontal third, not a short third
    /// (346 × 400, 349 × 357: the hosts were 70 points wide) or 450.
    #[test]
    fn machines_stack_where_two_columns_dont_fit() {
        for (w, h) in [(346.0, 400.0), (349.0, 357.0), (450.0, 357.0), (346.0, 220.0), (640.0, 712.0), (1104.0, 760.0)] {
            assert!(!machines_two_col(w, h), "{w} × {h}");
        }
        for (w, h) in [(530.0, 357.0), (1104.0, 233.0), (560.0, 400.0)] {
            assert!(machines_two_col(w, h), "{w} × {h}");
        }
    }

    /// Activity, short and in one column: the chart over Recent Requests
    /// where both have their least (a quarter, 530 × 357, the plot tall
    /// enough for its axes); shorter, down to the window's least of 220,
    /// the table alone, its least inside the page. The gateway's
    /// "stopped" box and a chosen request's details are budgeted too.
    #[test]
    fn activity_fits_a_short_window() {
        let vp = |w: f32, h: f32| olo_kit::size(px(w), px(h));
        let quarter = page_height(vp(530.0, 357.0));
        assert_eq!(quarter, 357.0 - 26.0 - 20.0 - 22.0 - 32.0);
        let (one_line, table) = (DETAIL_ROW, RECENT_LEAST);
        let fits = |page: f32, plot: f32, legend: f32, recent: f32| m::CONTROL_HEIGHT + 8.0 + chart_least(plot, legend) + 8.0 + recent <= page;
        assert!(activity_chart_fits(quarter, one_line, table));
        assert_eq!(activity_plot_least(quarter, one_line, table), GRAPH_AXES, "a quarter's plot has its axes");
        // Everything inside the page: the pop-ups, the chart at its least
        // with its axes, the table at its least, 8 apart.
        assert!(fits(quarter, GRAPH_AXES, one_line, table));
        let least = page_height(vp(346.0, 220.0));
        assert!(!activity_chart_fits(least, one_line, table), "220 tall: the table alone");
        assert!(m::CONTROL_HEIGHT + 8.0 + table <= least, "the table's least inside the page");
        // Where the chart just fits, its plot as short as it goes.
        let just = m::CONTROL_HEIGHT + 8.0 + chart_least(PLOT_LEAST, one_line) + 8.0 + table;
        assert!(activity_chart_fits(just, one_line, table) && !activity_chart_fits(just - 1.0, one_line, table));
        assert_eq!(activity_plot_least(just, one_line, table), PLOT_LEAST);
        // A narrow third, its legend on two lines: the plot gives way first.
        let two = wrapped_height(&[120.0, 150.0, 110.0], 305.0, 12.0);
        assert_eq!(two, 2.0 * DETAIL_ROW + 12.0);
        let third = page_height(vp(349.0, 357.0));
        let plot = activity_plot_least(third, two, table);
        assert!(fits(third, plot, two, table) || !activity_chart_fits(third, two, table));
        assert_eq!(wrapped_height(&[120.0, 150.0], 300.0, 12.0), DETAIL_ROW, "one line where they fit");
        // The page's width and height as the window's parts leave them.
        assert_eq!(page_width(vp(349.0, 357.0)), 349.0 - 12.0 - 32.0);

        // A request chosen in a quarter: its details' row kept under the
        // table, the plot shorter for it (not the table under a row).
        let picked = table + PICKED_ROW;
        assert!(activity_chart_fits(quarter, one_line, picked));
        let plot = activity_plot_least(quarter, one_line, picked);
        assert_eq!(plot, PLOT_LEAST);
        assert!(fits(quarter, plot, one_line, picked), "the chart and the table with its details inside a quarter");
        // The gateway stopped, its box over the tabs (112 points as first
        // guessed, or as laid out): in a quarter, the table alone, inside
        // what's left; at the window's least, nothing past the page.
        for alert in [Some(0.0), Some(130.0)] {
            let page = activity_page(vp(530.0, 357.0), alert);
            assert!(page < quarter - 100.0);
            assert!(!activity_chart_fits(page, one_line, table), "{alert:?}: no chart under the box");
            for needs in [table, picked] {
                assert!(m::CONTROL_HEIGHT + 8.0 + recent_kept(needs, page) <= page, "{alert:?}: the table inside what's under the box");
            }
        }
        assert_eq!(recent_kept(table, activity_page(vp(530.0, 357.0), Some(0.0))), table, "a line's box: the table's whole least");
        assert_eq!(activity_page(vp(530.0, 357.0), None), quarter);
        let squeezed = activity_page(vp(346.0, 220.0), Some(0.0));
        assert!(!activity_chart_fits(squeezed, one_line, picked));
        assert_eq!(recent_kept(picked, squeezed), 0.0, "the table's least never more than the page");
    }

    /// One point a stretch, at its middle: the same at every scale.
    #[test]
    fn a_series_is_drawn_by_its_stretches_middles() {
        let steps: VecDeque<(f64, f64)> = [(0.0, 1.0), (59.999, 1.0), (60.0, 3.0), (119.999, 3.0)].into_iter().collect();
        assert_eq!(midpoints(&steps), VecDeque::from([(29.9995, 1.0), (89.9995, 3.0)]));
    }

    /// An account's facts drop whole from the end, never cut in two.
    #[test]
    fn facts_drop_whole() {
        let facts: Vec<String> = ["alex@gmail.example", "AI Ultra plan", "2 banked resets"].map(str::to_owned).into();
        let width = |t: &str| t.chars().count() as f32 * 6.0;
        let all = facts.join(" · ");
        assert_eq!(facts_shown(&facts, width(&all), width), all);
        assert_eq!(facts_shown(&facts, width(&all) - 1.0, width), "alex@gmail.example · AI Ultra plan…");
        assert_eq!(facts_shown(&facts, 20.0 * 6.0, width), "alex@gmail.example…");
        // Not even the first: all, for the line to cut where a word ends.
        assert_eq!(facts_shown(&facts, 30.0, width), all);
    }

    /// A separate sign-in's result is taken only while it's the one waited
    /// for (as `Msg::SignedIn` asks, before anything of it is used: no
    /// account added or signed in again, no note over another's): dropped
    /// once cancelled, or once another sign-in began, for a new account or
    /// for one signed in again alike.
    #[test]
    fn only_the_sign_in_waited_for_is_taken() {
        let wait = |target: Option<&str>| Waiting { provider: Provider::Antigravity, target: target.map(str::to_owned), cancel: Arc::default(), until: Instant::now() + SIGN_IN_WAIT };
        let current = |w: &Option<Waiting>| w.as_ref().map(|w| w.cancel.clone());
        // Sign In Again in Terminal… for "acc-a": its result taken.
        let mut waiting = Some(wait(Some("acc-a")));
        let a = current(&waiting).unwrap();
        assert!(waited_for(current(&waiting).as_ref(), &a));
        // Another begun (`sign_in_separately` marks the first): the first's
        // result, read just before, dropped; the new one's taken.
        a.store(true, Ordering::Relaxed);
        waiting = Some(wait(None));
        let b = current(&waiting).unwrap();
        assert!(!waited_for(current(&waiting).as_ref(), &a));
        assert!(waited_for(current(&waiting).as_ref(), &b));
        // Cancel Sign-In (marked, none waited for): dropped.
        b.store(true, Ordering::Relaxed);
        waiting = None;
        assert!(!waited_for(current(&waiting).as_ref(), &b));
        // Unmarked but not the one waited for: dropped too.
        waiting = Some(wait(Some("acc-b")));
        assert!(!waited_for(current(&waiting).as_ref(), &Arc::new(AtomicBool::new(false))));
    }

    #[test]
    fn signed_in_notes_say_what_to_do() {
        let n = Note::signed_in("Workspace Dev", false, true);
        assert_eq!((n.text.as_str(), n.name_len, n.error), ("Workspace Dev joined. You can quit agy.", 13, false));
        assert_eq!(Note::signed_in("Pat", true, true).text, "Pat signed in. You can quit agy.");
        assert_eq!(Note::signed_in("Pat", false, false).text, "Pat joined the pool.");
        assert_eq!(Note::signed_in("Pat", true, false).text, "Pat is signed in again.");
        let plain: Note = ("Syncing 1 remote host…".to_owned(), false).into();
        assert_eq!(plain.name_len, 0);
    }

    /// Cut for the footer's room: an account's name before what follows it,
    /// and nothing rather than a lone "…". Measured as if each character
    /// were a point wide, cut as the kit cuts (with "…", where a word ends
    /// if asked).
    #[test]
    fn a_note_cut_keeps_what_follows_a_name() {
        let width = |t: &str| t.chars().count() as f32;
        let cut = |t: &str, w: f32, words: bool| {
            let max = (w.max(0.0) as usize).saturating_sub(1);
            let mut end = t.char_indices().map(|(i, _)| i).rfind(|&i| t[..i].chars().count() <= max).unwrap_or(0);
            if words {
                end = t[..end].rfind(' ').unwrap_or(0);
            }
            format!("{}…", t[..end].trim_end())
        };
        let n = Note::signed_in("Workspace Development", false, true);
        assert_eq!(fitted(&n, 60.0, width, cut).as_deref(), Some(n.text.as_str()));
        assert_eq!(fitted(&n, 35.0, width, cut).as_deref(), Some("Workspac… joined. You can quit agy."));
        assert_eq!(fitted(&n, 20.0, width, cut), None);
        let e: Note = ("Your keyring is locked, and Antigravity keeps its sign-in in it.".to_owned(), true).into();
        assert_eq!(fitted(&e, 30.0, width, cut).as_deref(), Some("Your keyring is locked, and…"));
        assert_eq!(fitted(&e, 3.0, width, cut), None);
        // Never "… joined": a letter of the name at least, the whole in the
        // room (Neon's review, 2026-11-28: with a point for the name, it
        // was "… joined. You can quit agy.").
        let rest = " joined. You can quit agy.".chars().count() as f32;
        for room in [rest, rest + 0.5, rest + 1.0, rest + 1.5] {
            assert_eq!(fitted(&n, room, width, cut), None, "{room}");
        }
        assert_eq!(fitted(&n, rest + 2.0, width, cut).as_deref(), Some("W… joined. You can quit agy."));
        // The wait: the same words at every width, its time last.
        let w = Note::waiting(Provider::Antigravity, "4:12");
        assert_eq!(w.text, "Waiting for agy in Terminal… 4:12 left");
        assert_eq!(&w.text[w.quiet_from.unwrap()..], "4:12 left");
        assert_eq!(Note::waiting(Provider::Claude, "19:59").text, "Waiting for claude in Terminal… 19:59 left");
        assert_eq!(fitted(&w, 60.0, width, cut).as_deref(), Some("Waiting for agy in Terminal… 4:12 left"));
    }

    /// The wait counts down in minutes and seconds, rounded up, and never
    /// below nothing.
    #[test]
    fn the_wait_counts_down() {
        let now = Instant::now();
        assert_eq!(left(now + Duration::from_secs(20 * 60), now), "20:00");
        assert_eq!(left(now + Duration::from_millis(252_400), now), "4:13");
        assert_eq!(left(now + Duration::from_secs(252), now), "4:12");
        assert_eq!(left(now + Duration::from_secs(9), now), "0:09");
        assert_eq!(left(now, now + Duration::from_secs(5)), "0:00");
    }

    /// A sign-in's result is taken only if it's the latest begun for its
    /// account, whichever way either was begun and whatever order they're
    /// answered in; a new account's always is (Astra's review,
    /// 2026-11-28).
    #[test]
    fn only_the_latest_sign_in_for_an_account_is_taken() {
        let mut ops = Ops::default();
        let local = ops.begin(Some("acc-a"));
        let separate = ops.begin(Some("acc-a"));
        let fresh = ops.begin(None);
        // The local one answered last (its keyring prompt left open):
        // dropped; the separate one's taken; a new account's either way.
        assert!(!ops.current(local, Some("acc-a")));
        assert!(ops.current(separate, Some("acc-a")));
        assert!(ops.current(fresh, None));
        // Another account's, begun meanwhile, its own.
        let b = ops.begin(Some("acc-b"));
        assert!(ops.current(b, Some("acc-b")) && ops.current(separate, Some("acc-a")));
        // As they're taken in: the newest for the account found, whichever
        // way it was begun. One added that turned out to be acc-a (its
        // email), begun after the separate one: it's acc-a's from then on,
        // and the separate one, ending later, isn't taken.
        assert!(!ops.take(local, "acc-a"));
        assert!(ops.take(fresh, "acc-a") && ops.latest_for(fresh, "acc-a"));
        assert!(!ops.take(separate, "acc-a") && !ops.latest_for(separate, "acc-a"));
        // One added before a sign-in again of the account it turns out to
        // be, ending after it: not taken.
        let older = ops.begin(None);
        let again = ops.begin(Some("acc-b"));
        assert!(ops.take(again, "acc-b"));
        assert!(!ops.take(older, "acc-b"));
        // A new account's, its own.
        assert!(ops.take(older, "acc-new") && ops.latest_for(older, "acc-new"));
    }

    /// One whose save is tried again (the vault not open) and that's
    /// removed meanwhile: what's said of it ends with the failure, never
    /// "joined" once a save goes through without it; its copy let go
    /// (Astra's and Neon's reviews, 2026-11-28).
    #[test]
    fn one_removed_while_its_save_is_tried_again_is_said_nothing_of() {
        use olo_token_pool::{model::Credential, store::Saved, upstream::Upstreams};
        let mut saved = Saved::default();
        saved.accounts.push(Account::new("acc-p".into(), "Pat".into(), Provider::Antigravity));
        saved.credentials.insert("acc-p".into(), Credential { access: "a".into(), refresh: None, expires_at: None, account_id: None });
        let shared = Shared::new(saved, None, Upstreams::fake("http://127.0.0.1:9"), || {});
        shared.pool.lock().unwrap().loaded = false;
        let (said, discarded) = (std::cell::RefCell::new(Vec::new()), Cell::new(0));
        // (Removed while it waited to try again, the vault open since.)
        let wait = || {
            shared.pool.lock().unwrap().loaded = true;
            shared.remove("acc-p").unwrap();
        };
        keep_taken(&shared, "acc-p", || discarded.set(discarded.get() + 1), |unsaved| said.borrow_mut().push(unsaved), wait);
        let said = said.into_inner();
        assert_eq!(said.len(), 1, "said after it was removed: {said:?}");
        assert!(said[0].as_deref().is_some_and(|e| e.contains("aren’t open")), "{said:?}");
        assert_eq!(discarded.get(), 1);
        // Saved at once: said so, and its copy let go, once.
        let mut saved = Saved::default();
        saved.accounts.push(Account::new("acc-q".into(), "Quinn".into(), Provider::Antigravity));
        let shared = Shared::new(saved, None, Upstreams::fake("http://127.0.0.1:9"), || {});
        let (said, discarded) = (std::cell::RefCell::new(Vec::new()), Cell::new(0));
        keep_taken(&shared, "acc-q", || discarded.set(discarded.get() + 1), |unsaved| said.borrow_mut().push(unsaved), || panic!("waited"));
        assert_eq!((said.into_inner(), discarded.get()), (vec![None], 1));
    }

    /// Sign-ins taken in one after another, in the order they came, each
    /// as its account's: the account found and its sign-in written under
    /// one lock, so two for one new email make one account, and of two
    /// for one account the newest begun stays, whichever ends first
    /// (Astra's and Neon's reviews, 2026-11-28: each looked for its account
    /// on its own and was written later, making two of one, and an older
    /// over a newer).
    #[test]
    fn sign_ins_are_taken_in_order_as_their_accounts() {
        use olo_token_pool::{model::Credential, store::Saved, upstream::Upstreams};
        let cred = |t: &str| Credential { access: t.into(), refresh: Some(format!("r-{t}")), expires_at: Some(now() + 3600), account_id: None };
        let mut saved = Saved::default();
        let mut a = Account::new("acc-a".into(), "A".into(), Provider::Antigravity);
        a.email = Some("a@example.com".into());
        saved.accounts.push(a);
        saved.credentials.insert("acc-a".into(), cred("first"));
        let shared = Shared::new(saved, None, Upstreams::fake("http://127.0.0.1:9"), || {});
        let ops = std::sync::Mutex::new(Ops::default());
        let said = |email: &str| SignedInAs { email: Some(email.into()), separate: true, ..Default::default() };
        let new = |email: &str| {
            let mut a = Account::new(store::new_id(), email.into(), Provider::Antigravity);
            a.email = Some(email.into());
            a
        };
        let take = |op: u64, target: Option<&str>, email: &str, token: &str| shared.take_sign_in(target, new(email), cred(token), said(email), |id| ops.lock().unwrap().take(op, id)).unwrap();
        // Two for one new email: one account.
        // (One at a time: a guard lives to its statement's end.)
        let one = ops.lock().unwrap().begin(None);
        let two = ops.lock().unwrap().begin(None);
        let first = take(one, None, "new@example.com", "n1").unwrap();
        let second = take(two, None, "new@example.com", "n2").unwrap();
        assert!(!first.2 && second.2 && first.0 == second.0, "two accounts of one: {first:?} {second:?}");
        let p = shared.snapshot();
        assert_eq!(p.saved.accounts.iter().filter(|a| a.email.as_deref() == Some("new@example.com")).count(), 1);
        assert_eq!(p.saved.credentials[&first.0].access, "n2");
        // Add Account (acc-a's email), then Sign In Again for acc-a; the
        // newer taken first, the older after: the newer stays.
        let added = ops.lock().unwrap().begin(None);
        let again = ops.lock().unwrap().begin(Some("acc-a"));
        assert!(take(again, Some("acc-a"), "a@example.com", "again").is_some());
        assert!(take(added, None, "a@example.com", "added").is_none(), "an older sign-in taken over a newer");
        assert_eq!(shared.snapshot().saved.credentials["acc-a"].access, "again");
        // The other way round: Sign In Again, then Add Account, the newer
        // (the add) first.
        let again = ops.lock().unwrap().begin(Some("acc-a"));
        let added = ops.lock().unwrap().begin(None);
        assert!(take(added, None, "a@example.com", "added-2").is_some());
        assert!(take(again, Some("acc-a"), "a@example.com", "again-2").is_none(), "an older sign-in again taken over a newer add");
        assert_eq!(shared.snapshot().saved.credentials["acc-a"].access, "added-2");
        assert_eq!(shared.snapshot().saved.accounts.len(), 2);
        // One for an account removed as it was read: said, nothing kept.
        let gone = ops.lock().unwrap().begin(Some("acc-gone"));
        assert!(shared.take_sign_in(Some("acc-gone"), new("g@example.com"), cred("g"), said("g@example.com"), |id| ops.lock().unwrap().take(gone, id)).unwrap_err().contains("left the pool"));
        assert!(!shared.snapshot().saved.credentials.contains_key("acc-gone"));
    }

    /// Sign-ins read while the accounts can't be opened wait, every one,
    /// in the order they came, until the accounts are open; a failure to
    /// open them since is said once each time (Astra's and Neon's reviews,
    /// 2026-11-28: a second replaced the first, and the note said
    /// "Opening…" for ever).
    #[test]
    fn sign_ins_wait_in_order_for_the_accounts() {
        let read = |op: u64| Read { found: Err(format!("{op}")), target: None, op, separate: None, unlocked: false };
        let mut held = Held::default();
        held.hold(read(1), 3);
        held.hold(read(2), 3);
        let (taken, failed) = held.looked(false, 3);
        assert!(taken.is_empty() && !failed);
        let (taken, failed) = held.looked(false, 4);
        assert!(taken.is_empty() && failed, "a failure to open them not said");
        assert!(!held.looked(false, 4).1, "said twice");
        held.hold(read(3), 4);
        let (taken, failed) = held.looked(true, 5);
        assert_eq!(taken.iter().map(|r| r.op).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(!failed && held.is_empty());
    }

    /// Where the list's pinned band can end on a row: from its top to its
    /// title's glyphs, between each two of its lines' glyphs, and from its
    /// last line's to its end; past a line of a height not known, its
    /// padding alone.
    #[test]
    fn a_rows_lines_are_known_to_the_band() {
        // Facts, two quotas, Today: 11 + 24, then 16, 18, 18, 18 with 3
        // between (a row of 125).
        let row = [Some(FACT_LINE), Some(QUOTA_LINE), Some(QUOTA_LINE), Some(TODAY_LINE)];
        let c = row_cuts((5.0, 4.0), &row);
        assert_eq!(c.from_top, [(0.0, 16.0), (31.0, 37.0), (50.0, 57.0), (70.0, 78.0), (91.0, 100.0)]);
        assert_eq!(c.end, 12.0);
        // A signed-out row: its button fills its title's line; a wrapped
        // reason after its facts, nothing past that but its end's padding.
        let c = row_cuts((0.0, 0.0), &[Some(FACT_LINE), None, Some(QUOTA_LINE)]);
        assert_eq!((c.from_top, c.end), (vec![(0.0, 11.0), (35.0, 37.0), (50.0, 54.0)], ROW_PAD));
    }

    /// The joined row brought into view with the band's edge between two
    /// lines, never through one: up into the row's own end by at most 7
    /// points, else down to the next; and not moved at all when it shows.
    #[test]
    fn a_row_is_revealed_with_no_line_cut_under_the_band() {
        // A band of 22, then two rows of 125 (the second under its rule,
        // its end at 273), a 170-point view: the last row's end shows only
        // at the far end (103), where the band's edge, at 125, would cut
        // the first row's Today line (its words from 122 to 135).
        let items = [(0.0, 22.0), (22.0, 147.0), (148.0, 273.0)];
        let row = row_cuts((5.0, 4.0), &[Some(FACT_LINE), Some(QUOTA_LINE), Some(QUOTA_LINE), Some(TODAY_LINE)]);
        let ruled = Clear { from_top: row.from_top.iter().map(|&(s, e)| (if s > 0.0 { s + 1.0 } else { s }, e + 1.0)).collect(), end: row.end };
        let clear = [Clear { from_top: vec![(0.0, 0.0)], end: 0.0 }, row.clone(), ruled];
        let o = reveal_offset((148.0, 273.0), 0.0, 170.0, 103.0, BAND, &items, &clear).unwrap();
        // Up to where the Today line's words begin (122): 3 points of the
        // row's own end under the view, 8 left under its last words.
        assert_eq!(o + BAND, 122.0);
        assert!(273.0 - (ROW_PAD - 4.0) <= o + 170.0, "the row's last line cut: {o}");
        // Where the edge is clear already (a 160-point view: at 135, in the
        // first row's end), as far as it goes.
        assert_eq!(reveal_offset((148.0, 273.0), 0.0, 160.0, 113.0, BAND, &items, &clear), Some(113.0));
        // Shown already: not moved.
        assert_eq!(reveal_offset((22.0, 147.0), 0.0, 400.0, 0.0, BAND, &items, &clear), None);
        // Nothing clear within reach: as far as it shows the row.
        let far = [(0.0, 22.0), (22.0, 400.0), (400.0, 525.0)];
        let lone = [Clear::default(), Clear { from_top: vec![(0.0, 11.0)], end: 11.0 }, row.clone()];
        assert_eq!(reveal_offset((400.0, 525.0), 0.0, 160.0, 365.0, BAND, &far, &lone), Some(365.0));
        // With room for it, the ledge (the band's last point) is kept 7
        // points over the first line under it: an edge that fell a point
        // into the first row's Today line (its words from 122) went up to
        // their top, under the ledge, and goes to the gap over them now,
        // the row still shown whole (Opus's and Neon's reviews, 2026-11-28).
        let items = [(0.0, 22.0), (22.0, 147.0), (148.0, 273.0), (274.0, 399.0)];
        let clear = [Clear { from_top: vec![(0.0, 0.0)], end: 0.0 }, row.clone(), clear[2].clone(), clear[2].clone()];
        let o = reveal_offset((148.0, 273.0), 0.0, 180.0, 219.0, BAND, &items, &clear).unwrap();
        assert_eq!(o, 92.0);
        assert!(122.0 - (o + BAND + 1.0) >= UNDER_LEDGE, "the line under the ledge {} points from it", 122.0 - (o + BAND + 1.0));
        assert!(o + BAND >= 113.0, "the quota line over it cut");
        assert!(273.0 - (ROW_PAD - 4.0) <= o + 180.0, "the row's last line cut: {o}");
    }

    /// Set small on the panel, a note is darker than other secondary words:
    /// 5.5:1 or more there, an error's red 4.5:1, in either theme.
    #[test]
    fn notes_are_inked_to_be_read_on_the_panel() {
        for p in [olo_kit::palette::Palette::light(), olo_kit::palette::Palette::dark()] {
            assert!(olo_theme::contrast(note_ink(&p, false), p.panel_background) >= 5.5, "{:?}", p.mode);
            assert!(olo_theme::contrast(note_ink(&p, true), p.panel_background) >= 4.5, "{:?}", p.mode);
        }
    }
}

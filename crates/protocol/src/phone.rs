//! The phone: an iPhone's or an Android phone's messages, contacts and
//! one-time codes, spoken to by Olo itself over Bluetooth
//! (`olo_system::phone`: the Message Access and Phone Book Access
//! profiles; nothing else is installed). What `olo-phone --service` knows
//! (a JSON file in the runtime directory, rewritten on every change,
//! which Deskbar's tray and Preferences ▸ Phone follow; **nothing a
//! message says is ever in it**), Olo's own settings for it, what may be
//! asked of the service over its socket ([`Request`], one JSON line each
//! way: the conversations and sending among it, for Phone the app and the
//! command line), and what the service and a browser's extension host say
//! to each other about a one-time code ([`ToBrowser`], [`FromBrowser`]).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where the service's state and socket are: the runtime directory, or
/// `OLO_PHONE_DIR` (tests: a service kept apart from the person's).
fn runtime_dir() -> PathBuf {
    std::env::var_os("OLO_PHONE_DIR").or_else(|| std::env::var_os("XDG_RUNTIME_DIR")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// What kind of phone it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    IPhone,
    /// Android, or anything else that speaks the profiles.
    Android,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::IPhone => "iPhone",
            Kind::Android => "phone",
        }
    }
}

/// How the phone stands with this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Link {
    /// No phone is paired with this computer.
    #[default]
    NotPaired,
    /// Paired, and not in reach (off, away, its Bluetooth off): tried
    /// again by itself.
    Away,
    /// In reach: its messages being opened.
    Connecting,
    /// The phone has to allow it: its own switch for this computer
    /// (an iPhone's Show Notifications; an Android's message access).
    Allow,
    /// Messages come and go.
    Ready,
    /// Another program has the phone's profiles (BlueFerry's obexd):
    /// Olo can't open them until it lets go.
    Taken,
}

impl Link {
    /// In a few words, for a status line.
    pub fn words(self) -> &'static str {
        match self {
            Link::NotPaired => "No phone paired",
            Link::Away => "Out of reach",
            Link::Connecting => "Connecting…",
            Link::Allow => "Waiting for the phone to allow it",
            Link::Ready => "Connected",
            Link::Taken => "Another program has the phone",
        }
    }

    pub fn connected(self) -> bool {
        self == Link::Ready
    }
}

/// What the phone is playing (its own player, which this computer is a
/// remote control of).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Music {
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
}

impl Music {
    /// "So What — Miles Davis" (the title alone, with no artist).
    pub fn line(&self) -> String {
        if self.artist.is_empty() { self.title.clone() } else { format!("{} \u{2014} {}", self.title, self.artist) }
    }
}

/// What a remote control does to the phone's music.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MusicAction {
    Play,
    Pause,
    /// Play if it's paused, pause if it's playing.
    Toggle,
    Next,
    Previous,
}

impl MusicAction {
    pub const ALL: [MusicAction; 5] = [MusicAction::Play, MusicAction::Pause, MusicAction::Toggle, MusicAction::Next, MusicAction::Previous];

    pub fn id(self) -> &'static str {
        match self {
            MusicAction::Play => "play",
            MusicAction::Pause => "pause",
            MusicAction::Toggle => "toggle",
            MusicAction::Next => "next",
            MusicAction::Previous => "previous",
        }
    }

    pub fn parse(id: &str) -> Option<MusicAction> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// The paired phone's own picture: the nearest of the phones drawn
/// (`data/icons/svg/phone-*.svg`: the ten commonest iPhones and the ten
/// commonest Android phones) to what the phone says it is, as an icon's
/// name; `phone` for one that matches none (Ben, 2026-11-16: "show the
/// closest matching icon for a paired phone based on what it is").
///
/// An iPhone says its model only over low energy (`model`: its Device
/// Information's "iPhone17,3"; its Bluetooth name is its owner's), so
/// one that hasn't said is the commonest iPhone. Any other phone is
/// told by its Bluetooth name, which is its model's until its owner
/// changes it ("Galaxy S26 Ultra", "Pixel 10 Pro", "SM-A566B").
pub fn picture(kind: Kind, model: &str, name: &str) -> &'static str {
    if kind == Kind::IPhone || model.starts_with("iPhone") || name.to_lowercase().contains("iphone") {
        let numbers = model.strip_prefix("iPhone").and_then(|m| m.split_once(',')).and_then(|(major, minor)| Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?)));
        return match numbers {
            Some((18, 1)) => "phone-iphone-17-pro",
            Some((18, 2)) => "phone-iphone-17-pro-max",
            Some((18, 4)) => "phone-iphone-air",
            Some((18, 5)) => "phone-iphone-17e",
            Some((17, 1)) => "phone-iphone-16-pro",
            Some((17, 2)) => "phone-iphone-16-pro-max",
            Some((17, 5)) => "phone-iphone-16e",
            Some((17, _)) => "phone-iphone-16",
            // (The 15 Pro and Pro Max, and the 14's: the same three
            // lenses in a square as the 16 Pro's.)
            Some((16, 2) | (15, 3)) => "phone-iphone-16-pro-max",
            Some((16, _) | (15, 2)) => "phone-iphone-16-pro",
            // (Older: two lenses set diagonally, as the 15's.)
            Some((major, _)) if major <= 15 => "phone-iphone-15",
            // (The 17, one newer than any drawn, and one that hasn't
            // said: the commonest.)
            _ => "phone-iphone-17",
        };
    }
    let name = name.to_lowercase();
    let has = |word: &str| name.contains(word);
    // Samsung's own model numbers ("SM-S948B": S, the line; 9x8, an
    // Ultra; the second digit of an A is its tier).
    if let Some(code) = name.split(|c: char| !c.is_ascii_alphanumeric() && c != '-').find_map(|w| w.strip_prefix("sm-")) {
        let c: Vec<char> = code.chars().collect();
        return match (c.first(), c.get(1), c.get(2), c.get(3)) {
            (Some('s'), Some('9'), Some('4'), Some('8')) => "phone-galaxy-s26-ultra",
            (Some('s'), Some('9'), _, Some('8')) => "phone-galaxy-s25-ultra",
            (Some('s'), ..) => "phone-galaxy-s26",
            (Some('a'), Some('0'), ..) => "phone-galaxy-a07",
            (Some('a'), Some('3' | '5' | '7'), ..) => "phone-galaxy-a56",
            (Some('a'), ..) => "phone-galaxy-a17",
            _ => "phone",
        };
    }
    // A Galaxy by its name: the line's letter and number.
    let galaxy = |line: char| -> Option<u32> {
        name.split(|c: char| !c.is_ascii_alphanumeric()).find_map(|w| {
            let digits = w.strip_prefix(line)?;
            (!digits.is_empty() && digits.len() <= 2 && digits.chars().all(|c| c.is_ascii_digit())).then(|| digits.parse().ok()).flatten()
        })
    };
    if let Some(s) = galaxy('s').filter(|_| has("galaxy") || has("ultra") || !has("phone")) {
        return match (has("ultra"), s) {
            (true, 26..) => "phone-galaxy-s26-ultra",
            (true, _) => "phone-galaxy-s25-ultra",
            _ => "phone-galaxy-s26",
        };
    }
    if let Some(a) = galaxy('a').filter(|_| has("galaxy")) {
        return match a {
            0..=9 => "phone-galaxy-a07",
            30..=79 => "phone-galaxy-a56",
            _ => "phone-galaxy-a17",
        };
    }
    if has("galaxy note") {
        return "phone-galaxy-s25-ultra";
    }
    if has("galaxy") {
        return "phone-galaxy-s26";
    }
    if has("pixel") {
        return if has("pro") || has("fold") { "phone-pixel-10-pro" } else { "phone-pixel-10" };
    }
    if has("redmi note") || has("poco") || has("xiaomi") {
        return "phone-redmi-note-14";
    }
    if has("redmi") {
        return "phone-redmi-a5";
    }
    "phone"
}

impl PhoneState {
    /// Its own picture ([`picture`]).
    pub fn picture(&self) -> &'static str {
        picture(self.kind, &self.model, &self.name)
    }
}

/// How a call taken on this computer stands (never who it's with).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallState {
    /// Ringing, to be answered or declined.
    Ringing,
    /// Made from here, not yet answered there.
    Calling,
    Active,
    Held,
}

impl CallState {
    pub fn words(self) -> &'static str {
        match self {
            CallState::Ringing => "Incoming call",
            CallState::Calling => "Calling…",
            CallState::Active => "On a call",
            CallState::Held => "On hold",
        }
    }
}

/// What the service knows. No sender, number, text or code is here: who
/// reads this file learns that a message came, never what it said.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PhoneState {
    /// The service's process (a state file left by one gone isn't it).
    pub pid: u32,
    /// The phone paired, as it's named ("Mira's iPhone"; empty: none).
    pub name: String,
    pub kind: Kind,
    /// Its object with BlueZ (to pair again, to forget it).
    pub path: String,
    pub link: Link,
    /// Its contacts came too (names for numbers).
    pub contacts: bool,
    /// The phone's battery, in percent, when it says.
    pub battery: Option<u8>,
    /// Conversations kept, and messages in them not yet read.
    pub conversations: u32,
    pub unread: u32,
    /// One-time codes that have arrived since the service started (the
    /// count alone), and when the latest did (Unix seconds) while it can
    /// still be used: Deskbar's menu offers to copy it.
    pub codes: u64,
    pub code_at: Option<u64>,
    /// Why it isn't working, when it's something to say plainly.
    pub trouble: String,
    /// Olo's profiles are registered with the BlueZ running now (what
    /// the phone's messages and contacts come through; Olo Doctor says
    /// when they aren't).
    pub profiles: bool,
    /// How many times the conversations or the contacts have changed
    /// (the count alone): who shows them asks again when it moves.
    pub changes: u64,
    /// Since when (Unix seconds) the link has stood as it does: a phone
    /// connected whose messages have long not come is Olo Doctor's to say.
    pub since: u64,
    /// What the phone is playing or last played, while it's connected
    /// and has said (none: nothing, or no phone).
    pub music: Option<Music>,
    /// Its other notifications come too (an iPhone's, over low energy).
    pub others: bool,
    /// What it says it is ("iPhone17,3"), once it has said.
    pub model: String,
    /// A call ringing or on, taken through this computer.
    pub call: Option<CallState>,
}

impl PhoneState {
    pub fn path() -> PathBuf {
        runtime_dir().join("olo-phone.json")
    }

    pub fn socket() -> PathBuf {
        runtime_dir().join("olo-phone.sock")
    }

    pub fn load() -> PhoneState {
        let state: PhoneState = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if state.pid != 0 && !std::path::Path::new(&format!("/proc/{}", state.pid)).exists() {
            return PhoneState::default();
        }
        state
    }

    /// Write it (atomically).
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        let tmp = path.with_extension("json.part");
        std::fs::write(&tmp, serde_json::to_vec(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(&tmp, &path)
    }

    /// A phone is paired.
    pub fn paired(&self) -> bool {
        self.link != Link::NotPaired
    }

    /// In a line, for Deskbar's tip and the command line.
    pub fn words(&self) -> String {
        let mut s = self.link.words().to_owned();
        if self.link.connected() {
            if let Some(b) = self.battery {
                s.push_str(&format!(" · {b}%"));
            }
            if self.unread > 0 {
                s.push_str(&format!(" · {} unread", self.unread));
            }
        }
        s
    }
}

/// What happens to a code when the tab waiting for it is on an agent's
/// screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentCodes {
    /// It's filled in there, and the person told.
    #[default]
    Give,
    /// The person is asked first (a notification on their screen).
    Ask,
    /// Agents' tabs get none.
    Never,
}

impl AgentCodes {
    pub const ALL: [AgentCodes; 3] = [AgentCodes::Give, AgentCodes::Ask, AgentCodes::Never];

    pub fn label(self) -> &'static str {
        match self {
            AgentCodes::Give => "Fill It In",
            AgentCodes::Ask => "Ask Me First",
            AgentCodes::Never => "Never",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            AgentCodes::Give => "give",
            AgentCodes::Ask => "ask",
            AgentCodes::Never => "never",
        }
    }

    pub fn from_word(w: &str) -> Option<AgentCodes> {
        AgentCodes::ALL.into_iter().find(|a| a.word() == w)
    }
}

/// What the phone's messages and notifications are told of with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Notify {
    /// Each message, in a notification that can be answered in place.
    #[default]
    Messages,
    /// Nothing.
    Nothing,
}

impl Notify {
    pub const ALL: [Notify; 2] = [Notify::Messages, Notify::Nothing];

    pub fn label(self) -> &'static str {
        match self {
            Notify::Messages => "Messages",
            Notify::Nothing => "Nothing",
        }
    }
}

/// Olo's own settings for the phone (Preferences ▸ Phone;
/// `~/.config/olo/phone.json`, which the service follows).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhoneSettings {
    /// What is told of as it comes.
    pub notify: Notify,
    /// What the message says, in its notification (else only who sent it).
    pub show_text: bool,
    /// Recognise one-time codes in messages as they come.
    pub codes: bool,
    /// Offer a code in the browser tab that is waiting for one.
    pub codes_in_browser: bool,
    /// When that tab is on an agent's screen.
    pub agent_screens: AgentCodes,
    /// Copy a code no tab is waiting for to the clipboard.
    pub copy_codes: bool,
    /// Mail accounts (their addresses, `olo_mail::accounts`) not looked
    /// in for codes; every other one set up is, while a page waits for a
    /// code (Preferences ▸ Security ▸ Passwords).
    pub unwatched_mail: Vec<String>,
    /// Show the phone's other notifications here too (an iPhone's, of
    /// every app: mail, calls, the rest), with their actions.
    pub others: bool,
    /// Take calls here: a call answered or made on this computer has
    /// its sound here (else Answer answers it on the phone).
    pub calls: bool,
}

impl Default for PhoneSettings {
    fn default() -> PhoneSettings {
        PhoneSettings {
            notify: Notify::Messages,
            show_text: true,
            codes: true,
            codes_in_browser: true,
            agent_screens: AgentCodes::Give,
            copy_codes: false,
            unwatched_mail: Vec::new(),
            others: true,
            calls: true,
        }
    }
}

impl PhoneSettings {
    pub fn path() -> PathBuf {
        std::env::var_os("OLO_PHONE_SETTINGS").map(PathBuf::from).unwrap_or_else(|| olo_xdg::config_dir().join("phone.json"))
    }

    pub fn load() -> PhoneSettings {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.new");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }
}

/// What may be asked of the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "do")]
pub enum Request {
    /// Everything it knows (answered with the state).
    Status,
    /// The latest one-time code, while it can still be used (Deskbar's
    /// Copy Code). Refused to a program on an agent's screen.
    Code,
    /// The conversations, the latest first, each with its messages (for
    /// Phone the app and the command line). Refused to a program on an
    /// agent's screen, as everything a message says is.
    Conversations,
    /// The phone's contacts whose name or number has `query` in it (all,
    /// with none).
    Contacts {
        query: String,
    },
    /// Send `text` to `to` (a telephone number or an address; a
    /// conversation's `key` answers it).
    Send {
        to: String,
        text: String,
    },
    /// A conversation's messages read (on the phone too).
    MarkRead {
        key: String,
    },
    /// Try the phone again now.
    Reconnect,
    /// Play, pause or skip the phone's music.
    Music {
        action: MusicAction,
    },
    /// Call `number` through the phone, its sound here.
    Call {
        number: String,
    },
    /// Answer the call ringing, here; hang up (or decline) the call.
    Answer,
    HangUp,
    /// A browser's extension host, staying connected: from here on the
    /// connection carries [`ToBrowser`] one way and [`FromBrowser`] the
    /// other, a JSON line each.
    Browser {
        /// The agent's screen its Chrome is on (`OLO_SCREEN`), if it is.
        screen: Option<String>,
        /// Chrome's process.
        pid: u32,
    },
}

/// The service's answer to a [`Request`] (not to `Browser`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reply {
    pub ok: bool,
    pub error: Option<String>,
    pub state: Option<PhoneState>,
    /// `Code`'s: the code and who sent it.
    pub code: Option<String>,
    pub from: Option<String>,
    /// `Conversations`' and `Contacts`'.
    pub conversations: Option<Vec<Conversation>>,
    pub contacts: Option<Vec<Person>>,
}

/// A message in a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Message {
    /// The phone's handle for it (empty for one sent from here).
    pub handle: String,
    pub text: String,
    /// When (Unix seconds).
    pub at: i64,
    /// Sent from here.
    pub outgoing: bool,
    pub read: bool,
}

/// A conversation: everything to and from one number or address.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Conversation {
    /// What it's known by to the service (send to it to answer).
    pub key: String,
    /// Who: their name in the phone's contacts, else as the phone names
    /// them, else the number.
    pub name: String,
    /// Their number or address, as written.
    pub address: String,
    /// The oldest first.
    pub messages: Vec<Message>,
}

impl Conversation {
    pub fn unread(&self) -> usize {
        self.messages.iter().filter(|m| !m.outgoing && !m.read).count()
    }

    pub fn latest(&self) -> i64 {
        self.messages.last().map_or(0, |m| m.at)
    }
}

/// Someone in the phone's contacts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Person {
    pub name: String,
    pub phones: Vec<String>,
    pub emails: Vec<String>,
}

/// A tab with a field waiting for a one-time code, as its browser's
/// extension sees it: what the service chooses among.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Waiting {
    /// Chrome's ids for the tab and the frame the field is in.
    pub tab: i64,
    pub frame: i64,
    /// The frame's host ("accounts.example.com") and the tab's title.
    pub host: String,
    pub title: String,
    /// The field has the keyboard focus; its tab is the one showing in
    /// its window; that window is the one in front.
    pub focused: bool,
    pub active: bool,
    pub front: bool,
    /// The field says it takes a one-time code (`autocomplete=
    /// "one-time-code"`), rather than only seeming to.
    pub declared: bool,
    /// How many digits the field takes, if it says (its `maxlength`, or
    /// how many one-digit boxes there are).
    pub digits: Option<u32>,
    /// What the page says round the field: that the code was sent by
    /// text message (or to a phone), by mail, or that it comes from an
    /// authenticator app.
    pub by_text: bool,
    pub by_mail: bool,
    pub by_app: bool,
    /// How long ago the field was first seen there, in seconds.
    pub age: u64,
}

/// The service to a browser's extension host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "say")]
pub enum ToBrowser {
    /// A code has come (not said: which, or from whom): which tabs are
    /// waiting for one? Answered with [`FromBrowser::Waiting`].
    Any { id: u64 },
    /// This browser's tab gets it: shown under the field for the person
    /// to take, or (`fill`: an agent's screen) put in at once. `mail`:
    /// it came by mail, not in a text message (what the offer says).
    Code {
        id: u64,
        tab: i64,
        frame: i64,
        code: String,
        from: String,
        fill: bool,
        #[serde(default)]
        mail: bool,
    },
    /// This browser's tab is offered a link to sign in with, that came
    /// by mail (`to`: the host it goes to, said in the offer): the tab
    /// goes there when the person takes it. Never put in unasked.
    Link { id: u64, tab: i64, frame: i64, url: String, to: String, from: String },
    /// It went elsewhere, was taken or is too old: an offer still
    /// showing for it goes.
    Gone { id: u64 },
}

/// A browser's extension host to the service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "say")]
pub enum FromBrowser {
    /// The tabs waiting for a code now: the answer to `Any` (its `id`),
    /// or unasked (`id: None`: a code field has just come up or taken
    /// the focus, and a code may have come before it).
    Waiting { id: Option<u64>, tabs: Vec<Waiting> },
    /// What came of a `Code`: "filled" (the person took it, or it was
    /// put in), "shown", "dismissed" or "gone" (the tab or its field
    /// went).
    Outcome { id: u64, outcome: String },
}

/// Ask the service (blocks up to `timeout`; call off the UI thread).
pub fn ask(request: &Request, timeout: std::time::Duration) -> std::io::Result<Reply> {
    use std::io::{BufRead, Write};
    let stream = std::os::unix::net::UnixStream::connect(PhoneState::socket())?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut line = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    line.push(b'\n');
    (&stream).write_all(&line)?;
    let mut answer = String::new();
    std::io::BufReader::new(&stream).read_line(&mut answer)?;
    serde_json::from_str(&answer).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_said_reads_as_json() {
        let r = Request::Browser { screen: Some("agent".into()), pid: 7 };
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(text, r#"{"do":"browser","screen":"agent","pid":7}"#);
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), r);
        let c = ToBrowser::Code { id: 3, tab: 12, frame: 0, code: "482913".into(), from: "Acme".into(), fill: false, mail: false };
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"say":"code","id":3,"tab":12,"frame":0,"code":"482913","from":"Acme","fill":false,"mail":false}"#);
        // (One from a service before `mail`: a text message's.)
        assert_eq!(serde_json::from_str::<ToBrowser>(r#"{"say":"code","id":3,"tab":12,"frame":0,"code":"482913","from":"Acme","fill":false}"#).unwrap(), c);
        let l = ToBrowser::Link { id: 4, tab: 12, frame: 0, url: "https://acme.example/magic#t".into(), to: "acme.example".into(), from: "Acme".into() };
        assert_eq!(serde_json::to_string(&l).unwrap(), r#"{"say":"link","id":4,"tab":12,"frame":0,"url":"https://acme.example/magic#t","to":"acme.example","from":"Acme"}"#);
        // (A browser says only what it knows: the rest is as if not so.)
        let w: FromBrowser = serde_json::from_str(r#"{"say":"waiting","id":null,"tabs":[{"tab":4,"host":"example.com","focused":true}]}"#).unwrap();
        let FromBrowser::Waiting { id: None, tabs } = w else { panic!() };
        assert!(tabs[0].focused && !tabs[0].declared && tabs[0].digits.is_none());
    }

    /// The nearest picture to what a phone says it is.
    #[test]
    fn a_phones_own_picture() {
        let iphone = |model: &str| picture(Kind::IPhone, model, "Mira’s iPhone");
        assert_eq!(iphone("iPhone18,3"), "phone-iphone-17");
        assert_eq!(iphone("iPhone18,1"), "phone-iphone-17-pro");
        assert_eq!(iphone("iPhone18,2"), "phone-iphone-17-pro-max");
        assert_eq!(iphone("iPhone18,4"), "phone-iphone-air");
        assert_eq!(iphone("iPhone18,5"), "phone-iphone-17e");
        assert_eq!(iphone("iPhone17,3"), "phone-iphone-16");
        assert_eq!(iphone("iPhone17,4"), "phone-iphone-16", "the 16 Plus: as the 16");
        assert_eq!(iphone("iPhone17,1"), "phone-iphone-16-pro");
        assert_eq!(iphone("iPhone17,2"), "phone-iphone-16-pro-max");
        assert_eq!(iphone("iPhone17,5"), "phone-iphone-16e");
        assert_eq!(iphone("iPhone16,1"), "phone-iphone-16-pro", "the 15 Pro: the nearest");
        assert_eq!(iphone("iPhone16,2"), "phone-iphone-16-pro-max");
        assert_eq!(iphone("iPhone15,4"), "phone-iphone-15");
        assert_eq!(iphone("iPhone14,5"), "phone-iphone-15", "an older one");
        assert_eq!(iphone("iPhone19,3"), "phone-iphone-17", "a newer one than any drawn");
        assert_eq!(iphone(""), "phone-iphone-17", "one that hasn't said");
        assert_eq!(picture(Kind::Android, "iPhone17,3", "Phone"), "phone-iphone-16", "said, whatever it was taken for");
        let other = |name: &str| picture(Kind::Android, "", name);
        assert_eq!(other("Galaxy S26 Ultra"), "phone-galaxy-s26-ultra");
        assert_eq!(other("Jun’s Galaxy S24 Ultra"), "phone-galaxy-s25-ultra");
        assert_eq!(other("Galaxy S26+"), "phone-galaxy-s26");
        assert_eq!(other("Galaxy S23"), "phone-galaxy-s26");
        assert_eq!(other("Galaxy A56 5G"), "phone-galaxy-a56");
        assert_eq!(other("Galaxy A17"), "phone-galaxy-a17");
        assert_eq!(other("Galaxy A07"), "phone-galaxy-a07");
        assert_eq!(other("Galaxy Z Flip7"), "phone-galaxy-s26");
        assert_eq!(other("SM-S948B"), "phone-galaxy-s26-ultra");
        assert_eq!(other("SM-S938U1"), "phone-galaxy-s25-ultra");
        assert_eq!(other("Ada's SM-A566B"), "phone-galaxy-a56");
        assert_eq!(other("SM-A075F"), "phone-galaxy-a07");
        assert_eq!(other("Pixel 10"), "phone-pixel-10");
        assert_eq!(other("Pixel 9 Pro XL"), "phone-pixel-10-pro");
        assert_eq!(other("Redmi A5"), "phone-redmi-a5");
        assert_eq!(other("Redmi Note 14 Pro"), "phone-redmi-note-14");
        assert_eq!(other("POCO X7"), "phone-redmi-note-14");
        assert_eq!(other("Mira’s iPhone"), "phone-iphone-17", "an iPhone by its name, where nothing else is known");
        assert_eq!(other("Nothing Phone (3)"), "phone");
        assert_eq!(other("moto g power"), "phone");
        assert_eq!(other(""), "phone");
        // (Every picture named is one drawn: `data/icons/svg`.)
        let drawings = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/icons/svg");
        let drawn: Vec<String> =
            std::fs::read_dir(&drawings).unwrap().flatten().filter_map(|f| Some(f.path().file_stem()?.to_string_lossy().into_owned())).filter(|n| n.starts_with("phone-")).collect();
        for name in ["iPhone18,1", "iPhone18,2", "iPhone18,3", "iPhone18,4", "iPhone18,5", "iPhone17,1", "iPhone17,2", "iPhone17,3", "iPhone17,5", "iPhone15,4"]
            .map(iphone)
            .into_iter()
            .chain(["Galaxy S26 Ultra", "Galaxy S25 Ultra", "Galaxy S26", "Galaxy A56", "Galaxy A17", "Galaxy A07", "Pixel 10", "Pixel 10 Pro", "Redmi A5", "Redmi Note 14"].map(other))
        {
            assert!(drawn.iter().any(|d| d == name), "{name} isn't drawn");
        }
        assert_eq!(drawn.len(), 20);
        assert!(drawings.join("phone.svg").is_file(), "the phone that's none of them");
    }

    #[test]
    fn the_state_says_nothing_of_what_a_message_says() {
        let s = PhoneState { name: "Mira’s iPhone".into(), link: Link::Ready, battery: Some(81), unread: 2, codes: 1, code_at: Some(1_794_700_000), ..Default::default() };
        assert_eq!(s.words(), "Connected · 81% · 2 unread");
        assert!(s.paired() && !PhoneState::default().paired());
        let text = serde_json::to_string(&s).unwrap();
        let keys: Vec<String> = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text).unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            [
                "pid",
                "name",
                "kind",
                "path",
                "link",
                "contacts",
                "battery",
                "conversations",
                "unread",
                "codes",
                "code_at",
                "trouble",
                "profiles",
                "changes",
                "since",
                "music",
                "others",
                "model",
                "call"
            ]
        );
        // (What it's playing is there, when it plays: a song's name
        // isn't a message's.)
        let playing = Music { playing: true, title: "So What".into(), artist: "Miles Davis".into(), album: "Kind of Blue".into() };
        assert_eq!(playing.line(), "So What \u{2014} Miles Davis");
        assert_eq!(Music { title: "A Podcast".into(), ..Default::default() }.line(), "A Podcast");
        assert_eq!(serde_json::to_string(&Request::Music { action: MusicAction::Next }).unwrap(), r#"{"do":"music","action":"next"}"#);
        assert_eq!(MusicAction::parse("toggle"), Some(MusicAction::Toggle));
        assert_eq!(serde_json::to_string(&Request::Send { to: "+15550100142".into(), text: "hi".into() }).unwrap(), r#"{"do":"send","to":"+15550100142","text":"hi"}"#);
    }
}

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
    /// Show the phone's other notifications here too (an iPhone's, of
    /// every app: mail, calls, the rest), with their actions.
    pub others: bool,
    /// Take calls here: a call answered or made on this computer has
    /// its sound here (else Answer answers it on the phone).
    pub calls: bool,
}

impl Default for PhoneSettings {
    fn default() -> PhoneSettings {
        PhoneSettings { notify: Notify::Messages, show_text: true, codes: true, codes_in_browser: true, agent_screens: AgentCodes::Give, copy_codes: false, others: true, calls: true }
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
    /// text message (or to a phone), or that it comes from an
    /// authenticator app.
    pub by_text: bool,
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
    /// to take, or (`fill`: an agent's screen) put in at once.
    Code { id: u64, tab: i64, frame: i64, code: String, from: String, fill: bool },
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
        let c = ToBrowser::Code { id: 3, tab: 12, frame: 0, code: "482913".into(), from: "Acme".into(), fill: false };
        assert_eq!(serde_json::to_string(&c).unwrap(), r#"{"say":"code","id":3,"tab":12,"frame":0,"code":"482913","from":"Acme","fill":false}"#);
        // (A browser says only what it knows: the rest is as if not so.)
        let w: FromBrowser = serde_json::from_str(r#"{"say":"waiting","id":null,"tabs":[{"tab":4,"host":"example.com","focused":true}]}"#).unwrap();
        let FromBrowser::Waiting { id: None, tabs } = w else { panic!() };
        assert!(tabs[0].focused && !tabs[0].declared && tabs[0].digits.is_none());
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

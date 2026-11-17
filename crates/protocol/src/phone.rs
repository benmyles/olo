//! The phone: an iPhone's messages and one-time codes, through
//! BlueFerry's backend (github.com/erikwb/blueferry: MAP, PBAP and ANCS
//! over Bluetooth, a D-Bus API on the session bus; Olo Doctor installs
//! it). What `olo-phone --service` knows of it (a JSON file in the
//! runtime directory, rewritten on every change, which Deskbar's tray and
//! Preferences ▸ Phone follow; **nothing a message says is ever in it**),
//! Olo's own settings for it, what may be asked of the service over its
//! socket ([`Request`], one JSON line each way), and what the service and
//! a browser's extension host say to each other about a one-time code
//! ([`ToBrowser`], [`FromBrowser`]).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Where the service's state and socket are: the runtime directory, or
/// `OLO_PHONE_DIR` (tests: a service kept apart from the person's).
fn runtime_dir() -> PathBuf {
    std::env::var_os("OLO_PHONE_DIR").or_else(|| std::env::var_os("XDG_RUNTIME_DIR")).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// How the backend stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    /// Not installed (Olo Doctor offers it).
    #[default]
    Missing,
    /// Installed, not running.
    Stopped,
    /// Running, but it speaks another generation of its API than this
    /// Olo knows ([`API`]): one of them wants updating.
    Other,
    Running,
}

/// The generation of BlueFerry's messaging API this Olo speaks
/// (`GetStatus`'s `api_version`).
pub const API: u32 = 2;

/// How the phone stands with this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Link {
    /// No phone has been paired for messages.
    #[default]
    NotPaired,
    /// Starting, or opening its Bluetooth profiles.
    Connecting,
    /// Messages come and go.
    Ready,
    /// Some of it works (messages without contacts, say).
    Limited,
    /// The phone went (out of range, Bluetooth off): tried again by
    /// itself.
    Reconnecting,
    /// The phone has to allow it: its switches under Settings ▸
    /// Bluetooth ▸ ⓘ.
    Allow,
    /// The phone refused the messages' connection.
    Refused,
}

impl Link {
    /// From the backend's `connectivity_state`.
    pub fn of(state: &str) -> Link {
        match state {
            "ready" => Link::Ready,
            "degraded" => Link::Limited,
            "reconnecting" => Link::Reconnecting,
            "authorization-required" => Link::Allow,
            "map-connection-refused" => Link::Refused,
            _ => Link::Connecting,
        }
    }

    /// In a few words, for a status line.
    pub fn words(self) -> &'static str {
        match self {
            Link::NotPaired => "No phone paired",
            Link::Connecting => "Connecting…",
            Link::Ready => "Connected",
            Link::Limited => "Connected in part",
            Link::Reconnecting => "Out of reach",
            Link::Allow => "Waiting for the phone to allow it",
            Link::Refused => "The phone refused",
        }
    }

    pub fn connected(self) -> bool {
        matches!(self, Link::Ready | Link::Limited)
    }
}

/// What the service knows. No sender, number, text or code is here: who
/// reads this file learns that a message came, never what it said.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PhoneState {
    /// The service's process (a state file left by one gone isn't it).
    pub pid: u32,
    pub backend: Backend,
    /// The backend's release ("0.9.1-1.fc45").
    pub release: String,
    pub link: Link,
    /// Which of the phone's services are up: its messages (MAP), its
    /// contacts (PBAP), its notifications (ANCS).
    pub messages: bool,
    pub contacts: bool,
    pub notifications: bool,
    /// The phone's battery, in percent, when it says.
    pub battery: Option<u8>,
    /// Conversations kept, and messages in them not yet read.
    pub conversations: u32,
    pub unread: u32,
    /// The backend's store of messages: "ready", "locked",
    /// "unavailable", "disabled".
    pub storage: String,
    /// One-time codes that have arrived since the service started (the
    /// count alone), and when the latest did (Unix seconds) while it can
    /// still be used: Deskbar's menu offers to copy it.
    pub codes: u64,
    pub code_at: Option<u64>,
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

    /// In a line, for Deskbar's tip and the command line.
    pub fn words(&self) -> String {
        match self.backend {
            Backend::Missing => "The phone's backend isn't installed".into(),
            Backend::Stopped => "The phone's backend isn't running".into(),
            Backend::Other => "The phone's backend and Olo don't match: update them".into(),
            Backend::Running => {
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
    /// Each message, in a notification of Olo's own that can be
    /// answered in place (the backend's own popups off).
    #[default]
    Messages,
    /// Everything the phone notifies of, its other apps too, in the
    /// backend's own popups (it mirrors them as the phone shows them).
    Everything,
    /// Nothing.
    Nothing,
}

impl Notify {
    pub const ALL: [Notify; 3] = [Notify::Messages, Notify::Everything, Notify::Nothing];

    pub fn label(self) -> &'static str {
        match self {
            Notify::Messages => "Messages",
            Notify::Everything => "Everything on the Phone",
            Notify::Nothing => "Nothing",
        }
    }

    /// The backend's own policy that goes with it: Olo tells of messages
    /// itself, so the backend's popups are for everything or off.
    pub fn backend_policy(self) -> &'static str {
        match self {
            Notify::Everything => "all",
            Notify::Messages | Notify::Nothing => "none",
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
}

impl Default for PhoneSettings {
    fn default() -> PhoneSettings {
        PhoneSettings { notify: Notify::Messages, show_text: true, codes: true, codes_in_browser: true, agent_screens: AgentCodes::Give, copy_codes: false }
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
        let s = PhoneState { backend: Backend::Running, link: Link::Ready, battery: Some(81), unread: 2, codes: 1, code_at: Some(1_794_700_000), ..Default::default() };
        assert_eq!(s.words(), "Connected · 81% · 2 unread");
        let text = serde_json::to_string(&s).unwrap();
        let keys: Vec<String> = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text).unwrap().keys().cloned().collect();
        assert_eq!(keys, ["pid", "backend", "release", "link", "messages", "contacts", "notifications", "battery", "conversations", "unread", "storage", "codes", "code_at"]);
        assert_eq!(Link::of("authorization-required"), Link::Allow);
        assert_eq!(Link::of("something new"), Link::Connecting);
    }
}

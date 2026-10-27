//! A polkit authentication agent: the part of the desktop that asks for
//! a password when a program wants to do something privileged (set the
//! clock, reboot while others are logged in, …).
//!
//! [`Agent::register`] exports `org.freedesktop.PolicyKit1.AuthenticationAgent`
//! on the system bus and registers it with polkitd for a login session (or,
//! for tests, a single process). polkitd then calls `BeginAuthentication`;
//! the app is handed a [`Request`] and answers it with [`Request::finish`].
//! Proving who the user is goes through polkit's setuid helper
//! (`polkit-agent-helper-1`, [`Helper`]): only it may tell polkitd that a
//! cookie was authenticated, so every agent runs it and relays its PAM
//! conversation (prompts, fingerprint messages) to the user.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::Arc,
};

use zbus::blocking::Connection;
use zvariant::{OwnedValue, Value};

use crate::{Result, bus};

const AUTHORITY: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
const AGENT_PATH: &str = "/com/olodesktop/PolicyKit1/AuthenticationAgent";

/// Who the agent answers for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A logind session (the desktop's own).
    Session(String),
    /// One process (tests, like `pkttyagent --process`).
    Process { pid: u32, start_time: u64 },
}

impl Subject {
    /// `pid` as a polkit subject (its start time from /proc).
    pub fn process(pid: u32) -> Option<Subject> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Field 22, counting after the parenthesised command name.
        let start_time = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()?;
        Some(Subject::Process { pid, start_time })
    }

    fn to_dbus(&self) -> (&'static str, HashMap<&'static str, Value<'_>>) {
        match self {
            Subject::Session(id) => ("unix-session", HashMap::from([("session-id", Value::from(id.as_str()))])),
            Subject::Process { pid, start_time } => {
                ("unix-process", HashMap::from([("pid", Value::from(*pid)), ("start-time", Value::from(*start_time))]))
            }
        }
    }
}

/// Someone who may authenticate a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    User { uid: u32, name: String },
    Group { gid: u32, members: Vec<String> },
}

/// What polkitd asks the agent.
pub enum Event {
    Begin(Request),
    /// polkitd gave up on the request with this cookie (the program went
    /// away): close its window.
    Cancel(String),
}

/// One request for authentication. Dropping it unanswered cancels it.
pub struct Request {
    pub action_id: String,
    /// What the program wants to do, for the user ("Authentication is
    /// required to set the system time.").
    pub message: String,
    pub icon_name: String,
    pub details: Vec<(String, String)>,
    pub cookie: String,
    /// Users who may authenticate: the user's own name first when allowed.
    pub users: Vec<String>,
    reply: Option<async_channel::Sender<bool>>,
}

impl Request {
    /// Answer polkitd: `true` once the helper reported success (polkitd
    /// then has the result from the helper itself), `false` to cancel.
    pub fn finish(mut self, authenticated: bool) {
        if let Some(r) = self.reply.take() {
            let _ = r.try_send(authenticated);
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Some(r) = self.reply.take() {
            let _ = r.try_send(false);
        }
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Cancelled(String),
}

type Handler = Arc<dyn Fn(Event) + Send + Sync>;

struct AgentObject {
    on: Handler,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl AgentObject {
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        icon_name: String,
        details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> std::result::Result<(), AgentError> {
        let (tx, rx) = async_channel::bounded(1);
        let mut details: Vec<(String, String)> = details.into_iter().collect();
        details.sort();
        let users = users(&identities.iter().filter_map(|(kind, d)| identity(kind, d)).collect::<Vec<_>>());
        (self.on)(Event::Begin(Request { action_id, message, icon_name, details, cookie, users, reply: Some(tx) }));
        match rx.recv().await {
            Ok(true) => Ok(()),
            _ => Err(AgentError::Cancelled("Authentication was cancelled".into())),
        }
    }

    async fn cancel_authentication(&self, cookie: String) {
        (self.on)(Event::Cancel(cookie));
    }
}

fn identity(kind: &str, d: &HashMap<String, OwnedValue>) -> Option<Identity> {
    let id = |key: &str| d.get(key).and_then(|v| u32::try_from(v).ok());
    match kind {
        "unix-user" => {
            let uid = id("uid")?;
            Some(Identity::User { uid, name: user_name(uid)? })
        }
        "unix-group" => {
            let gid = id("gid")?;
            Some(Identity::Group { gid, members: group_members(gid) })
        }
        _ => None,
    }
}

/// The users an identity list allows, the current user first if among
/// them (as GNOME and KDE do), then the rest; groups stand for their
/// members.
fn users(identities: &[Identity]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in identities {
        match i {
            Identity::User { name, .. } => out.push(name.clone()),
            Identity::Group { members, .. } => out.extend(members.iter().cloned()),
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|n| seen.insert(n.clone()));
    // SAFETY: getuid has no failure modes.
    let me = user_name(unsafe { libc::getuid() });
    if let Some(i) = me.and_then(|me| out.iter().position(|n| *n == me)) {
        let me = out.remove(i);
        out.insert(0, me);
    }
    out
}

fn user_name(uid: u32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    // SAFETY: getpwuid_r writes into `pw` and `buf`, which we own and size.
    unsafe {
        let mut pw: libc::passwd = std::mem::zeroed();
        let mut out = std::ptr::null_mut();
        if libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr().cast(), buf.len(), &mut out) != 0 || out.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr(pw.pw_name).to_string_lossy().into_owned())
    }
}

fn group_members(gid: u32) -> Vec<String> {
    let mut buf = vec![0u8; 16384];
    // SAFETY: getgrgid_r writes into `gr` and `buf`, which we own and size;
    // gr_mem is a NULL-terminated array inside `buf`.
    unsafe {
        let mut gr: libc::group = std::mem::zeroed();
        let mut out = std::ptr::null_mut();
        if libc::getgrgid_r(gid, &mut gr, buf.as_mut_ptr().cast(), buf.len(), &mut out) != 0 || out.is_null() {
            return Vec::new();
        }
        let mut names = Vec::new();
        let mut p = gr.gr_mem;
        while !p.is_null() && !(*p).is_null() {
            names.push(std::ffi::CStr::from_ptr(*p).to_string_lossy().into_owned());
            p = p.add(1);
        }
        names
    }
}

/// A registered agent. Dropping it unregisters.
pub struct Agent {
    conn: Connection,
    subject: Subject,
}

impl Agent {
    /// Export the agent and register it with polkitd for `subject`. `on`
    /// is called from zbus's thread; hand the event to the UI.
    pub fn register(subject: Subject, on: impl Fn(Event) + Send + Sync + 'static) -> Result<Agent> {
        let conn = zbus::blocking::connection::Builder::system()?
            .serve_at(AGENT_PATH, AgentObject { on: Arc::new(on) })?
            .build()?;
        let locale = std::env::var("LANG").unwrap_or_else(|_| "C".into());
        bus::call::<_, ()>(&conn, AUTHORITY, AUTHORITY_PATH, AUTHORITY_IFACE, "RegisterAuthenticationAgent", &(subject.to_dbus(), locale, AGENT_PATH))?;
        Ok(Agent { conn, subject })
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = bus::call::<_, ()>(&self.conn, AUTHORITY, AUTHORITY_PATH, AUTHORITY_IFACE, "UnregisterAuthenticationAgent", &(self.subject.to_dbus(), AGENT_PATH));
    }
}

/// The logind session this process belongs to.
pub fn own_session() -> Result<String> {
    let path: zvariant::OwnedObjectPath = bus::call(
        bus::system()?,
        "org.freedesktop.login1",
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
        "GetSessionByPID",
        &(std::process::id(),),
    )?;
    bus::property(bus::system()?, "org.freedesktop.login1", path.as_str(), "org.freedesktop.login1.Session", "Id")
}

/// Ask polkitd whether this process may do `action_id`, letting it ask the
/// user (through whichever agent serves us). Changes nothing: for tests.
pub fn check_interactive(action_id: &str) -> Result<bool> {
    let subject = Subject::process(std::process::id()).ok_or_else(|| zbus::Error::Failure("no /proc entry".into()))?;
    let details: HashMap<&str, &str> = HashMap::new();
    let (authorized, _challenge, _details): (bool, bool, HashMap<String, String>) = bus::call(
        bus::system()?,
        AUTHORITY,
        AUTHORITY_PATH,
        AUTHORITY_IFACE,
        "CheckAuthorization",
        &(subject.to_dbus(), action_id, details, 1u32, ""),
    )?;
    Ok(authorized)
}

// ---- the helper ----------------------------------------------------------------

/// What the helper says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelperEvent {
    /// PAM wants an answer (`echo`: may be shown as typed).
    Prompt { text: String, echo: bool },
    Info(String),
    Error(String),
    /// Finished: authenticated or not.
    Done(bool),
}

/// One run of polkit's setuid helper for a cookie and user.
pub struct Helper {
    child: Child,
    stdin: ChildStdin,
}

fn helper_path() -> String {
    if let Ok(p) = std::env::var("OLO_POLKIT_HELPER") {
        return p;
    }
    ["/usr/lib/polkit-1/polkit-agent-helper-1", "/usr/libexec/polkit-agent-helper-1", "/usr/lib/policykit-1/polkit-agent-helper-1"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .unwrap_or("/usr/lib/polkit-1/polkit-agent-helper-1")
        .to_owned()
}

/// Parse one line of the helper's output.
pub fn parse_helper_line(line: &str) -> Option<HelperEvent> {
    let line = line.trim_end_matches(['\n', '\r']);
    let rest = |p: &str| line.strip_prefix(p).map(|r| r.strip_prefix(' ').unwrap_or(r).to_owned());
    if let Some(t) = rest("PAM_PROMPT_ECHO_OFF") {
        Some(HelperEvent::Prompt { text: t, echo: false })
    } else if let Some(t) = rest("PAM_PROMPT_ECHO_ON") {
        Some(HelperEvent::Prompt { text: t, echo: true })
    } else if let Some(t) = rest("PAM_ERROR_MSG") {
        Some(HelperEvent::Error(t))
    } else if let Some(t) = rest("PAM_TEXT_INFO") {
        Some(HelperEvent::Info(t))
    } else if line == "SUCCESS" {
        Some(HelperEvent::Done(true))
    } else if line == "FAILURE" {
        Some(HelperEvent::Done(false))
    } else {
        None
    }
}

impl Helper {
    /// Start the helper for `user`; its events arrive on a thread through
    /// `on`, ending with `Done` (also sent if it dies).
    pub fn start(user: &str, cookie: &str, on: impl Fn(HelperEvent) + Send + 'static) -> std::io::Result<Helper> {
        let mut child = Command::new(helper_path())
            .arg(user)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        // The cookie goes on stdin, not argv, where anyone could read it.
        writeln!(stdin, "{cookie}")?;
        stdin.flush()?;
        std::thread::Builder::new().name("polkit-helper".into()).spawn(move || {
            let mut done = false;
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(e) = parse_helper_line(&line) {
                    done |= matches!(e, HelperEvent::Done(_));
                    on(e);
                    if done {
                        break;
                    }
                }
            }
            if !done {
                on(HelperEvent::Done(false));
            }
        })?;
        Ok(Helper { child, stdin })
    }

    /// Answer the prompt.
    pub fn respond(&mut self, text: &str) -> std::io::Result<()> {
        writeln!(self.stdin, "{text}")?;
        self.stdin.flush()
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        // A finished helper has exited; otherwise stop it (cancel).
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_lines() {
        assert_eq!(parse_helper_line("PAM_PROMPT_ECHO_OFF Password: \n"), Some(HelperEvent::Prompt { text: "Password: ".into(), echo: false }));
        assert_eq!(parse_helper_line("PAM_TEXT_INFO Place your finger on the reader"), Some(HelperEvent::Info("Place your finger on the reader".into())));
        assert_eq!(parse_helper_line("PAM_ERROR_MSG Failed to match fingerprint"), Some(HelperEvent::Error("Failed to match fingerprint".into())));
        assert_eq!(parse_helper_line("SUCCESS"), Some(HelperEvent::Done(true)));
        assert_eq!(parse_helper_line("FAILURE"), Some(HelperEvent::Done(false)));
        assert_eq!(parse_helper_line("something else"), None);
    }

    #[test]
    fn own_process_subject() {
        let Some(Subject::Process { pid, start_time }) = Subject::process(std::process::id()) else { panic!() };
        assert_eq!(pid, std::process::id());
        assert!(start_time > 0);
    }

    #[test]
    fn user_order() {
        let me = user_name(unsafe { libc::getuid() }).unwrap();
        let ids = [Identity::User { uid: 0, name: "root".into() }, Identity::User { uid: 1, name: me.clone() }];
        assert_eq!(users(&ids)[0], me);
    }

    /// The helper protocol end to end against a fake helper script.
    #[test]
    fn fake_helper() {
        let dir = std::env::temp_dir().join(format!("olo-polkit-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("helper");
        std::fs::write(
            &script,
            "#!/bin/sh\nread cookie\n[ \"$cookie\" = c00kie ] || { echo FAILURE; exit 1; }\necho 'PAM_TEXT_INFO hello'\necho 'PAM_PROMPT_ECHO_OFF Password: '\nread pw\n[ \"$pw\" = sesame ] && echo SUCCESS || echo FAILURE\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        // SAFETY: tests in this module don't read the variable concurrently.
        unsafe { std::env::set_var("OLO_POLKIT_HELPER", &script) };
        for (pw, ok) in [("sesame", true), ("wrong", false)] {
            let (tx, rx) = std::sync::mpsc::channel();
            let mut h = Helper::start("nobody", "c00kie", move |e| tx.send(e).unwrap()).unwrap();
            assert_eq!(rx.recv().unwrap(), HelperEvent::Info("hello".into()));
            assert!(matches!(rx.recv().unwrap(), HelperEvent::Prompt { echo: false, .. }));
            h.respond(pw).unwrap();
            assert_eq!(rx.recv().unwrap(), HelperEvent::Done(ok));
        }
        unsafe { std::env::remove_var("OLO_POLKIT_HELPER") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! sudo's password in a window (olo_sudo.rs is the other side). Terminal
//! puts `olo-sudo`, as `sudo`, first on its shells' PATH and listens for
//! it here. When sudo needs a password, an alert opens over the window of
//! the tab it came from, saying who's asking (a coding agent by name) and
//! the command, and relays sudo's conversation: Touch ID first where PAM
//! has it, then the password. Cancel, Escape or the close box says no.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    io::{BufRead, BufReader, Write},
    os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use olo_kit::{
    Proxy,
    egui::{self, Align, Rect, pos2, vec2},
    metrics as m,
    style::{self, c},
    tip::Tip,
    widgets::{self, Conversation},
};

use zeroize::{Zeroize, Zeroizing};

use crate::{
    agent::Agent,
    sudo_wire::{self, Ask, Reply},
    wire,
};

pub const WIDTH: u32 = 420;
const ICON: &str = "authentication";
/// The password field's label column, as in the polkit agent's.
const DIVIDER: f32 = 72.0;
/// Lines of the command shown before it's cut short.
const COMMAND_ROWS: usize = 3;
/// Room around the command in its box.
const PAD: f32 = 4.0;
/// Text column: the alert's width less its stripe, icon and margins.
const COLUMN: f32 = WIDTH as f32 - widgets::ALERT_TEXT_X - m::WINDOW_INSET;
const LINE: f32 = style::PLAIN_SIZE + 4.0;

/// Where olo-sudo finds us (one per display).
pub fn socket_path() -> PathBuf {
    wire::runtime_dir().join(format!("olo-terminal-sudo-{}.sock", wire::display()))
}

/// Where the `sudo` that is olo-sudo is.
fn bin_dir() -> PathBuf {
    wire::runtime_dir().join(format!("olo-terminal-bin-{}", wire::display()))
}

/// Make the `sudo` that is olo-sudo (the one beside us) and put it first
/// on new shells' PATH, with where we listen. Without olo-sudo, shells
/// get sudo as it is.
pub fn shell_env(env: &mut Vec<(String, String)>) {
    let Some(ours) = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join("olo-sudo"))).filter(|p| p.exists()) else { return };
    let dir = bin_dir();
    let link = dir.join("sudo");
    if std::fs::read_link(&link).ok().as_deref() != Some(ours.as_path()) {
        let tmp = dir.join(".sudo.new");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_file(&tmp);
        if let Err(e) = std::os::unix::fs::symlink(&ours, &tmp).and_then(|_| std::fs::rename(&tmp, &link)) {
            tracing::warn!(?e, "sudo: can't put olo-sudo on the shells' PATH");
            return;
        }
    }
    let dir = dir.to_string_lossy().into_owned();
    let old = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).unwrap_or_default();
    let path = std::iter::once(dir.as_str()).chain(old.split(':').filter(|d| !d.is_empty() && *d != dir)).collect::<Vec<_>>().join(":");
    env.retain(|(k, _)| k != "PATH" && k != sudo_wire::SOCKET_ENV);
    env.push(("PATH".into(), path));
    env.push((sudo_wire::SOCKET_ENV.into(), socket_path().to_string_lossy().into_owned()));
}

/// What the listener hears, for the app.
pub enum Event {
    /// A sudo wants a password: from process `pid`, answered on `stream`;
    /// `approval` while requests are approved without asking.
    Begin { conn: u64, pid: i32, stream: UnixStream, args: Vec<String>, tty: bool, approval: Option<Approval> },
    Said(u64, Ask),
    /// It went (done, or stopped).
    Gone(u64),
    /// PAM takes a fingerprint first.
    Touch(u64),
}

/// Listen for olo-sudo, on a thread of its own.
pub fn listen(proxy: Proxy) {
    let path = socket_path();
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(?e, path = %path.display(), "sudo: can't listen");
            return;
        }
    };
    let spawned = std::thread::Builder::new().name("sudo".into()).spawn(move || {
        for (conn, s) in (1..).zip(listener.incoming().flatten()) {
            let Some(pid) = peer(&s) else { continue };
            let proxy = proxy.clone();
            std::thread::spawn(move || serve(proxy, conn, pid, s));
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(?e, "sudo: no thread to listen on");
    }
}

/// The peer's pid, if it is olo-sudo run by this user.
fn peer(s: &UnixStream) -> Option<i32> {
    // SAFETY: a zeroed ucred is valid; getsockopt fills it.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred and len are valid for getsockopt to write.
    let r = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&raw mut cred).cast(), &mut len) };
    // SAFETY: getuid can't fail.
    if r != 0 || cred.uid != unsafe { libc::getuid() } {
        return None;
    }
    let exe = std::fs::read_link(format!("/proc/{}/exe", cred.pid)).ok()?;
    let exe = exe.to_string_lossy();
    let name = Path::new(exe.strip_suffix(" (deleted)").unwrap_or(&exe)).file_name()?.to_owned();
    (name == "olo-sudo").then_some(cred.pid)
}

fn serve(proxy: Proxy, conn: u64, pid: i32, s: UnixStream) {
    let Ok(stream) = s.try_clone() else { return };
    let mut lines = BufReader::new(s).lines();
    let Some(Ok(Ask::Begin { args, tty, .. })) = lines.next().map(|l| l.map(|l| serde_json::from_str(&l))).and_then(Result::ok) else { return };
    let approval = if tty && !WINDOW_FOR_TYPED.load(Ordering::Relaxed) { None } else { approval() };
    proxy.send(Event::Begin { conn, pid, stream, args, tty, approval });
    let p = proxy.clone();
    std::thread::spawn(move || {
        if touch_first() {
            p.send(Event::Touch(conn));
        }
    });
    for l in lines.map_while(Result::ok) {
        if let Ok(a) = serde_json::from_str::<Ask>(&l) {
            proxy.send(Event::Said(conn, a));
        }
    }
    proxy.send(Event::Gone(conn));
}

/// Settings ▸ Ask for sudo Passwords in a Window, for the listener.
pub static WINDOW_FOR_TYPED: AtomicBool = AtomicBool::new(true);

/// While requests are approved without asking (app_server's
/// approve.rs): the password it was turned on with, and the fingerprint
/// sensor, held meanwhile when sudo's PAM would wait on it first, so it
/// goes straight to the password.
pub struct Approval {
    pub secret: Zeroizing<String>,
    _held: Option<olo_system::fingerprint::Held>,
}

/// The approval, if requests are approved without asking now (app_server
/// tells only Olo's Terminal). Blocks: call it off the UI thread.
fn approval() -> Option<Approval> {
    let reply = olo_protocol::Client::connect().ok()?.request(&olo_protocol::Request::AutoApproveSecret).ok()?;
    let olo_protocol::Reply::Secret { password } = reply else { return None };
    let secret = Zeroizing::new(password);
    let held = touch_first().then(|| olo_system::auth::current_user().and_then(|u| olo_system::fingerprint::hold(&u).ok())).flatten();
    Some(Approval { secret, _held: held })
}

/// Whether sudo's PAM asks for a fingerprint first, and there are some.
fn touch_first() -> bool {
    // Tests leave fprintd alone: no sensor wanted, or a stand-in sudo
    // (not the PAM read here).
    if std::env::var_os("OLO_NO_FINGERPRINT").is_some() || cfg!(debug_assertions) && std::env::var_os("OLO_SUDO_REAL").is_some() {
        return false;
    }
    pam_fprintd("sudo", 0) && olo_system::auth::current_user().is_some_and(|u| olo_system::fingerprint::enrolled(&u).unwrap_or(false))
}

/// Whether PAM `service`'s auth has pam_fprintd, following includes.
fn pam_fprintd(service: &str, depth: u8) -> bool {
    let Ok(text) = std::fs::read_to_string(Path::new("/etc/pam.d").join(service)) else { return false };
    text.lines().map(|l| l.split('#').next().unwrap_or_default()).any(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w.as_slice() {
            [kind, ..] if kind.trim_start_matches('-') == "auth" && l.contains("pam_fprintd") => true,
            [kind, "include" | "substack", s, ..] if kind.trim_start_matches('-') == "auth" && depth < 4 => pam_fprintd(s, depth + 1),
            ["@include", s] if depth < 4 => pam_fprintd(s, depth + 1),
            _ => false,
        }
    })
}

/// `pid`'s parents, nearest first.
pub fn ancestors(pid: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let mut p = pid;
    while out.len() < 64 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{p}/stat")) else { break };
        // After the name, which is in brackets and may hold anything.
        let Some(ppid) = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(1)?.parse::<i32>().ok()) else { break };
        if ppid <= 1 {
            break;
        }
        out.push(ppid);
        p = ppid;
    }
    out
}

/// One sudo's request, in its alert.
pub struct Request {
    pub conn: u64,
    stream: UnixStream,
    /// The coding agent asking, if one is.
    who: Option<Agent>,
    /// The tab it's in.
    tab: Option<String>,
    /// As typed: `sudo …`.
    command: String,
    pub conv: Conversation,
    /// Approved without asking: the password is given at sudo's prompt
    /// (once: should it be asked again, a window asks the person).
    pub approval: Option<Approval>,
    answered: bool,
}

/// How a request approved without asking goes.
pub enum Approved {
    Waiting,
    /// sudo is done: it said yes, or no.
    Done(bool),
    /// It wants the password again: ask the person, in a window.
    AskPerson,
}

pub enum Choice {
    Authenticate,
    Cancel,
}

impl Request {
    pub fn new(conn: u64, stream: UnixStream, args: &[String], who: Option<Agent>, tab: Option<String>) -> Request {
        let command = std::iter::once("sudo".to_owned()).chain(args.iter().map(|a| crate::shell_quote(a))).collect::<Vec<_>>().join(" ");
        Request { conn, stream, who, tab, command, conv: Conversation::default(), approval: None, answered: false }
    }

    /// Who asked for what, for the log and the notification.
    pub fn summary(&self) -> String {
        let who = self.who.map_or("sudo", |a| a.name());
        let tab = self.tab.as_ref().map(|t| format!(" (in “{t}”)")).unwrap_or_default();
        format!("{who}{tab}: {}", self.command)
    }

    /// Approved without asking, what sudo said, and what comes of it.
    pub fn approved(&mut self, a: Ask) -> Approved {
        match a {
            Ask::Prompt(_) if !self.answered => {
                let secret = self.approval.as_ref().map(|a| a.secret.to_string()).unwrap_or_default();
                let mut r = Reply::Answer(secret);
                self.reply(&r);
                if let Reply::Answer(a) = &mut r {
                    a.zeroize();
                }
                self.answered = true;
                Approved::Waiting
            }
            Ask::Info(_) | Ask::Begin { .. } => Approved::Waiting,
            Ask::Done(ok) => Approved::Done(ok),
            // Asked again, or an error: not the password it should be.
            said @ (Ask::Prompt(_) | Ask::Error(_)) => {
                self.approval = None;
                self.said(said);
                Approved::AskPerson
            }
        }
    }

    pub fn message(&self) -> String {
        // The tab's name last: it is the part that wraps.
        let tab = self.tab.as_ref().map(|t| format!(" in “{t}”")).unwrap_or_default();
        match self.who {
            Some(a) => format!("{} wants to run this with sudo{tab}:", a.name()),
            None => format!("sudo needs your password to run this{tab}:"),
        }
    }

    /// The window's height for its message and command.
    pub fn height(&self) -> u32 {
        let message = style::plain_text_height(&self.message(), COLUMN);
        // Monospaced, so its rows go by the count of characters.
        let command = (self.command.chars().count() as f32 * 7.3 / (COLUMN - 2.0 * PAD)).ceil().clamp(1.0, COMMAND_ROWS as f32);
        let fixed = 2.0 * m::WINDOW_INSET + m::LABEL_ROW + 2.0 * m::TIGHT + 2.0 * PAD + 3.0 * m::SPACING + 2.0 * m::CONTROL_HEIGHT + m::LABEL_ROW;
        (fixed + message + command * LINE).ceil() as u32
    }

    pub fn reply(&mut self, r: &Reply) {
        let mut line = sudo_wire::line(r);
        let _ = self.stream.write_all(&line);
        line.fill(0);
    }

    /// Send the answer typed.
    pub fn answer(&mut self) {
        let mut r = Reply::Answer(std::mem::take(&mut self.conv.answer));
        self.reply(&r);
        if let Reply::Answer(a) = &mut r {
            // SAFETY: zero bytes keep the string valid UTF-8.
            unsafe { a.as_bytes_mut().fill(0) };
        }
        self.conv.sent();
    }

    /// What sudo said; true once it's done.
    pub fn said(&mut self, a: Ask) -> bool {
        match a {
            Ask::Info(t) => {
                self.conv.error.clear();
                self.conv.info = t;
            }
            Ask::Error(t) => {
                self.conv.checking = false;
                self.conv.error = if t.starts_with("Sorry, try again") { "Sorry, that didn't work. Please try again.".into() } else { t };
            }
            Ask::Prompt(t) => {
                // Past the fingerprint, if there was one.
                self.conv.info.clear();
                self.conv.ask(t, false);
            }
            Ask::Done(_) => return true,
            Ask::Begin { .. } => {}
        }
        false
    }

    /// PAM takes a fingerprint first: say so until sudo says otherwise.
    pub fn touch(&mut self) {
        if self.conv.prompt.is_none() && self.conv.info.is_empty() && !self.conv.checking {
            self.conv.info = "Touch the fingerprint sensor, or wait to type your password.".into();
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<Choice> {
        let p = style::palette(ui.ctx());
        let col = widgets::alert_frame(ui, ICON);
        let painter = ui.painter().clone();
        let mut y = col.top();
        style::label(&painter, col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, Align::Min, "Authentication Required", style::bold_font(), c(p.panel_text));
        y += m::LABEL_ROW + m::TIGHT;

        let message = self.message();
        let mut job = egui::text::LayoutJob::single_section(message.clone(), egui::TextFormat::simple(style::plain_font(), c(p.panel_text)));
        job.wrap.max_width = col.width();
        let g = painter.layout_job(job);
        let pos = style::text_origin(ui.ctx(), &g, col.left(), y + style::PLAIN_SIZE / 2.0 + 2.0, style::PLAIN_SIZE);
        let r = Rect::from_min_size(pos, g.size());
        painter.galley(pos, g, c(p.panel_text));
        style::describe_text(ui.ctx(), egui::accesskit::Role::Label, &message, r, None);
        y = r.bottom() + m::TIGHT;

        // The command, as typed, in a box of its own.
        let mut job = egui::text::LayoutJob::single_section(self.command.clone(), egui::TextFormat::simple(style::mono_font(style::PLAIN_SIZE), c(p.document_text)));
        job.wrap = egui::text::TextWrapping { max_width: col.width() - 2.0 * PAD, max_rows: COMMAND_ROWS, break_anywhere: true, overflow_character: Some('…') };
        let g = painter.layout_job(job);
        let cut = g.elided;
        let field = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), g.size().y + 2.0 * PAD));
        painter.rect_filled(field.shrink(1.0), 0.0, c(p.document_background));
        style::sunken(&painter, field, &p, false);
        painter.galley(field.min + vec2(PAD, PAD), g, c(p.document_text));
        style::describe_text(ui.ctx(), egui::accesskit::Role::Label, &self.command, field, None);
        if cut {
            // In a popup of its own: the whole command is wider than the
            // window, and egui's tip would be cut at its edge.
            ui.interact(field, ui.scope_id().with("command"), egui::Sense::hover()).tip(&self.command);
        }
        y = field.bottom() + m::SPACING;

        let form = Rect::from_min_max(pos2(col.left(), y), pos2(col.right(), col.bottom() - m::CONTROL_HEIGHT - m::SPACING));
        let mut submit = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(form).layout(egui::Layout::top_down(Align::Min)), |ui| {
            ui.spacing_mut().item_spacing.y = m::SPACING;
            submit = widgets::conversation(ui, &mut self.conv, DIVIDER, form.width());
        });
        match widgets::alert_buttons(ui, col, &["Cancel", "Authenticate"], self.conv.can_submit()) {
            Some(0) => return Some(Choice::Cancel),
            Some(_) => submit = true,
            None => {}
        }
        (submit && self.conv.can_submit()).then_some(Choice::Authenticate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ancestors_lead_to_init() {
        let me = std::process::id() as i32;
        let up = ancestors(me);
        // SAFETY: getppid can't fail.
        assert_eq!(up.first().copied(), Some(unsafe { libc::getppid() }).filter(|&p| p > 1));
        assert!(!up.contains(&me));
    }
}

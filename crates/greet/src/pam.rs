//! PAM for logging in, natively (libpam opened at run time, as
//! `olo_system::auth` does for the lock screen): a conversation that asks
//! whoever is logging in, then the person's credentials, a session
//! (pam_systemd makes it logind's) and the environment PAM gives it.
//!
//! A [`Pam`] lives on one thread (a worker process's), from
//! [`Pam::start`] through [`Pam::authenticate`] and
//! [`Pam::open_session`] to its drop, which closes what was opened.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
};

use crate::PromptKind;

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_ESTABLISH_CRED: c_int = 0x0002;
const PAM_DELETE_CRED: c_int = 0x0004;
const PAM_REINITIALIZE_CRED: c_int = 0x0008;
const PAM_TTY: c_int = 3;
const PAM_RHOST: c_int = 4;
const PAM_XDISPLAY: c_int = 11;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type ConvFn = unsafe extern "C" fn(c_int, *mut *const PamMessage, *mut *mut PamResponse, *mut c_void) -> c_int;

#[repr(C)]
struct PamConv {
    conv: ConvFn,
    appdata_ptr: *mut c_void,
}

/// What the conversation asks: `kind` and its text; the answer (None for
/// words only shown, or to give up on a question).
pub type Converse = Box<dyn FnMut(PromptKind, &str) -> Answer>;

/// An answer to what PAM said.
pub enum Answer {
    /// Typed (for a question), or nothing (words shown).
    Said(Option<String>),
    /// Given up: the conversation fails.
    GiveUp,
}

type StartFn = unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *mut *mut c_void) -> c_int;
type StartConfdirFn = unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *const c_char, *mut *mut c_void) -> c_int;
type HandleFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type EndFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type StrErrorFn = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;
type SetItemFn = unsafe extern "C" fn(*mut c_void, c_int, *const c_void) -> c_int;
type PutEnvFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type GetEnvListFn = unsafe extern "C" fn(*mut c_void) -> *mut *mut c_char;

/// libpam's functions.
struct Lib {
    _lib: libloading::Library,
    start: StartFn,
    start_confdir: Option<StartConfdirFn>,
    authenticate: HandleFn,
    acct_mgmt: HandleFn,
    setcred: HandleFn,
    open_session: HandleFn,
    close_session: HandleFn,
    end: EndFn,
    strerror: StrErrorFn,
    set_item: SetItemFn,
    putenv: PutEnvFn,
    getenvlist: GetEnvListFn,
}

impl Lib {
    fn load() -> Result<Lib, String> {
        // SAFETY: the system's libpam, its own initialisers only; each
        // symbol taken with its documented C signature.
        unsafe {
            let lib = libloading::Library::new("libpam.so.0").map_err(|e| format!("libpam: {e}"))?;
            let get = |name: &[u8]| lib.get::<*const c_void>(name).map(|s| *s).map_err(|e| format!("libpam: {e}"));
            Ok(Lib {
                start: std::mem::transmute::<*const c_void, StartFn>(get(b"pam_start\0")?),
                start_confdir: get(b"pam_start_confdir\0").ok().map(|p| std::mem::transmute::<*const c_void, StartConfdirFn>(p)),
                authenticate: std::mem::transmute::<*const c_void, HandleFn>(get(b"pam_authenticate\0")?),
                acct_mgmt: std::mem::transmute::<*const c_void, HandleFn>(get(b"pam_acct_mgmt\0")?),
                setcred: std::mem::transmute::<*const c_void, HandleFn>(get(b"pam_setcred\0")?),
                open_session: std::mem::transmute::<*const c_void, HandleFn>(get(b"pam_open_session\0")?),
                close_session: std::mem::transmute::<*const c_void, HandleFn>(get(b"pam_close_session\0")?),
                end: std::mem::transmute::<*const c_void, EndFn>(get(b"pam_end\0")?),
                strerror: std::mem::transmute::<*const c_void, StrErrorFn>(get(b"pam_strerror\0")?),
                set_item: std::mem::transmute::<*const c_void, SetItemFn>(get(b"pam_set_item\0")?),
                putenv: std::mem::transmute::<*const c_void, PutEnvFn>(get(b"pam_putenv\0")?),
                getenvlist: std::mem::transmute::<*const c_void, GetEnvListFn>(get(b"pam_getenvlist\0")?),
                _lib: lib,
            })
        }
    }
}

/// The conversation's state, PAM's `appdata_ptr`.
struct Talk {
    converse: Converse,
}

/// PAM frees the responses with free(), so they come from malloc.
unsafe extern "C" fn conversation(n: c_int, msgs: *mut *const PamMessage, resp: *mut *mut PamResponse, data: *mut c_void) -> c_int {
    if n <= 0 || msgs.is_null() || resp.is_null() || data.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: PAM hands n messages and a slot for n responses; `data` is
    // the Talk given to pam_start, alive as long as the handle.
    unsafe {
        let talk = &mut *(data as *mut Talk);
        let out = libc::calloc(n as usize, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
        if out.is_null() {
            return PAM_BUF_ERR;
        }
        for i in 0..n as usize {
            // (Linux-PAM: an array of pointers to messages.)
            let m = *msgs.add(i);
            let text = if (*m).msg.is_null() { String::new() } else { CStr::from_ptr((*m).msg).to_string_lossy().into_owned() };
            let kind = match (*m).msg_style {
                PAM_PROMPT_ECHO_OFF => PromptKind::Secret,
                PAM_PROMPT_ECHO_ON => PromptKind::Visible,
                PAM_ERROR_MSG => PromptKind::Error,
                PAM_TEXT_INFO => PromptKind::Info,
                _ => PromptKind::Info,
            };
            match (talk.converse)(kind, &text) {
                Answer::Said(Some(answer)) => {
                    let Ok(c) = CString::new(answer) else {
                        free_responses(out, i);
                        return PAM_CONV_ERR;
                    };
                    (*out.add(i)).resp = libc::strdup(c.as_ptr());
                }
                Answer::Said(None) => {}
                Answer::GiveUp => {
                    free_responses(out, i);
                    return PAM_CONV_ERR;
                }
            }
        }
        *resp = out;
    }
    PAM_SUCCESS
}

/// The first `n` responses' strings, and the array, freed (a
/// conversation given up).
unsafe fn free_responses(out: *mut PamResponse, n: usize) {
    // SAFETY: the caller's: `out` from calloc with at least `n` filled.
    unsafe {
        for k in 0..n {
            let r = (*out.add(k)).resp;
            if !r.is_null() {
                // (A password: wiped first.)
                libc::memset(r.cast(), 0, libc::strlen(r));
                libc::free(r.cast());
            }
        }
        libc::free(out.cast());
    }
}

/// A PAM transaction for one person through one service.
pub struct Pam {
    lib: Lib,
    handle: *mut c_void,
    /// Kept where PAM points at it.
    _talk: Box<Talk>,
    _conv: Box<PamConv>,
    credentials: bool,
    session: bool,
    last: c_int,
}

impl Pam {
    /// A transaction for `user` through `service` (its stack from
    /// `confdir` instead of /etc/pam.d, for tests), its questions put to
    /// `converse`.
    pub fn start(service: &str, user: &str, confdir: Option<&Path>, converse: Converse) -> Result<Pam, String> {
        let lib = Lib::load()?;
        let bad = |_| "a name with a NUL in it".to_string();
        let (c_service, c_user) = (CString::new(service).map_err(bad)?, CString::new(user).map_err(bad)?);
        let mut talk = Box::new(Talk { converse });
        let conv = Box::new(PamConv { conv: conversation, appdata_ptr: (&mut *talk as *mut Talk).cast() });
        let mut handle: *mut c_void = std::ptr::null_mut();
        // SAFETY: the strings live through the call (PAM copies them); the
        // conversation and its state are boxed and kept with the handle.
        let r = unsafe {
            match confdir {
                None => (lib.start)(c_service.as_ptr(), c_user.as_ptr(), &*conv, &mut handle),
                Some(dir) => {
                    let start = lib.start_confdir.ok_or("this libpam has no pam_start_confdir")?;
                    let dir = CString::new(dir.as_os_str().as_encoded_bytes()).map_err(bad)?;
                    start(c_service.as_ptr(), c_user.as_ptr(), &*conv, dir.as_ptr(), &mut handle)
                }
            }
        };
        if r != PAM_SUCCESS || handle.is_null() {
            return Err(format!("PAM couldn't start {service} ({r})"));
        }
        Ok(Pam { lib, handle, _talk: talk, _conv: conv, credentials: false, session: false, last: PAM_SUCCESS })
    }

    fn check(&mut self, r: c_int) -> Result<(), String> {
        self.last = r;
        if r == PAM_SUCCESS {
            return Ok(());
        }
        // SAFETY: our handle; PAM's static text for the code.
        let p = unsafe { (self.lib.strerror)(self.handle, r) };
        Err(if p.is_null() { format!("PAM error {r}") } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() })
    }

    /// Ask whoever is logging in (the conversation), and whether their
    /// account may log in now.
    pub fn authenticate(&mut self) -> Result<(), String> {
        // SAFETY: our handle.
        let r = unsafe { (self.lib.authenticate)(self.handle, 0) };
        self.check(r)?;
        // SAFETY: as above.
        let r = unsafe { (self.lib.acct_mgmt)(self.handle, 0) };
        self.check(r)
    }

    /// Set an item: the session's terminal (`tty1`) or display.
    pub fn set_tty(&mut self, tty: &str) -> Result<(), String> {
        self.set(PAM_TTY, tty)
    }

    pub fn set_display(&mut self, display: &str) -> Result<(), String> {
        self.set(PAM_XDISPLAY, display)
    }

    /// The host logging in from (none: this computer).
    pub fn set_rhost(&mut self, host: &str) -> Result<(), String> {
        self.set(PAM_RHOST, host)
    }

    fn set(&mut self, item: c_int, value: &str) -> Result<(), String> {
        let c = CString::new(value).map_err(|_| "a NUL in an item".to_string())?;
        // SAFETY: our handle; PAM copies the string.
        let r = unsafe { (self.lib.set_item)(self.handle, item, c.as_ptr().cast()) };
        self.check(r)
    }

    /// `NAME=value` in PAM's environment for the session (pam_systemd
    /// reads XDG_SESSION_TYPE, XDG_SESSION_CLASS, XDG_SEAT, XDG_VTNR).
    pub fn putenv(&mut self, name: &str, value: &str) -> Result<(), String> {
        let c = CString::new(format!("{name}={value}")).map_err(|_| "a NUL in a variable".to_string())?;
        // SAFETY: our handle; PAM copies the string.
        let r = unsafe { (self.lib.putenv)(self.handle, c.as_ptr()) };
        self.check(r)
    }

    /// The person's credentials, then their session (logind's, by
    /// pam_systemd, with the variables put before).
    pub fn open_session(&mut self) -> Result<(), String> {
        // SAFETY: our handle.
        let r = unsafe { (self.lib.setcred)(self.handle, PAM_ESTABLISH_CRED) };
        self.check(r)?;
        self.credentials = true;
        // SAFETY: as above.
        let r = unsafe { (self.lib.open_session)(self.handle, 0) };
        self.check(r)?;
        self.session = true;
        // (Some modules set credentials that need the session first.)
        // SAFETY: as above.
        let r = unsafe { (self.lib.setcred)(self.handle, PAM_REINITIALIZE_CRED) };
        self.check(r)
    }

    /// PAM's environment for the session (`NAME=value`s: XDG_SESSION_ID,
    /// XDG_RUNTIME_DIR and what modules add).
    pub fn env(&self) -> Vec<(String, String)> {
        // SAFETY: our handle; a NULL-ended array of malloc'd strings, each
        // freed (and the array) as it's read.
        unsafe {
            let list = (self.lib.getenvlist)(self.handle);
            if list.is_null() {
                return Vec::new();
            }
            let mut out = Vec::new();
            let mut i = 0;
            loop {
                let p = *list.add(i);
                if p.is_null() {
                    break;
                }
                if let Some((k, v)) = CStr::from_ptr(p).to_string_lossy().split_once('=') {
                    out.push((k.to_owned(), v.to_owned()));
                }
                libc::free(p.cast());
                i += 1;
            }
            libc::free(list.cast());
            out
        }
    }
}

impl Drop for Pam {
    fn drop(&mut self) {
        // SAFETY: our handle, closed once, in order.
        unsafe {
            if self.session {
                (self.lib.close_session)(self.handle, 0);
            }
            if self.credentials {
                (self.lib.setcred)(self.handle, PAM_DELETE_CRED);
            }
            (self.lib.end)(self.handle, self.last);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, path::PathBuf, rc::Rc};

    use super::*;

    fn me() -> String {
        // SAFETY: getpwuid's static entry, copied at once.
        unsafe { CStr::from_ptr((*libc::getpwuid(libc::getuid())).pw_name).to_string_lossy().into_owned() }
    }

    /// The conversation through private services: a password asked and
    /// checked (pam_exec piping it to a script that knows it), words
    /// shown, refusal, giving up; then a session opened (pam_permit's)
    /// and its environment read.
    #[test]
    fn a_conversation_through_private_services() {
        let dir = PathBuf::from(format!("/var/tmp/olo-greet-pam-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("check.sh");
        std::fs::write(&script, "#!/bin/sh\nread -r p\n[ \"$p\" = \"open sesame\" ]\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::fs::write(
            dir.join("login"),
            format!("auth required pam_exec.so type=auth expose_authtok quiet {}\nauth required pam_permit.so\naccount required pam_permit.so\nsession required pam_permit.so\n", script.display()),
        )
        .unwrap();
        std::fs::write(dir.join("deny"), "auth required pam_deny.so\naccount required pam_permit.so\n").unwrap();
        let asked: Rc<RefCell<Vec<(PromptKind, String)>>> = Rc::default();
        let with = |answer: Option<&'static str>, asked: Rc<RefCell<Vec<(PromptKind, String)>>>| -> Converse {
            Box::new(move |kind, text: &str| {
                asked.borrow_mut().push((kind, text.to_owned()));
                match (kind, answer) {
                    (PromptKind::Secret | PromptKind::Visible, Some(a)) => Answer::Said(Some(a.to_owned())),
                    (PromptKind::Secret | PromptKind::Visible, None) => Answer::GiveUp,
                    _ => Answer::Said(None),
                }
            })
        };
        let user = me();
        // The right password: in, a session opened.
        let mut p = Pam::start("login", &user, Some(&dir), with(Some("open sesame"), asked.clone())).unwrap();
        p.putenv("XDG_SESSION_TYPE", "wayland").unwrap();
        assert_eq!(p.authenticate(), Ok(()));
        assert!(asked.borrow().iter().any(|(k, _)| *k == PromptKind::Secret), "{:?}", asked.borrow());
        p.open_session().unwrap();
        assert!(p.env().contains(&("XDG_SESSION_TYPE".into(), "wayland".into())));
        drop(p);
        // A wrong one, a refusing service, and giving up: not in.
        let mut p = Pam::start("login", &user, Some(&dir), with(Some("open sesame!"), asked.clone())).unwrap();
        assert!(p.authenticate().is_err());
        let mut p = Pam::start("deny", &user, Some(&dir), with(Some("anything"), asked.clone())).unwrap();
        assert!(p.authenticate().is_err());
        let mut p = Pam::start("login", &user, Some(&dir), with(None, asked.clone())).unwrap();
        assert!(p.authenticate().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

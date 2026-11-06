//! `pam_olo_face.so`: face unlock in PAM. In sudo's and polkit's stacks
//! as `auth sufficient pam_olo_face.so` before the password: it asks
//! olo-faced (`org.olo.Face1`, the system bus) to look for the user's
//! face, saying "Looking for your face…" through the conversation, and
//! succeeds if it's them. Anything else (no service, nobody enrolled,
//! no camera, not them, a remote login) lets the stack go on to the
//! password: it never fails a login, only spares the typing. It looks
//! for at most 3 seconds (2 when no face shows at all), and not at all
//! when there's no one enrolled or no camera (the service says so at
//! once).
//!
//! Module arguments: `bus=ADDRESS` (tests: a private bus).

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    time::Duration,
};

const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_AUTHINFO_UNAVAIL: c_int = 9;
const PAM_IGNORE: c_int = 25;
const PAM_CONV: c_int = 5;
const PAM_RHOST: c_int = 4;
const PAM_TEXT_INFO: c_int = 4;
const LOOK_MS: u32 = 3000;

#[repr(C)]
struct PamMessage {
    style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    retcode: c_int,
}

#[repr(C)]
struct PamConv {
    conv: Option<unsafe extern "C" fn(c_int, *mut *const PamMessage, *mut *mut PamResponse, *mut c_void) -> c_int>,
    appdata: *mut c_void,
}

// libpam by its file name (as PAM's own modules need it: a host that
// opened libpam privately, a test's, still gives them to the module),
// no development link needed to build.
#[link(name = "libpam.so.0", kind = "dylib", modifiers = "+verbatim")]
unsafe extern "C" {
    fn pam_get_user(pamh: *mut c_void, user: *mut *const c_char, prompt: *const c_char) -> c_int;
    fn pam_get_item(pamh: *const c_void, item: c_int, out: *mut *const c_void) -> c_int;
}

/// Say `text` through the application's conversation (sudo prints it,
/// polkit's agent shows it).
unsafe fn say(pamh: *mut c_void, text: &str) {
    let mut item: *const c_void = std::ptr::null();
    if unsafe { pam_get_item(pamh, PAM_CONV, &mut item) } != PAM_SUCCESS || item.is_null() {
        return;
    }
    let conv = unsafe { &*(item as *const PamConv) };
    let Some(f) = conv.conv else { return };
    let Ok(c) = CString::new(text) else { return };
    let msg = PamMessage { style: PAM_TEXT_INFO, msg: c.as_ptr() };
    let mut msgs = [&msg as *const PamMessage];
    let mut resp: *mut PamResponse = std::ptr::null_mut();
    unsafe { f(1, msgs.as_mut_ptr(), &mut resp, conv.appdata) };
    if !resp.is_null() {
        // The application's answer to an info message: freed as PAM does.
        unsafe {
            if !(*resp).resp.is_null() {
                libc_free((*resp).resp as *mut c_void);
            }
            libc_free(resp as *mut c_void);
        }
    }
}

unsafe extern "C" {
    #[link_name = "free"]
    fn libc_free(p: *mut c_void);
}

/// Ask olo-faced, on a thread (a call that never answers doesn't hold
/// the login): its answer, or None.
fn ask(bus: Option<String>, method: &'static str, user: String, timeout: Duration) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let conn = match &bus {
            Some(a) => zbus::blocking::connection::Builder::address(a.as_str()).and_then(|b| b.build()),
            None => zbus::blocking::Connection::system(),
        };
        let r = conn.ok().and_then(|c| {
            let reply = match method {
                "Status" => c.call_method(Some("org.olo.Face1"), "/org/olo/Face1", Some("org.olo.Face1"), "Status", &(user.as_str(),)),
                _ => c.call_method(Some("org.olo.Face1"), "/org/olo/Face1", Some("org.olo.Face1"), "Verify", &(user.as_str(), LOOK_MS)),
            };
            reply.ok().and_then(|m| m.body().deserialize::<String>().ok())
        });
        let _ = tx.send(r);
    });
    rx.recv_timeout(timeout).ok().flatten()
}

/// # Safety
/// PAM calls it with its handle and the module's arguments.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_authenticate(pamh: *mut c_void, _flags: c_int, argc: c_int, argv: *const *const c_char) -> c_int {
    let args: Vec<String> = (0..argc.max(0) as usize).filter_map(|i| unsafe { argv.add(i).as_ref() }).filter_map(|p| unsafe { CStr::from_ptr(*p) }.to_str().ok().map(str::to_owned)).collect();
    let bus = args.iter().find_map(|a| a.strip_prefix("bus=")).map(str::to_owned);
    // Only someone at this computer.
    let mut rhost: *const c_void = std::ptr::null();
    if unsafe { pam_get_item(pamh, PAM_RHOST, &mut rhost) } == PAM_SUCCESS && !rhost.is_null() && !unsafe { CStr::from_ptr(rhost as *const c_char) }.to_bytes().is_empty() {
        return PAM_IGNORE;
    }
    if std::env::var_os("SSH_CONNECTION").is_some() {
        return PAM_IGNORE;
    }
    let mut user: *const c_char = std::ptr::null();
    if unsafe { pam_get_user(pamh, &mut user, std::ptr::null()) } != PAM_SUCCESS || user.is_null() {
        return PAM_AUTHINFO_UNAVAIL;
    }
    let Ok(user) = unsafe { CStr::from_ptr(user) }.to_str().map(str::to_owned) else { return PAM_AUTHINFO_UNAVAIL };
    // Enrolled, with a camera: else on to the password at once.
    let Some(status) = ask(bus.clone(), "Status", user.clone(), Duration::from_secs(2)) else { return PAM_AUTHINFO_UNAVAIL };
    if !status.contains("\"camera\":true") || status.contains("\"enrolled\":0") {
        return PAM_AUTHINFO_UNAVAIL;
    }
    unsafe { say(pamh, "Looking for your face…") };
    match ask(bus, "Verify", user, Duration::from_millis(LOOK_MS as u64 + 3000)).as_deref() {
        Some("match") => PAM_SUCCESS,
        Some("no-match" | "flat") => PAM_AUTH_ERR,
        _ => PAM_AUTHINFO_UNAVAIL,
    }
}

/// # Safety
/// PAM calls it; nothing to set.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_setcred(_pamh: *mut c_void, _flags: c_int, _argc: c_int, _argv: *const *const c_char) -> c_int {
    PAM_SUCCESS
}

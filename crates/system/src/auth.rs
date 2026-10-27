//! Checking a user's password through PAM, natively: libpam is loaded at
//! run time (only its runtime library is needed) and driven with a
//! conversation that answers the password prompt. Blocks (PAM sleeps on a
//! failure, two seconds by default): call it from a worker thread.
//!
//! The service is `olo-lock` if installed, else `password-auth`, a
//! password-only stack (fingerprints are read separately, over fprintd, so
//! a password never waits on the sensor), else `login`.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
};

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// Wrong password (or account refused).
    Denied(String),
    /// PAM couldn't be used at all.
    Unavailable(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Denied(m) | AuthError::Unavailable(m) => f.write_str(m),
        }
    }
}

struct Answers<'a> {
    user: &'a CStr,
    password: &'a CStr,
}

/// PAM frees the responses with free(), so they must come from malloc.
unsafe extern "C" fn converse(
    n: c_int,
    msgs: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    data: *mut c_void,
) -> c_int {
    if n <= 0 || msgs.is_null() || resp.is_null() || data.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: PAM hands us n messages and a slot for n responses; `data` is
    // the Answers we passed to pam_start, alive for the whole call.
    unsafe {
        let answers = &*(data as *const Answers);
        let out = libc::calloc(n as usize, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
        if out.is_null() {
            return PAM_BUF_ERR;
        }
        for i in 0..n as usize {
            // Linux-PAM passes an array of pointers to messages.
            let m = *msgs.add(i);
            let reply = match (*m).msg_style {
                PAM_PROMPT_ECHO_OFF => Some(answers.password),
                PAM_PROMPT_ECHO_ON => Some(answers.user),
                _ => None, // info and error text: nothing to answer
            };
            if let Some(text) = reply {
                (*out.add(i)).resp = libc::strdup(text.as_ptr());
            }
        }
        *resp = out;
    }
    PAM_SUCCESS
}

struct Pam {
    lib: libloading::Library,
}

type StartFn = unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *mut *mut c_void) -> c_int;
type StartConfdirFn =
    unsafe extern "C" fn(*const c_char, *const c_char, *const PamConv, *const c_char, *mut *mut c_void) -> c_int;
type HandleFn = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type StrErrorFn = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_char;

impl Pam {
    fn load() -> Result<Pam, AuthError> {
        // SAFETY: loading the system's libpam runs no foreign initialisers
        // beyond its own.
        let lib = unsafe { libloading::Library::new("libpam.so.0") }
            .map_err(|e| AuthError::Unavailable(format!("libpam: {e}")))?;
        Ok(Pam { lib })
    }

    fn sym<T: Copy>(&self, name: &[u8]) -> Result<T, AuthError> {
        // SAFETY: each name is looked up with its documented C signature.
        unsafe { self.lib.get::<T>(name).map(|s| *s) }
            .map_err(|e| AuthError::Unavailable(format!("libpam: {e}")))
    }
}

pub fn service() -> &'static str {
    for s in ["olo-lock", "password-auth"] {
        if Path::new("/etc/pam.d").join(s).exists() || Path::new("/usr/lib/pam.d").join(s).exists() {
            return s;
        }
    }
    "login"
}

/// Check `password` for `user`. Ok means PAM authenticated the user and
/// their account is usable.
pub fn check_password(user: &str, password: &str) -> Result<(), AuthError> {
    // Debug builds only: a private PAM service for automated lock-screen
    // tests (OLO_PAM_CONFDIR holding OLO_PAM_SERVICE).
    #[cfg(debug_assertions)]
    if let (Some(dir), Ok(svc)) = (std::env::var_os("OLO_PAM_CONFDIR"), std::env::var("OLO_PAM_SERVICE")) {
        return check(&svc, Some(Path::new(&dir)), user, password);
    }
    check(service(), None, user, password)
}

/// With `confdir`, the service is read from there instead of /etc/pam.d
/// (`pam_start_confdir`, for tests).
fn check(service: &str, confdir: Option<&Path>, user: &str, password: &str) -> Result<(), AuthError> {
    let pam = Pam::load()?;
    let authenticate: HandleFn = pam.sym(b"pam_authenticate\0")?;
    let acct_mgmt: HandleFn = pam.sym(b"pam_acct_mgmt\0")?;
    let end: HandleFn = pam.sym(b"pam_end\0")?;
    let strerror: StrErrorFn = pam.sym(b"pam_strerror\0")?;

    let bad = |_| AuthError::Denied("Invalid characters".into());
    let (c_user, c_pass, c_service) =
        (CString::new(user).map_err(bad)?, CString::new(password).map_err(bad)?, CString::new(service).map_err(bad)?);
    let answers = Answers { user: &c_user, password: &c_pass };
    let conv = PamConv { conv: converse, appdata_ptr: &answers as *const Answers as *mut c_void };
    let mut handle: *mut c_void = std::ptr::null_mut();
    // SAFETY: the strings, conversation and answers outlive the handle,
    // which pam_end releases on every path.
    unsafe {
        let r = match confdir {
            None => pam.sym::<StartFn>(b"pam_start\0")?(c_service.as_ptr(), c_user.as_ptr(), &conv, &mut handle),
            Some(dir) => {
                let dir = CString::new(dir.as_os_str().as_encoded_bytes()).map_err(bad)?;
                pam.sym::<StartConfdirFn>(b"pam_start_confdir\0")?(
                    c_service.as_ptr(),
                    c_user.as_ptr(),
                    &conv,
                    dir.as_ptr(),
                    &mut handle,
                )
            }
        };
        if r != PAM_SUCCESS || handle.is_null() {
            return Err(AuthError::Unavailable(format!("pam_start failed ({r})")));
        }
        let mut r = authenticate(handle, 0);
        if r == PAM_SUCCESS {
            r = acct_mgmt(handle, 0);
        }
        let message = if r == PAM_SUCCESS {
            String::new()
        } else {
            let p = strerror(handle, r);
            if p.is_null() { format!("error {r}") } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        end(handle, r);
        if r == PAM_SUCCESS { Ok(()) } else { Err(AuthError::Denied(message)) }
    }
}

/// The user running this process.
pub fn current_user() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage (or null); we
    // copy the name out before anything else can call it.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        Some(CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned())
    }
}

/// The user's full name from the GECOS field, if set.
pub fn full_name(user: &str) -> Option<String> {
    let c = CString::new(user).ok()?;
    // SAFETY: as in current_user.
    unsafe {
        let pw = libc::getpwnam(c.as_ptr());
        if pw.is_null() || (*pw).pw_gecos.is_null() {
            return None;
        }
        let gecos = CStr::from_ptr((*pw).pw_gecos).to_string_lossy();
        let name = gecos.split(',').next().unwrap_or("").trim().to_owned();
        (!name.is_empty()).then_some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wrong password is refused (and PAM's delay is paid once).
    #[test]
    #[ignore]
    fn live_wrong_password() {
        let user = current_user().unwrap();
        let r = check_password(&user, "definitely not the password");
        eprintln!("service {}: {r:?}", service());
        assert!(matches!(r, Err(AuthError::Denied(_))));
    }

    /// The whole conversation, against private services: one that checks
    /// the password the module was asked for, one that refuses.
    #[test]
    fn conversation_with_private_services() {
        let dir = std::env::temp_dir().join(format!("olo-pam-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("allow"), "auth required pam_permit.so\naccount required pam_permit.so\n").unwrap();
        std::fs::write(dir.join("deny"), "auth required pam_deny.so\naccount required pam_permit.so\n").unwrap();
        // pam_exec asks for the password through our conversation and
        // pipes it to a script that knows the right one.
        let script = dir.join("check.sh");
        std::fs::write(&script, "#!/bin/sh\nread -r p\n[ \"$p\" = \"open sesame\" ]\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::fs::write(
            dir.join("exec"),
            format!("auth required pam_exec.so expose_authtok quiet {}\naccount required pam_permit.so\n", script.display()),
        )
        .unwrap();
        let user = current_user().unwrap();
        assert_eq!(check("allow", Some(&dir), &user, "pw"), Ok(()));
        assert!(matches!(check("deny", Some(&dir), &user, "pw"), Err(AuthError::Denied(_))));
        assert!(matches!(check("allow", Some(&dir), &user, "nul\0inside"), Err(AuthError::Denied(_))));
        assert_eq!(check("exec", Some(&dir), &user, "open sesame"), Ok(()));
        assert!(matches!(check("exec", Some(&dir), &user, "open sesame!"), Err(AuthError::Denied(_))));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn users() {
        assert!(current_user().is_some());
    }
}

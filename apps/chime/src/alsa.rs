//! alsa-lib, loaded as olo-chime runs (`libasound.so.2`, there wherever
//! PipeWire is; no headers to build against): a card's use-case manager
//! (UCM: which of its paths to switch on for the speakers, as WirePlumber
//! asks it) and a PCM to write the chime to. Only what olo-chime uses.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};

use libloading::{Library, Symbol};

/// `SND_PCM_STREAM_PLAYBACK`, `SND_PCM_FORMAT_S16_LE`,
/// `SND_PCM_ACCESS_RW_INTERLEAVED`.
const PLAYBACK: c_int = 0;
const S16_LE: c_int = 2;
const RW_INTERLEAVED: c_int = 3;

pub struct Alsa {
    lib: Library,
}

type Ucm = *mut c_void;
type Pcm = *mut c_void;

impl Alsa {
    pub fn load() -> Result<Alsa, String> {
        // SAFETY: alsa-lib's initialisers are safe to run.
        let lib = unsafe { Library::new("libasound.so.2") }.map_err(|e| format!("alsa-lib isn't there: {e}"))?;
        Ok(Alsa { lib })
    }

    fn sym<T>(&self, name: &[u8]) -> Result<Symbol<'_, T>, String> {
        // SAFETY: each name is looked up with the type alsa-lib's headers
        // give it.
        unsafe { self.lib.get(name) }.map_err(|e| format!("alsa-lib: {e}"))
    }

    /// alsa-lib's words for error `n` (negative).
    pub fn error(&self, n: c_int) -> String {
        let Ok(f) = self.sym::<unsafe extern "C" fn(c_int) -> *const c_char>(b"snd_strerror\0") else { return format!("error {n}") };
        // SAFETY: snd_strerror returns a static string.
        unsafe { CStr::from_ptr(f(n)) }.to_string_lossy().into_owned()
    }

    fn check(&self, what: &str, n: c_int) -> Result<c_int, String> {
        if n < 0 { Err(format!("{what}: {}", self.error(n))) } else { Ok(n) }
    }

    /// Card `card`'s use-case manager ("hw:1"), if it has one.
    pub fn ucm(&self, card: &str) -> Result<UseCases<'_>, String> {
        let open = self.sym::<unsafe extern "C" fn(*mut Ucm, *const c_char) -> c_int>(b"snd_use_case_mgr_open\0")?;
        let name = CString::new(card).map_err(|e| e.to_string())?;
        let mut mgr: Ucm = std::ptr::null_mut();
        // SAFETY: mgr is written on success.
        self.check("its use cases", unsafe { open(&mut mgr, name.as_ptr()) })?;
        Ok(UseCases { alsa: self, mgr })
    }

    /// PCM `name` opened for playback at 48 kHz, 16-bit stereo (`plughw`
    /// makes what the card takes of it).
    pub fn pcm(&self, name: &str) -> Result<Playback<'_>, String> {
        let open = self.sym::<unsafe extern "C" fn(*mut Pcm, *const c_char, c_int, c_int) -> c_int>(b"snd_pcm_open\0")?;
        let set = self.sym::<unsafe extern "C" fn(Pcm, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int>(b"snd_pcm_set_params\0")?;
        let cname = CString::new(name).map_err(|e| e.to_string())?;
        let mut pcm: Pcm = std::ptr::null_mut();
        // SAFETY: pcm is written on success.
        self.check(name, unsafe { open(&mut pcm, cname.as_ptr(), PLAYBACK, 0) })?;
        let p = Playback { alsa: self, pcm };
        // (A fifth of a second buffered: soft resampling allowed.)
        // SAFETY: pcm is open.
        self.check(name, unsafe { set(pcm, S16_LE, RW_INTERLEAVED, 2, olo_chime::RATE, 1, 200_000) })?;
        Ok(p)
    }
}

/// A card's use-case manager.
pub struct UseCases<'a> {
    alsa: &'a Alsa,
    mgr: Ucm,
}

impl UseCases<'_> {
    /// A list's names (`_verbs`, `_devices/HiFi`: name and comment
    /// pairs; the names).
    pub fn list(&self, of: &str) -> Vec<String> {
        let (Ok(get), Ok(free)) = (
            self.alsa.sym::<unsafe extern "C" fn(Ucm, *const c_char, *mut *mut *const c_char) -> c_int>(b"snd_use_case_get_list\0"),
            self.alsa.sym::<unsafe extern "C" fn(*mut *const c_char, c_int) -> c_int>(b"snd_use_case_free_list\0"),
        ) else {
            return Vec::new();
        };
        let Ok(id) = CString::new(of) else { return Vec::new() };
        let mut list: *mut *const c_char = std::ptr::null_mut();
        // SAFETY: list is written with n strings on success.
        let n = unsafe { get(self.mgr, id.as_ptr(), &mut list) };
        if n <= 0 || list.is_null() {
            return Vec::new();
        }
        // SAFETY: n strings at list, each a C string or null.
        let all: Vec<String> =
            (0..n as usize).map(|i| unsafe { *list.add(i) }).map(|s| if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy().into_owned() }).collect();
        // SAFETY: the list alsa-lib gave.
        unsafe { free(list, n) };
        all.into_iter().step_by(2).collect()
    }

    /// One value (`PlaybackPCM/Speaker`).
    pub fn get(&self, what: &str) -> Option<String> {
        let get = self.alsa.sym::<unsafe extern "C" fn(Ucm, *const c_char, *mut *const c_char) -> c_int>(b"snd_use_case_get\0").ok()?;
        let id = CString::new(what).ok()?;
        let mut value: *const c_char = std::ptr::null();
        // SAFETY: value is written with a string we free on success.
        if unsafe { get(self.mgr, id.as_ptr(), &mut value) } < 0 || value.is_null() {
            return None;
        }
        // SAFETY: a C string alsa-lib allocated.
        let v = unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned();
        // SAFETY: freed with libc's free, as alsa-lib says.
        unsafe { libc::free(value as *mut c_void) };
        Some(v)
    }

    /// Set `what` (`_verb`, `_enadev`, `_disdev`) to `value`: what the card
    /// is switched to (its use case's mixer settings run).
    pub fn set(&self, what: &str, value: &str) -> Result<(), String> {
        let set = self.alsa.sym::<unsafe extern "C" fn(Ucm, *const c_char, *const c_char) -> c_int>(b"snd_use_case_set\0")?;
        let (id, v) = (CString::new(what).map_err(|e| e.to_string())?, CString::new(value).map_err(|e| e.to_string())?);
        // SAFETY: the manager is open.
        self.alsa.check(&format!("{what} {value}"), unsafe { set(self.mgr, id.as_ptr(), v.as_ptr()) }).map(|_| ())
    }
}

impl Drop for UseCases<'_> {
    fn drop(&mut self) {
        if let Ok(close) = self.alsa.sym::<unsafe extern "C" fn(Ucm) -> c_int>(b"snd_use_case_mgr_close\0") {
            // SAFETY: opened by snd_use_case_mgr_open.
            unsafe { close(self.mgr) };
        }
    }
}

/// An open playback PCM.
pub struct Playback<'a> {
    alsa: &'a Alsa,
    pcm: Pcm,
}

impl Playback<'_> {
    /// `samples` (stereo, interleaved) played, then drained: returns when
    /// the card has played it all.
    pub fn play(&self, samples: &[i16]) -> Result<(), String> {
        let write = self.alsa.sym::<unsafe extern "C" fn(Pcm, *const c_void, c_ulong) -> c_long>(b"snd_pcm_writei\0")?;
        let recover = self.alsa.sym::<unsafe extern "C" fn(Pcm, c_int, c_int) -> c_int>(b"snd_pcm_recover\0")?;
        let drain = self.alsa.sym::<unsafe extern "C" fn(Pcm) -> c_int>(b"snd_pcm_drain\0")?;
        let mut at = 0;
        let frames = samples.len() / 2;
        while at < frames {
            let rest = &samples[at * 2..];
            // SAFETY: rest holds (frames − at) stereo frames.
            let n = unsafe { write(self.pcm, rest.as_ptr() as *const c_void, (frames - at) as c_ulong) };
            if n < 0 {
                // (An underrun: taken up again, once.)
                // SAFETY: the PCM is open.
                self.alsa.check("playing", unsafe { recover(self.pcm, n as c_int, 1) })?;
                continue;
            }
            at += n as usize;
        }
        // SAFETY: the PCM is open.
        self.alsa.check("playing to the end", unsafe { drain(self.pcm) }).map(|_| ())
    }
}

impl Drop for Playback<'_> {
    fn drop(&mut self) {
        if let Ok(close) = self.alsa.sym::<unsafe extern "C" fn(Pcm) -> c_int>(b"snd_pcm_close\0") {
            // SAFETY: opened by snd_pcm_open.
            unsafe { close(self.pcm) };
        }
    }
}

//! alsa-lib, loaded as olo-chime runs (`libasound.so.2`, there wherever
//! PipeWire is; no headers to build against): a card's use-case manager
//! (UCM: which of its paths to switch on for the speakers, as WirePlumber
//! asks it), a PCM to write the chime to, and a card's mixer read (never
//! set): how far its speakers' own volume is turned down, and whether
//! they're switched off. Only what olo-chime uses.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};

use libloading::{Library, Symbol};

/// `SND_PCM_STREAM_PLAYBACK`, `SND_PCM_FORMAT_S16_LE`,
/// `SND_PCM_FORMAT_S32_LE`, `SND_PCM_ACCESS_RW_INTERLEAVED`.
const PLAYBACK: c_int = 0;
const S16_LE: c_int = 2;
const S32_LE: c_int = 10;
const RW_INTERLEAVED: c_int = 3;

pub struct Alsa {
    lib: Library,
}

type Ucm = *mut c_void;
type Pcm = *mut c_void;
type MixerHandle = *mut c_void;
type Elem = *mut c_void;

/// A mixer control for playback, as it is now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Control {
    /// How far below its loudest its volume is, decibels (0 at its
    /// loudest; none: it has no volume).
    pub db: Option<f32>,
    /// Its switch (none: it has no switch).
    pub on: Option<bool>,
}

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

    /// Card `card`'s mixer ("hw:1"), to read.
    pub fn mixer(&self, card: &str) -> Result<Mixer<'_>, String> {
        let open = self.sym::<unsafe extern "C" fn(*mut MixerHandle, c_int) -> c_int>(b"snd_mixer_open\0")?;
        let attach = self.sym::<unsafe extern "C" fn(MixerHandle, *const c_char) -> c_int>(b"snd_mixer_attach\0")?;
        let register = self.sym::<unsafe extern "C" fn(MixerHandle, *mut c_void, *mut c_void) -> c_int>(b"snd_mixer_selem_register\0")?;
        let load = self.sym::<unsafe extern "C" fn(MixerHandle) -> c_int>(b"snd_mixer_load\0")?;
        let name = CString::new(card).map_err(|e| e.to_string())?;
        let mut handle: MixerHandle = std::ptr::null_mut();
        // SAFETY: handle is written on success.
        self.check("its mixer", unsafe { open(&mut handle, 0) })?;
        let mixer = Mixer { alsa: self, handle };
        // SAFETY: the mixer is open; attached, its simple controls
        // registered and loaded, as alsa-lib's own programs do.
        self.check("its mixer", unsafe { attach(handle, name.as_ptr()) })?;
        self.check("its mixer", unsafe { register(handle, std::ptr::null_mut(), std::ptr::null_mut()) })?;
        self.check("its mixer", unsafe { load(handle) })?;
        Ok(mixer)
    }

    /// PCM `name` opened for playback at 48 kHz stereo, 32-bit (16 where
    /// it takes no more; `plughw` makes what the card takes of it).
    pub fn pcm(&self, name: &str) -> Result<Playback<'_>, String> {
        let open = self.sym::<unsafe extern "C" fn(*mut Pcm, *const c_char, c_int, c_int) -> c_int>(b"snd_pcm_open\0")?;
        let set = self.sym::<unsafe extern "C" fn(Pcm, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int>(b"snd_pcm_set_params\0")?;
        let cname = CString::new(name).map_err(|e| e.to_string())?;
        let mut pcm: Pcm = std::ptr::null_mut();
        // SAFETY: pcm is written on success.
        self.check(name, unsafe { open(&mut pcm, cname.as_ptr(), PLAYBACK, 0) })?;
        let mut p = Playback { alsa: self, pcm, wide: true };
        // (A fifth of a second buffered: soft resampling allowed.)
        // SAFETY: pcm is open.
        if unsafe { set(pcm, S32_LE, RW_INTERLEAVED, 2, olo_chime::RATE, 1, 200_000) } < 0 {
            p.wide = false;
            // SAFETY: pcm is open.
            self.check(name, unsafe { set(pcm, S16_LE, RW_INTERLEAVED, 2, olo_chime::RATE, 1, 200_000) })?;
        }
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

/// A card's mixer, open to read.
pub struct Mixer<'a> {
    alsa: &'a Alsa,
    handle: MixerHandle,
}

impl Mixer<'_> {
    /// The playback control named `name` (`index`: 0 but for a card with
    /// several of a name), as it is now: none if the card has no such.
    pub fn playback(&self, name: &str, index: u32) -> Option<Control> {
        let a = self.alsa;
        let malloc = a.sym::<unsafe extern "C" fn(*mut *mut c_void) -> c_int>(b"snd_mixer_selem_id_malloc\0").ok()?;
        let free = a.sym::<unsafe extern "C" fn(*mut c_void)>(b"snd_mixer_selem_id_free\0").ok()?;
        let set_name = a.sym::<unsafe extern "C" fn(*mut c_void, *const c_char)>(b"snd_mixer_selem_id_set_name\0").ok()?;
        let set_index = a.sym::<unsafe extern "C" fn(*mut c_void, c_uint)>(b"snd_mixer_selem_id_set_index\0").ok()?;
        let find = a.sym::<unsafe extern "C" fn(MixerHandle, *const c_void) -> Elem>(b"snd_mixer_find_selem\0").ok()?;
        let has = |f: &[u8]| a.sym::<unsafe extern "C" fn(Elem) -> c_int>(f).ok();
        let (has_volume, has_switch) = (has(b"snd_mixer_selem_has_playback_volume\0")?, has(b"snd_mixer_selem_has_playback_switch\0")?);
        let one = |f: &[u8]| a.sym::<unsafe extern "C" fn(Elem, c_int, *mut c_long) -> c_int>(f).ok();
        let range = |f: &[u8]| a.sym::<unsafe extern "C" fn(Elem, *mut c_long, *mut c_long) -> c_int>(f).ok();
        let (get_db, db_range) = (one(b"snd_mixer_selem_get_playback_dB\0")?, range(b"snd_mixer_selem_get_playback_dB_range\0")?);
        let (get_volume, volume_range) = (one(b"snd_mixer_selem_get_playback_volume\0")?, range(b"snd_mixer_selem_get_playback_volume_range\0")?);
        let get_switch = a.sym::<unsafe extern "C" fn(Elem, c_int, *mut c_int) -> c_int>(b"snd_mixer_selem_get_playback_switch\0").ok()?;
        let cname = CString::new(name).ok()?;
        let mut id: *mut c_void = std::ptr::null_mut();
        // SAFETY: id is written on success, and freed below.
        if unsafe { malloc(&mut id) } < 0 || id.is_null() {
            return None;
        }
        // SAFETY: id is ours; the mixer is open and loaded.
        let elem = unsafe {
            set_name(id, cname.as_ptr());
            set_index(id, index);
            let e = find(self.handle, id);
            free(id);
            e
        };
        if elem.is_null() {
            return None;
        }
        // (Its first channel's, the front left: both are set alike.)
        let (mut now, mut least, mut most): (c_long, c_long, c_long) = (0, 0, 0);
        // SAFETY: elem is the mixer's, which is open; each value is
        // written on success.
        let db = unsafe {
            if has_volume(elem) == 0 {
                None
            } else if get_db(elem, 0, &mut now) >= 0 && db_range(elem, &mut least, &mut most) >= 0 {
                // (Hundredths of a decibel.)
                Some(((now - most) as f32 / 100.0).min(0.0))
            } else if get_volume(elem, 0, &mut now) >= 0 && volume_range(elem, &mut least, &mut most) >= 0 && most > least {
                // (No decibels: its steps, as a volume's own scale.)
                Some(olo_chime::level::db(olo_chime::level::gain((now - least) as f32 / (most - least) as f32)).max(-120.0))
            } else {
                None
            }
        };
        let mut switch: c_int = 1;
        // SAFETY: as above.
        let on = unsafe { (has_switch(elem) != 0 && get_switch(elem, 0, &mut switch) >= 0).then_some(switch != 0) };
        Some(Control { db, on })
    }
}

impl Drop for Mixer<'_> {
    fn drop(&mut self) {
        if let Ok(close) = self.alsa.sym::<unsafe extern "C" fn(MixerHandle) -> c_int>(b"snd_mixer_close\0") {
            // SAFETY: opened by snd_mixer_open.
            unsafe { close(self.handle) };
        }
    }
}

/// An open playback PCM.
pub struct Playback<'a> {
    alsa: &'a Alsa,
    pcm: Pcm,
    /// It takes 32-bit samples (else 16).
    wide: bool,
}

impl Playback<'_> {
    /// `samples` (stereo, interleaved, 32-bit) played, then drained:
    /// returns when the card has played it all.
    pub fn play(&self, samples: &[i32]) -> Result<(), String> {
        if self.wide {
            return self.write(samples.as_ptr().cast(), samples.len() / 2, 8);
        }
        let narrow: Vec<i16> = samples.iter().map(|s| (s.saturating_add(1 << 15) >> 16) as i16).collect();
        self.write(narrow.as_ptr().cast(), narrow.len() / 2, 4)
    }

    /// `frames` stereo frames at `samples`, each `frame` bytes.
    fn write(&self, samples: *const c_void, frames: usize, frame: usize) -> Result<(), String> {
        let write = self.alsa.sym::<unsafe extern "C" fn(Pcm, *const c_void, c_ulong) -> c_long>(b"snd_pcm_writei\0")?;
        let recover = self.alsa.sym::<unsafe extern "C" fn(Pcm, c_int, c_int) -> c_int>(b"snd_pcm_recover\0")?;
        let drain = self.alsa.sym::<unsafe extern "C" fn(Pcm) -> c_int>(b"snd_pcm_drain\0")?;
        let mut at = 0;
        while at < frames {
            // SAFETY: (frames − at) stereo frames are left from there.
            let n = unsafe { write(self.pcm, samples.byte_add(at * frame), (frames - at) as c_ulong) };
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

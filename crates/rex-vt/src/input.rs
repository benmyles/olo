//! Key, mouse, paste, focus, and color-scheme encoding via `libghostty-vt`.

use std::{ffi::c_void, mem, ptr};

use crate::{
    error::{VtError, check},
    ffi,
    terminal::GhosttyTerminal,
};

/// Keyboard modifiers bitmask matching `GHOSTTY_MODS_*`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyMods(pub u16);

impl KeyMods {
    pub const NONE: Self = Self(0);
    pub const SHIFT: u16 = 1 << 0;
    pub const CTRL: u16 = 1 << 1;
    pub const ALT: u16 = 1 << 2;
    pub const SUPER: u16 = 1 << 3;
    pub const CAPS_LOCK: u16 = 1 << 4;
    pub const NUM_LOCK: u16 = 1 << 5;
}

/// Mouse button identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Four,
    Five,
}

impl MouseButton {
    fn to_ffi(self) -> ffi::GhosttyMouseButton {
        match self {
            Self::Left => ffi::GHOSTTY_MOUSE_BUTTON_LEFT,
            Self::Right => ffi::GHOSTTY_MOUSE_BUTTON_RIGHT,
            Self::Middle => ffi::GHOSTTY_MOUSE_BUTTON_MIDDLE,
            Self::Four => ffi::GHOSTTY_MOUSE_BUTTON_FOUR,
            Self::Five => ffi::GHOSTTY_MOUSE_BUTTON_FIVE,
        }
    }
}

/// Mouse event action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

impl MouseAction {
    fn to_ffi(self) -> ffi::GhosttyMouseAction {
        match self {
            Self::Press => ffi::GHOSTTY_MOUSE_ACTION_PRESS,
            Self::Release => ffi::GHOSTTY_MOUSE_ACTION_RELEASE,
            Self::Motion => ffi::GHOSTTY_MOUSE_ACTION_MOTION,
        }
    }
}

/// Geometry used by [`MouseEncoder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseGeometry {
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}

/// Reusable Ghostty mouse encoder.
pub struct MouseEncoder {
    encoder: ffi::GhosttyMouseEncoder,
    event: ffi::GhosttyMouseEvent,
}

// SAFETY: both native handles are uniquely owned, have no thread affinity and are never shared.
unsafe impl Send for MouseEncoder {}

impl MouseEncoder {
    pub fn new() -> Result<Self, VtError> {
        crate::sys::init()?;
        let mut encoder: ffi::GhosttyMouseEncoder = ptr::null_mut();
        // SAFETY: The default allocator is permitted and encoder is a writable output slot.
        check(unsafe { ffi::ghostty_mouse_encoder_new(ptr::null(), &mut encoder) })?;
        let mut event: ffi::GhosttyMouseEvent = ptr::null_mut();
        // SAFETY: The event output is distinct from the live encoder and receives ownership on success.
        if let Err(e) = check(unsafe { ffi::ghostty_mouse_event_new(ptr::null(), &mut event) }) {
            // SAFETY: Construction failed after allocating the encoder, whose sole owner is still this function.
            unsafe { ffi::ghostty_mouse_encoder_free(encoder) };
            return Err(e);
        }
        let owned = Self { encoder, event };
        if encoder.is_null() || event.is_null() {
            return Err(VtError::InvalidValue);
        }
        Ok(owned)
    }

    pub fn sync_from_terminal(&mut self, term: &GhosttyTerminal) -> Result<(), VtError> {
        let _access = term.lock();
        // SAFETY: The terminal lock covers the copy; the encoder retains only its tracking settings.
        check(unsafe { ffi::ghostty_mouse_encoder_set_from_terminal(self.encoder, term.raw()) })
    }

    pub fn set_geometry(&mut self, g: MouseGeometry) -> Result<(), VtError> {
        if g.padding_left.saturating_add(g.padding_right) >= g.screen_width.max(1) || g.padding_top.saturating_add(g.padding_bottom) >= g.screen_height.max(1) {
            return Err(VtError::InvalidValue);
        }
        let size = ffi::GhosttyMouseEncoderSize {
            size: mem::size_of::<ffi::GhosttyMouseEncoderSize>(),
            screen_width: g.screen_width.max(1),
            screen_height: g.screen_height.max(1),
            cell_width: g.cell_width.max(1),
            cell_height: g.cell_height.max(1),
            padding_top: g.padding_top,
            padding_bottom: g.padding_bottom,
            padding_right: g.padding_right,
            padding_left: g.padding_left,
        };
        // SAFETY: SIZE copies an initialized, size-tagged geometry with nonzero cells.
        check(unsafe { ffi::ghostty_mouse_encoder_set(self.encoder, ffi::GHOSTTY_MOUSE_ENCODER_OPT_SIZE, ptr::from_ref(&size).cast::<c_void>()) })
    }

    pub fn set_any_button_pressed(&mut self, pressed: bool) -> Result<(), VtError> {
        // SAFETY: The option copies one C bool from a live Rust bool.
        check(unsafe { ffi::ghostty_mouse_encoder_set(self.encoder, ffi::GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED, (&pressed as *const bool).cast::<c_void>()) })
    }

    pub fn encode(&mut self, action: MouseAction, button: Option<MouseButton>, mods: KeyMods, x_px: f32, y_px: f32) -> Result<Vec<u8>, VtError> {
        if !x_px.is_finite() || !y_px.is_finite() || x_px.abs() >= i32::MAX as f32 || y_px.abs() >= i32::MAX as f32 {
            return Err(VtError::InvalidValue);
        }
        // SAFETY: The event is uniquely owned; action and button are known tags and the position is finite.
        unsafe {
            check(ffi::ghostty_mouse_event_set_action(self.event, action.to_ffi()))?;
            match button {
                Some(b) => check(ffi::ghostty_mouse_event_set_button(self.event, b.to_ffi()))?,
                None => check(ffi::ghostty_mouse_event_clear_button(self.event))?,
            }
            check(ffi::ghostty_mouse_event_set_mods(self.event, mods.0))?;
            check(ffi::ghostty_mouse_event_set_position(self.event, ffi::GhosttyMousePosition { x: x_px, y: y_px }))?;
        }
        let mut buf = [0u8; 128];
        let mut written = 0usize;
        // SAFETY: Both handles are live and the byte buffer and length output are separate writable objects.
        let res = unsafe { ffi::ghostty_mouse_encoder_encode(self.encoder, self.event, buf.as_mut_ptr(), buf.len(), &mut written) };
        if res == ffi::GHOSTTY_OUT_OF_SPACE {
            if written > 4096 {
                return Err(VtError::LimitExceeded);
            }
            let mut dyn_buf = vec![0u8; written];
            // SAFETY: Both handles are live and the byte buffer and length output are separate writable objects.
            check(unsafe { ffi::ghostty_mouse_encoder_encode(self.encoder, self.event, dyn_buf.as_mut_ptr(), dyn_buf.len(), &mut written) })?;
            if written > dyn_buf.len() {
                return Err(VtError::InvalidValue);
            }
            dyn_buf.truncate(written);
            return Ok(dyn_buf);
        }
        check(res)?;
        Ok(buf.get(..written).ok_or(VtError::InvalidValue)?.to_vec())
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        // SAFETY: These are the two uniquely owned handles returned by successful constructors.
        unsafe {
            if !self.event.is_null() {
                ffi::ghostty_mouse_event_free(self.event);
            }
            if !self.encoder.is_null() {
                ffi::ghostty_mouse_encoder_free(self.encoder);
            }
        }
    }
}

/// Check whether `text` is safe to paste without risking command injection.
pub fn paste_is_safe(text: &str) -> bool {
    let _ = crate::sys::init();
    // SAFETY: Paste safety reads precisely text.len() initialized bytes, including embedded NULs.
    unsafe { ffi::ghostty_paste_is_safe(text.as_ptr().cast(), text.len()) }
}

/// Encode `text` for pasting into a terminal (honoring bracketed paste mode).
pub fn encode_paste(text: &str, bracketed: bool) -> Result<Vec<u8>, VtError> {
    crate::sys::init()?;
    if text.len() > crate::limits::IMAGE_BYTES {
        return Err(VtError::LimitExceeded);
    }
    let mut input = text.as_bytes().to_vec();
    let mut needed = 0usize;
    // SAFETY: The encoder may sanitize the mutable copy; NULL with zero capacity asks for its output size.
    let res = unsafe { ffi::ghostty_paste_encode(input.as_mut_ptr().cast(), input.len(), bracketed, ptr::null_mut(), 0, &mut needed) };
    if res == ffi::GHOSTTY_SUCCESS && needed == 0 {
        return Ok(Vec::new());
    }
    if res != ffi::GHOSTTY_OUT_OF_SPACE && res != ffi::GHOSTTY_SUCCESS {
        return Err(VtError::from_ffi(res));
    }
    if needed > input.len() + 12 {
        return Err(VtError::InvalidValue);
    }
    let mut out = vec![0u8; needed];
    let mut written = 0usize;
    // SAFETY: Input and output are distinct owned buffers with the advertised lengths.
    check(unsafe { ffi::ghostty_paste_encode(input.as_mut_ptr().cast(), input.len(), bracketed, out.as_mut_ptr(), out.len(), &mut written) })?;
    if written > out.len() {
        return Err(VtError::InvalidValue);
    }
    out.truncate(written);
    Ok(out)
}

/// Encode a focus-in (`CSI I`) or focus-out (`CSI O`) report.
pub fn encode_focus(focused: bool) -> Vec<u8> {
    let _ = crate::sys::init();
    let ev = if focused { ffi::GHOSTTY_FOCUS_GAINED } else { ffi::GHOSTTY_FOCUS_LOST };
    let mut buf = [0u8; 16];
    let mut written = 0usize;
    // SAFETY: The focus tag is known and Ghostty receives the full writable buffer and a size_t output.
    let res = unsafe { ffi::ghostty_focus_encode(ev, buf.as_mut_ptr().cast(), buf.len(), &mut written) };
    if res == ffi::GHOSTTY_SUCCESS && written <= buf.len() { buf[..written].to_vec() } else { Vec::new() }
}

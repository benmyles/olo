use std::{env, path::PathBuf, rc::Rc, sync::Arc};
use std::{
    ffi::OsString,
    fs::File,
    io::Read as _,
    os::fd::{AsFd, AsRawFd, FromRawFd},
    time::Duration,
};

use async_task::Runnable;
use calloop::{LoopSignal, channel::Channel};
use util::ResultExt as _;
use xkbcommon::xkb::{self, Keycode, Keysym, State};

use crate::{
    BackgroundExecutor, ClipboardItem, CursorStyle, DisplayId, ForegroundExecutor, LinuxDispatcher, Pixels, PlatformDisplay, PlatformKeyboardLayout, PlatformTextSystem, PlatformWindow, Point, Result,
    WindowAppearance, WindowId, WindowParams, px,
};

pub(crate) const SCROLL_LINES: f32 = 3.0;

// Values match the defaults on GTK.
// Taken from https://github.com/GNOME/gtk/blob/main/gtk/gtksettings.c#L320
pub(crate) const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);
pub(crate) const DOUBLE_CLICK_DISTANCE: Pixels = px(5.0);
pub(crate) const KEYRING_LABEL: &str = "zed-github-account";

const FILE_PICKER_PORTAL_MISSING: &str = "Couldn't open file picker due to missing xdg-desktop-portal implementation.";

pub trait LinuxClient {
    fn compositor_name(&self) -> &'static str;
    fn with_common<R>(&self, f: impl FnOnce(&mut LinuxCommon) -> R) -> R;
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout>;
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>>;
    #[allow(unused)]
    fn display(&self, id: DisplayId) -> Option<Rc<dyn PlatformDisplay>>;
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>>;

    fn open_window(&self, handle: WindowId, options: WindowParams) -> anyhow::Result<Box<dyn PlatformWindow>>;
    fn set_cursor_style(&self, style: CursorStyle);
    fn open_uri(&self, uri: &str);
    fn reveal_path(&self, path: PathBuf);
    fn write_to_primary(&self, item: ClipboardItem);
    fn write_to_clipboard(&self, item: ClipboardItem);
    /// takes `item` off the clipboard if it's still ours.
    fn withdraw_clipboard(&self, _item: &ClipboardItem) {}
    /// quitting: whether what's on the clipboard is ours, in which
    /// case it runs on until that's gone (`withdraw_clipboard`, or
    /// something else copied) so what was copied can still be pasted.
    fn linger_for_clipboard(&self) -> bool {
        false
    }
    /// whether what's on the clipboard is ours.
    fn holds_clipboard(&self) -> bool {
        false
    }
    /// as `Platform::external_modifiers`.
    fn external_modifiers(&self, _fourcc: u32) -> Vec<u64> {
        Vec::new()
    }
    fn read_from_primary(&self) -> Option<ClipboardItem>;
    fn read_from_clipboard(&self) -> Option<ClipboardItem>;
    /// what's on the clipboard if it's ours.
    fn own_clipboard(&self) -> Option<ClipboardItem> {
        None
    }
    /// the types on the clipboard now.
    fn clipboard_types(&self) -> Vec<String> {
        Vec::new()
    }
    /// asks for the clipboard as `mime`.
    fn receive_clipboard(&self, _mime: &str) -> Option<crate::ClipboardReceipt> {
        None
    }
    fn active_window(&self) -> Option<WindowId>;
    fn window_stack(&self) -> Option<Vec<WindowId>>;
    fn run(&self);
}

#[derive(Default)]
pub(crate) struct PlatformHandlers {
    pub(crate) quit: Option<Box<dyn FnMut()>>,
    pub(crate) keyboard_layout_change: Option<Box<dyn FnMut()>>,
    /// a display came or went (`App::on_displays_changed`).
    pub(crate) displays_changed: Option<Box<dyn FnMut()>>,
    /// the compositor's connection is gone (`App::on_compositor_lost`).
    pub(crate) compositor_lost: Option<Box<dyn FnMut()>>,
    /// what's on the clipboard changed.
    pub(crate) clipboard_changed: Option<Box<dyn FnMut()>>,
}

pub(crate) struct LinuxCommon {
    pub(crate) background_executor: BackgroundExecutor,
    pub(crate) foreground_executor: ForegroundExecutor,
    pub(crate) text_system: Arc<dyn PlatformTextSystem>,
    pub(crate) appearance: WindowAppearance,
    pub(crate) auto_hide_scrollbars: bool,
    pub(crate) callbacks: PlatformHandlers,
    pub(crate) signal: LoopSignal,
    /// quit asked for. The event loop's `run` clears a stop sent
    /// before it starts, so a quit while launching (olo-lock with no
    /// compositor to lock) was lost and the process idled for ever.
    pub(crate) quitting: bool,
}

impl LinuxCommon {
    pub fn new(signal: LoopSignal) -> (Self, Channel<Runnable>) {
        let (main_sender, main_receiver) = calloop::channel::channel::<Runnable>();

        let text_system = Arc::new(crate::OloTextSystem::new());

        let callbacks = PlatformHandlers::default();

        let dispatcher = Arc::new(LinuxDispatcher::new(main_sender));

        let background_executor = BackgroundExecutor::new(dispatcher.clone());

        let common = LinuxCommon {
            background_executor,
            foreground_executor: ForegroundExecutor::new(dispatcher),
            text_system,
            appearance: WindowAppearance::Light,
            auto_hide_scrollbars: false,
            callbacks,
            signal,
            quitting: false,
        };

        (common, main_receiver)
    }
}

/// asks the scheduler to take this thread as at least
/// `olo_protocol::performance::UI_UTIL_MIN` busy while it runs (its
/// uclamp.min; `OLO_UTIL_MIN` instead, 0 for none, to compare): a frame
/// on a slow core at a low clock took three or four times as long. The
/// threads it starts from now on don't inherit it (or a raised
/// priority): their work isn't a frame's.
pub(crate) fn util_floor() {
    let min = env::var("OLO_UTIL_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(olo_protocol::performance::UI_UTIL_MIN);
    // (Not on an agent's screen: background work beside the person's
    // desktop, `olo_system::priority::not_the_persons`.)
    if min == 0 || env::var_os("OLO_SCREEN").is_some() {
        return;
    }
    #[repr(C)]
    #[derive(Default)]
    struct SchedAttr {
        size: u32,
        policy: u32,
        flags: u64,
        nice: i32,
        priority: u32,
        runtime: u64,
        deadline: u64,
        period: u64,
        util_min: u32,
        util_max: u32,
    }
    const RESET_ON_FORK: u64 = 0x01;
    const KEEP_PARAMS: u64 = 0x10;
    const UTIL_CLAMP_MIN: u64 = 0x20;
    let attr = SchedAttr { size: size_of::<SchedAttr>() as u32, policy: libc::SCHED_OTHER as u32, flags: RESET_ON_FORK | KEEP_PARAMS | UTIL_CLAMP_MIN, util_min: min.min(1024), ..Default::default() };
    // SAFETY: sched_setattr reads `attr`, of the size it's told, for this
    // thread (0).
    if unsafe { libc::syscall(libc::SYS_sched_setattr, 0, &attr as *const SchedAttr, 0) } != 0 {
        log::info!("the windows' thread keeps the scheduler's choice of core: {}", std::io::Error::last_os_error());
    }
}

pub(super) fn open_uri_internal(executor: BackgroundExecutor, uri: &str, activation_token: Option<String>) {
    if let Some(uri) = ashpd::url::Url::parse(uri).log_err() {
        executor
            .spawn(async move {
                match ashpd::desktop::open_uri::OpenFileRequest::default().activation_token(activation_token.clone().map(ashpd::ActivationToken::from)).send_uri(&uri).await {
                    Ok(_) => {}
                    // (Through the portal only: Olo wraps no programs such
                    // as xdg-open.)
                    Err(e) => log::error!("Failed to open with dbus: {}", e),
                }
            })
            .detach();
    }
}

pub(super) fn reveal_path_internal(executor: BackgroundExecutor, path: PathBuf, activation_token: Option<String>) {
    executor
        .spawn(async move {
            if let Some(dir) = File::open(path.clone()).log_err() {
                match ashpd::desktop::open_uri::OpenDirectoryRequest::default().activation_token(activation_token.map(ashpd::ActivationToken::from)).send(&dir.as_fd()).await {
                    Ok(_) => {}
                    // (Through the portal only, as `open_uri_internal`.)
                    Err(e) => log::error!("Failed to open with dbus: {}", e),
                }
            }
        })
        .detach();
}

#[allow(unused)]
pub(super) fn is_within_click_distance(a: Point<Pixels>, b: Point<Pixels>) -> bool {
    let diff = a - b;
    diff.x.abs() <= DOUBLE_CLICK_DISTANCE && diff.y.abs() <= DOUBLE_CLICK_DISTANCE
}

pub(super) fn get_xkb_compose_state(cx: &xkb::Context) -> Option<xkb::compose::State> {
    let mut locales = Vec::default();
    if let Some(locale) = env::var_os("LC_CTYPE") {
        locales.push(locale);
    }
    locales.push(OsString::from("C"));
    let mut state: Option<xkb::compose::State> = None;
    for locale in locales {
        if let Ok(table) = xkb::compose::Table::new_from_locale(cx, &locale, xkb::compose::COMPILE_NO_FLAGS) {
            state = Some(xkb::compose::State::new(&table, xkb::compose::STATE_NO_FLAGS));
            break;
        }
    }
    state
}

pub(super) unsafe fn read_fd(mut fd: filedescriptor::FileDescriptor) -> Result<Vec<u8>> {
    let mut file = unsafe { File::from_raw_fd(fd.as_raw_fd()) };
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(buffer)
}

pub(super) const DEFAULT_CURSOR_ICON_NAME: &str = "left_ptr";

impl CursorStyle {
    pub(super) fn to_icon_names(self) -> &'static [&'static str] {
        // Based on cursor names from chromium:
        // https://github.com/chromium/chromium/blob/d3069cf9c973dc3627fa75f64085c6a86c8f41bf/ui/base/cursor/cursor_factory.cc#L113
        match self {
            CursorStyle::Arrow => &[DEFAULT_CURSOR_ICON_NAME],
            CursorStyle::IBeam => &["text", "xterm"],
            CursorStyle::Crosshair => &["crosshair", "cross"],
            CursorStyle::ClosedHand => &["closedhand", "grabbing", "hand2"],
            CursorStyle::OpenHand => &["openhand", "grab", "hand1"],
            CursorStyle::PointingHand => &["pointer", "hand", "hand2"],
            CursorStyle::ResizeLeft => &["w-resize", "left_side"],
            CursorStyle::ResizeRight => &["e-resize", "right_side"],
            CursorStyle::ResizeLeftRight => &["ew-resize", "sb_h_double_arrow"],
            CursorStyle::ResizeUp => &["n-resize", "top_side"],
            CursorStyle::ResizeDown => &["s-resize", "bottom_side"],
            CursorStyle::ResizeUpDown => &["sb_v_double_arrow", "ns-resize"],
            CursorStyle::ResizeUpLeftDownRight => &["size_fdiag", "bd_double_arrow", "nwse-resize"],
            CursorStyle::ResizeUpRightDownLeft => &["size_bdiag", "nesw-resize", "fd_double_arrow"],
            CursorStyle::ResizeColumn => &["col-resize", "sb_h_double_arrow"],
            CursorStyle::ResizeRow => &["row-resize", "sb_v_double_arrow"],
            CursorStyle::IBeamCursorForVerticalLayout => &["vertical-text"],
            CursorStyle::OperationNotAllowed => &["not-allowed", "crossed_circle"],
            CursorStyle::DragLink => &["alias"],
            CursorStyle::DragCopy => &["copy"],
            CursorStyle::ContextualMenu => &["context-menu"],
            CursorStyle::None => {
                #[cfg(debug_assertions)]
                panic!("CursorStyle::None should be handled separately in the client");
                #[cfg(not(debug_assertions))]
                &[DEFAULT_CURSOR_ICON_NAME]
            }
        }
    }
}

pub(super) fn log_cursor_icon_warning(message: impl std::fmt::Display) {
    if let Ok(xcursor_path) = env::var("XCURSOR_PATH") {
        log::warn!(
            "{:#}\ncursor icon loading may be failing if XCURSOR_PATH environment variable is invalid. \
                    XCURSOR_PATH overrides the default icon search. Its current value is '{}'",
            message,
            xcursor_path
        );
    } else {
        log::warn!("{:#}", message);
    }
}

fn guess_ascii(keycode: Keycode, shift: bool) -> Option<char> {
    let c = match (keycode.raw(), shift) {
        (24, _) => 'q',
        (25, _) => 'w',
        (26, _) => 'e',
        (27, _) => 'r',
        (28, _) => 't',
        (29, _) => 'y',
        (30, _) => 'u',
        (31, _) => 'i',
        (32, _) => 'o',
        (33, _) => 'p',
        (34, false) => '[',
        (34, true) => '{',
        (35, false) => ']',
        (35, true) => '}',
        (38, _) => 'a',
        (39, _) => 's',
        (40, _) => 'd',
        (41, _) => 'f',
        (42, _) => 'g',
        (43, _) => 'h',
        (44, _) => 'j',
        (45, _) => 'k',
        (46, _) => 'l',
        (47, false) => ';',
        (47, true) => ':',
        (48, false) => '\'',
        (48, true) => '"',
        (49, false) => '`',
        (49, true) => '~',
        (51, false) => '\\',
        (51, true) => '|',
        (52, _) => 'z',
        (53, _) => 'x',
        (54, _) => 'c',
        (55, _) => 'v',
        (56, _) => 'b',
        (57, _) => 'n',
        (58, _) => 'm',
        (59, false) => ',',
        (59, true) => '>',
        (60, false) => '.',
        (60, true) => '<',
        (61, false) => '/',
        (61, true) => '?',

        _ => return None,
    };

    Some(c)
}

impl crate::Keystroke {
    pub(super) fn from_xkb(state: &State, mut modifiers: crate::Modifiers, keycode: Keycode) -> Self {
        let key_utf32 = state.key_get_utf32(keycode);
        let key_utf8 = state.key_get_utf8(keycode);
        let key_sym = state.key_get_one_sym(keycode);

        let key = match key_sym {
            Keysym::Return => "enter".to_owned(),
            Keysym::Prior => "pageup".to_owned(),
            Keysym::Next => "pagedown".to_owned(),
            Keysym::ISO_Left_Tab => "tab".to_owned(),
            Keysym::KP_Prior => "pageup".to_owned(),
            Keysym::KP_Next => "pagedown".to_owned(),
            Keysym::XF86_Back => "back".to_owned(),
            Keysym::XF86_Forward => "forward".to_owned(),
            Keysym::XF86_Cut => "cut".to_owned(),
            Keysym::XF86_Copy => "copy".to_owned(),
            Keysym::XF86_Paste => "paste".to_owned(),
            Keysym::XF86_New => "new".to_owned(),
            Keysym::XF86_Open => "open".to_owned(),
            Keysym::XF86_Save => "save".to_owned(),

            Keysym::comma => ",".to_owned(),
            Keysym::period => ".".to_owned(),
            Keysym::less => "<".to_owned(),
            Keysym::greater => ">".to_owned(),
            Keysym::slash => "/".to_owned(),
            Keysym::question => "?".to_owned(),

            Keysym::semicolon => ";".to_owned(),
            Keysym::colon => ":".to_owned(),
            Keysym::apostrophe => "'".to_owned(),
            Keysym::quotedbl => "\"".to_owned(),

            Keysym::bracketleft => "[".to_owned(),
            Keysym::braceleft => "{".to_owned(),
            Keysym::bracketright => "]".to_owned(),
            Keysym::braceright => "}".to_owned(),
            Keysym::backslash => "\\".to_owned(),
            Keysym::bar => "|".to_owned(),

            Keysym::grave => "`".to_owned(),
            Keysym::asciitilde => "~".to_owned(),
            Keysym::exclam => "!".to_owned(),
            Keysym::at => "@".to_owned(),
            Keysym::numbersign => "#".to_owned(),
            Keysym::dollar => "$".to_owned(),
            Keysym::percent => "%".to_owned(),
            Keysym::asciicircum => "^".to_owned(),
            Keysym::ampersand => "&".to_owned(),
            Keysym::asterisk => "*".to_owned(),
            Keysym::parenleft => "(".to_owned(),
            Keysym::parenright => ")".to_owned(),
            Keysym::minus => "-".to_owned(),
            Keysym::underscore => "_".to_owned(),
            Keysym::equal => "=".to_owned(),
            Keysym::plus => "+".to_owned(),
            Keysym::space => "space".to_owned(),
            Keysym::BackSpace => "backspace".to_owned(),
            Keysym::Tab => "tab".to_owned(),
            Keysym::Delete => "delete".to_owned(),
            Keysym::Escape => "escape".to_owned(),

            Keysym::Left => "left".to_owned(),
            Keysym::Right => "right".to_owned(),
            Keysym::Up => "up".to_owned(),
            Keysym::Down => "down".to_owned(),
            Keysym::Home => "home".to_owned(),
            Keysym::End => "end".to_owned(),
            Keysym::Insert => "insert".to_owned(),

            _ => {
                let name = xkb::keysym_get_name(key_sym).to_lowercase();
                if key_sym.is_keypad_key() {
                    name.replace("kp_", "")
                } else if let Some(key) = key_utf8.chars().next()
                    && key_utf8.len() == 1
                    && key.is_ascii()
                {
                    if key.is_ascii_graphic() {
                        key_utf8.to_lowercase()
                    // map ctrl-a to `a`
                    // ctrl-0..9 may emit control codes like ctrl-[, but
                    // we don't want to map them to `[`
                    } else if key_utf32 <= 0x1f && !name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        ((key_utf32 as u8 + 0x40) as char).to_ascii_lowercase().to_string()
                    } else {
                        name
                    }
                } else if let Some(key_en) = guess_ascii(keycode, modifiers.shift) {
                    String::from(key_en)
                } else {
                    name
                }
            }
        };

        if modifiers.shift {
            // we only include the shift for upper-case letters by convention,
            // so don't include for numbers and symbols, but do include for
            // tab/enter, etc.
            if key.chars().count() == 1 && key.to_lowercase() == key.to_uppercase() {
                modifiers.shift = false;
            }
        }

        // Ignore control characters (and DEL) for the purposes of key_char
        let key_char = (key_utf32 >= 32 && key_utf32 != 127 && !key_utf8.is_empty()).then_some(key_utf8);

        Self { modifiers, key, key_char }
    }

    /**
     * Returns which symbol the dead key represents
     * <https://developer.mozilla.org/en-US/docs/Web/API/UI_Events/Keyboard_event_key_values#dead_keycodes_for_linux>
     */
    pub fn underlying_dead_key(keysym: Keysym) -> Option<String> {
        match keysym {
            Keysym::dead_grave => Some("`".to_owned()),
            Keysym::dead_acute => Some("´".to_owned()),
            Keysym::dead_circumflex => Some("^".to_owned()),
            Keysym::dead_tilde => Some("~".to_owned()),
            Keysym::dead_macron => Some("¯".to_owned()),
            Keysym::dead_breve => Some("˘".to_owned()),
            Keysym::dead_abovedot => Some("˙".to_owned()),
            Keysym::dead_diaeresis => Some("¨".to_owned()),
            Keysym::dead_abovering => Some("˚".to_owned()),
            Keysym::dead_doubleacute => Some("˝".to_owned()),
            Keysym::dead_caron => Some("ˇ".to_owned()),
            Keysym::dead_cedilla => Some("¸".to_owned()),
            Keysym::dead_ogonek => Some("˛".to_owned()),
            Keysym::dead_iota => Some("ͅ".to_owned()),
            Keysym::dead_voiced_sound => Some("゙".to_owned()),
            Keysym::dead_semivoiced_sound => Some("゚".to_owned()),
            Keysym::dead_belowdot => Some("̣̣".to_owned()),
            Keysym::dead_hook => Some("̡".to_owned()),
            Keysym::dead_horn => Some("̛".to_owned()),
            Keysym::dead_stroke => Some("̶̶".to_owned()),
            Keysym::dead_abovecomma => Some("̓̓".to_owned()),
            Keysym::dead_abovereversedcomma => Some("ʽ".to_owned()),
            Keysym::dead_doublegrave => Some("̏".to_owned()),
            Keysym::dead_belowring => Some("˳".to_owned()),
            Keysym::dead_belowmacron => Some("̱".to_owned()),
            Keysym::dead_belowcircumflex => Some("ꞈ".to_owned()),
            Keysym::dead_belowtilde => Some("̰".to_owned()),
            Keysym::dead_belowbreve => Some("̮".to_owned()),
            Keysym::dead_belowdiaeresis => Some("̤".to_owned()),
            Keysym::dead_invertedbreve => Some("̯".to_owned()),
            Keysym::dead_belowcomma => Some("̦".to_owned()),
            Keysym::dead_currency => None,
            Keysym::dead_lowline => None,
            Keysym::dead_aboveverticalline => None,
            Keysym::dead_belowverticalline => None,
            Keysym::dead_longsolidusoverlay => None,
            Keysym::dead_a => None,
            Keysym::dead_A => None,
            Keysym::dead_e => None,
            Keysym::dead_E => None,
            Keysym::dead_i => None,
            Keysym::dead_I => None,
            Keysym::dead_o => None,
            Keysym::dead_O => None,
            Keysym::dead_u => None,
            Keysym::dead_U => None,
            Keysym::dead_small_schwa => Some("ə".to_owned()),
            Keysym::dead_capital_schwa => Some("Ə".to_owned()),
            Keysym::dead_greek => None,
            _ => None,
        }
    }
}

impl crate::Modifiers {
    pub(super) fn from_xkb(keymap_state: &State) -> Self {
        let shift = keymap_state.mod_name_is_active(xkb::MOD_NAME_SHIFT, xkb::STATE_MODS_EFFECTIVE);
        let alt = keymap_state.mod_name_is_active(xkb::MOD_NAME_ALT, xkb::STATE_MODS_EFFECTIVE);
        let control = keymap_state.mod_name_is_active(xkb::MOD_NAME_CTRL, xkb::STATE_MODS_EFFECTIVE);
        let platform = keymap_state.mod_name_is_active(xkb::MOD_NAME_LOGO, xkb::STATE_MODS_EFFECTIVE);
        Self { shift, alt, control, platform, function: false }
    }
}

impl crate::Capslock {
    pub(super) fn from_xkb(keymap_state: &State) -> Self {
        let on = keymap_state.mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE);
        Self { on }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Point, px};

    #[test]
    fn test_is_within_click_distance() {
        let zero = Point::new(px(0.0), px(0.0));
        assert!(is_within_click_distance(zero, Point::new(px(5.0), px(5.0))));
        assert!(is_within_click_distance(zero, Point::new(px(-4.9), px(5.0))));
        assert!(is_within_click_distance(Point::new(px(3.0), px(2.0)), Point::new(px(-2.0), px(-2.0))));
        assert!(!is_within_click_distance(zero, Point::new(px(5.0), px(5.1))),);
    }
}

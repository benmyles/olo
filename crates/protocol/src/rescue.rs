//! Olo Rescue: what app_server and olo-rescue agree on. Holding
//! Ctrl+Shift+Super and pressing Delete (or Backspace, a Mac's delete),
//! anywhere, whatever has the keyboard, puts every window away, runs the
//! displays, keyboard and pointer on safe settings and shows olo-rescue
//! full screen: a terminal, the processes (to end one), Preferences and
//! a way back ([`crate::Request::Rescue`]).

/// olo-rescue's app id.
pub const APP_ID: &str = "olo.rescue";
/// olo-rescue's program.
pub const PROGRAM: &str = "olo-rescue";
/// Its layer surfaces' namespace: in Rescue the only layer surfaces drawn
/// or reached (Deskbar, the desktop and banners are put away with the
/// windows).
pub const NAMESPACE: &str = "olo-rescue";
/// Set (to anything) in app_server's environment: it starts in Rescue
/// (the session does after app_server keeps failing to start).
pub const START_ENV: &str = "OLO_RESCUE";
/// The keys, as people read them.
pub const KEYS: &str = "Ctrl Shift Super Delete";

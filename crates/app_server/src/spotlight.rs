//! The spotlight while switching: as ⌘Tab chooses an app (or ⌘\` a
//! window), every other window is dimmed, so the eye goes to what is
//! chosen. `desktop.rs` brings the chosen windows to the front and names
//! them (`show`); the dim is drawn just under them (`render.rs`), over
//! every other window and the desktop, the Deskbar and Twitcher above
//! it. It fades in once the keys have been held a moment (a quick ⌘Tab
//! flips between two apps without it) and out as the choice is made;
//! Esc, putting everything back, takes it away at once.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

use smithay::{
    backend::renderer::{
        element::{Id, Kind, solid::SolidColorRenderElement},
        utils::CommitCounter,
    },
    utils::{Physical, Rectangle},
};

/// Held this long before the dim begins.
const WAIT: Duration = Duration::from_millis(90);
/// How long it takes to come and go.
const FADE: Duration = Duration::from_millis(140);
/// How dark the rest gets: black at this opacity.
const DIM: f32 = 0.4;

struct State {
    /// The windows kept bright (`WindowElement::id`).
    bright: HashSet<u64>,
    since: Instant,
    /// Fading out: from when, and how dark it was then.
    ending: Option<(Instant, f32)>,
}

impl State {
    fn level(&self, now: Instant) -> f32 {
        match self.ending {
            Some((at, from)) => from * (1.0 - fraction(now.saturating_duration_since(at))),
            None => DIM * fraction(now.saturating_duration_since(self.since).saturating_sub(WAIT)),
        }
    }
}

/// How far through a fade `t` is, eased.
fn fraction(t: Duration) -> f32 {
    let x = (t.as_secs_f32() / FADE.as_secs_f32()).clamp(0.0, 1.0);
    x * (2.0 - x)
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// What each display's dim was last drawn as, so it is damaged only as it
/// changes.
static DRAWN: Mutex<Option<HashMap<String, (Id, CommitCounter, f32)>>> = Mutex::new(None);

/// Keep `bright` lit and dim the rest (fading in, if it isn't yet).
pub fn show(bright: HashSet<u64>) {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    match state.as_mut().filter(|s| s.ending.is_none()) {
        Some(s) => s.bright = bright,
        None => *state = Some(State { bright, since: Instant::now(), ending: None }),
    }
}

/// The choice is made: fade out (`fade`), or (Esc, the lock) be gone now.
pub fn end(fade: bool) {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    match state.as_mut() {
        Some(s) if fade && s.ending.is_none() => s.ending = Some((now, s.level(now))),
        Some(_) if !fade => *state = None,
        _ => {}
    }
}

/// The windows kept bright and the dim to draw under them on `output`
/// (`area`, its device pixels), while there is one.
pub fn dim(output: &str, area: Rectangle<i32, Physical>) -> Option<(HashSet<u64>, SolidColorRenderElement)> {
    let now = Instant::now();
    let (bright, level) = {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let s = state.as_ref()?;
        let level = s.level(now);
        if s.ending.is_some() && level <= 0.0 {
            *state = None;
            return None;
        }
        (s.bright.clone(), level)
    };
    let mut drawn = DRAWN.lock().unwrap_or_else(|e| e.into_inner());
    let (id, commit, last) =
        drawn.get_or_insert_with(HashMap::new).entry(output.to_owned()).or_insert_with(|| (Id::new(), CommitCounter::default(), -1.0));
    if *last != level {
        commit.increment();
        *last = level;
    }
    // Premultiplied: black at `level`.
    let element = SolidColorRenderElement::new(id.clone(), area, *commit, [0.0, 0.0, 0.0, level], Kind::Unspecified);
    Some((bright, element))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_then_fades_in_and_out() {
        let t0 = Instant::now();
        let mut s = State { bright: HashSet::new(), since: t0, ending: None };
        assert_eq!(s.level(t0 + WAIT), 0.0);
        assert!(s.level(t0 + WAIT + FADE / 2) > 0.0 && s.level(t0 + WAIT + FADE / 2) < DIM);
        assert_eq!(s.level(t0 + WAIT + FADE), DIM);
        let end = t0 + Duration::from_secs(1);
        s.ending = Some((end, s.level(end)));
        assert_eq!(s.level(end), DIM);
        assert_eq!(s.level(end + FADE), 0.0);
    }
}

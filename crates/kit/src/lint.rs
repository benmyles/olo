//! The layout lint (`cargo ui-lint`; HACKING, "The layout lint"): a
//! window's view drawn headlessly as it will be shown, in a window with
//! no display (`olo_interface`'s headless window: Olo's text system and
//! fonts, nothing rasterised), at its window's size, in light and dark
//! at 1×, 1.5× and 2×, and checked: the layout review's errors (what
//! `tsession review` finds in a live app), a control outside the window,
//! over another or laid out to nothing, and text over text, over a
//! control or past the window's edge. An app's tests run it on every
//! fixed-size window and dialog with the longest things real machines
//! show (Preferences' panels with made-up long device names).
//!
//! Built with the `lint` feature, for tests only. The program the
//! windows belong to is started once in the test binary, on a thread of
//! its own, with no compositor (`OLO_HEADLESS`); the look is the
//! program's, so one [`Lint`] works at a time (each waits for the one
//! before).

use std::{
    sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc},
    time::{Duration, Instant},
};

use olo_interface::{Cx, Pixels, Point, View, WindowCx, WindowHandle, point, px};
use olo_kit_base::{commands::UiNode, metrics as m};
use olo_theme::Palette;

use crate::WindowOptions;

/// The room [`crate::screen_room`] says there is, as if the shortest
/// screen left `room` points (None: the screens'); for tests of windows
/// that fit what they show (there are no screens headless).
pub fn set_room_for_tests(room: Option<f32>) {
    ROOM.store(room.map_or(0, f32::to_bits), std::sync::atomic::Ordering::SeqCst);
}

static ROOM: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub(crate) fn room_for_tests() -> Option<f32> {
    let bits = ROOM.load(std::sync::atomic::Ordering::SeqCst);
    (bits != 0).then(|| f32::from_bits(bits))
}

/// The room [`crate::screen_room_across`] says there is, as if the
/// narrowest screen left `room` points across (None: the screens').
pub fn set_room_across_for_tests(room: Option<f32>) {
    ROOM_ACROSS.store(room.map_or(0, f32::to_bits), std::sync::atomic::Ordering::SeqCst);
}

static ROOM_ACROSS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub(crate) fn room_across_for_tests() -> Option<f32> {
    let bits = ROOM_ACROSS.load(std::sync::atomic::Ordering::SeqCst);
    (bits != 0).then(|| f32::from_bits(bits))
}

thread_local! {
    /// The scroll of each fitted window's Frame on this looper, by its
    /// window (`frame_scroll`).
    static FRAME_SCROLLS: std::cell::RefCell<std::collections::HashMap<olo_interface::WindowId, olo_interface::ScrollHandle>> = Default::default();
}

/// Notes the scroll of window `id`'s Frame, as it lays its content out
/// in a scroll view (`frame_scroll`).
pub(crate) fn note_frame_scroll(id: olo_interface::WindowId, scroll: &olo_interface::ScrollHandle) {
    FRAME_SCROLLS.with_borrow_mut(|m| {
        m.entry(id).or_insert_with(|| scroll.clone());
    });
}

/// Where a fitted window's Frame (one that scrolls what it shows past
/// the screen's room) is scrolled to, and the furthest it scrolls: for a
/// test, on the window's looper (scrolling doesn't draw its content
/// again, so the content can't tell).
pub fn frame_scroll(id: olo_interface::WindowId) -> Option<(Point<Pixels>, olo_interface::Size<Pixels>)> {
    FRAME_SCROLLS.with_borrow(|m| m.get(&id).map(|s| (s.offset(), s.max_offset())))
}

/// The scales every window is checked at.
pub const SCALES: &[f32] = &[1.0, 1.5, 2.0];

/// How long a window is waited for (to take a scale, to follow a click).
const WAIT: Duration = Duration::from_secs(5);

/// The looks every window is checked in: the default theme, light and
/// dark.
pub fn looks() -> Vec<(&'static str, Arc<Palette>)> {
    let look = |mode| {
        let a = olo_protocol::Appearance { effective_theme: mode, ..Default::default() };
        Arc::new(a.palette())
    };
    vec![("light", look(olo_theme::ThemeMode::Light)), ("dark", look(olo_theme::ThemeMode::Dark))]
}

/// One thing found, said with where (which look and scale) and what.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub look: &'static str,
    pub scale: f32,
    pub kind: String,
    pub text: String,
}

impl std::fmt::Display for Found {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}×: {}: {}", self.look, self.scale, self.kind, self.text)
    }
}

/// The program the lint's windows belong to: started once, headless.
fn program() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        // SAFETY: said before the display thread (which reads it) starts;
        // tests that lint are the only ones that start it.
        unsafe { std::env::set_var("OLO_HEADLESS", "1") };
        // In the language `$OLO_LANGUAGE` names (English unless): German's
        // longer words checked against every window, as the person sees
        // them. (Not started here, a lint run "in German" checked English.)
        olo_kit_base::words::start("");
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("lint-app".into())
            .spawn(move || {
                olo_interface::run("olo.lint", |_| {
                    olo_interface::stay_resident();
                    let _ = tx.send(());
                })
            })
            .expect("a thread");
        let _ = rx.recv_timeout(Duration::from_secs(20));
    });
}

static ONE: Mutex<()> = Mutex::new(());

/// A headless program for linting windows in.
pub struct Lint {
    /// Room the next window keeps below what the lint draws of it, for a
    /// state it doesn't draw (`Lint::keeps_room`).
    room: f32,
    _one: MutexGuard<'static, ()>,
}

impl Default for Lint {
    fn default() -> Self {
        Self::new()
    }
}

/// Has `f` run on the window's looper, and waits for what it gives.
fn on<V: View, R: Send + 'static>(window: &WindowHandle<V>, f: impl FnOnce(&mut WindowCx) -> R + Send + 'static) -> Option<R> {
    let (tx, rx) = mpsc::channel();
    window.with(move |cx| {
        let _ = tx.send(f(cx));
    });
    rx.recv_timeout(WAIT).ok()
}

/// Waits until `done` says so of the window (asked on its looper, a few
/// milliseconds apart); whether it did in time.
fn until<V: View>(window: &WindowHandle<V>, done: impl Fn(&mut WindowCx) -> bool + Send + Sync + Clone + 'static) -> bool {
    let began = Instant::now();
    while began.elapsed() < WAIT {
        let done = done.clone();
        if on(window, move |cx| done(cx)) == Some(true) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    false
}

/// The window drawn at the size it has come to rest at: one that sizes
/// itself to what it shows asks for that size and is given it, a turn
/// or two later.
fn settle<V: View>(window: &WindowHandle<V>) {
    // Laid out at a size that stays, and nothing it asked for still to
    // come: a window that fits what it shows asks for a size once its
    // content says how tall it is (on the looper, before the next `on`),
    // and has it when the display thread answers, which takes as long as
    // the machine takes; meanwhile its frames are held. By what's so, not
    // by a quiet spell: two reads alike 4 ms apart were once taken for
    // settled on a busy machine, and DiskBackup's window was checked at
    // its old height; so was one whose resize took longer than the hold
    // (`RESIZE_WAIT`). (`SETTLE_MOST`: only for one that never settles.)
    let mut last = None;
    let since = Instant::now();
    while since.elapsed() < SETTLE_MOST {
        let now = on(window, |cx| {
            cx.flush_now();
            (cx.viewport_size(), cx.commit_held() || cx.resize_pending())
        });
        let Some((size, waiting)) = now else { return };
        if !waiting && last == Some(size) {
            return;
        }
        last = (!waiting).then_some(size);
        std::thread::sleep(SETTLE_POLL);
    }
}

/// How often [`settle`] looks, and for how long at most.
const SETTLE_POLL: Duration = Duration::from_millis(4);
const SETTLE_MOST: Duration = Duration::from_secs(20);

/// The window's tabs, as agents find them: each one's name, its middle,
/// and whether it's the one showing.
fn tabs<V: View>(window: &WindowHandle<V>) -> Vec<(String, Point<Pixels>, bool)> {
    nodes(window).into_iter().filter(|n| n.role == "Tab").map(|n| (n.name.unwrap_or_default(), point(px(n.x + n.width / 2.0), px(n.y + n.height / 2.0)), n.selected == Some(true))).collect()
}

fn nodes<V: View>(window: &WindowHandle<V>) -> Vec<UiNode> {
    on(window, |cx| crate::agent::tree(cx).nodes).unwrap_or_default()
}

/// Clicks the tab named `name` at `at`, and waits for it to show.
fn click_tab<V: View>(window: &WindowHandle<V>, name: &str, at: Point<Pixels>) {
    on(window, move |cx| cx.headless_click(at));
    let name = name.to_owned();
    until(window, move |cx| {
        cx.flush_now();
        crate::agent::tree(cx).nodes.iter().any(|n| n.role == "Tab" && n.selected == Some(true) && n.name.as_deref() == Some(name.as_str()))
    });
    settle(window);
}

/// The window drawn once more, recorded and checked: the review's
/// errors, how far what it shows runs out of its boxes, the empty space
/// at its bottom, and how far down it's laid out (each control's and
/// label's bottom, or, in a scroll view or another clip of its own, the
/// clip's).
fn check<V: View>(window: &WindowHandle<V>, dark: bool) -> (Vec<(String, String)>, f32, f32, f32) {
    on(window, move |cx| {
        let (review, (over, slack)) = crate::review::review_window(cx, dark);
        let size = cx.viewport_size();
        let lowest = cx
            .agent_nodes()
            .iter()
            .filter_map(|n| {
                let bottom = f32::from(n.bounds.bottom());
                let clip = n.clip?;
                // (The window's own clip is the window: overflowing it
                // counts.)
                let whole = clip.origin.y <= px(0.5) && clip.size.height >= size.height - px(0.5);
                Some(if whole { bottom } else { bottom.min(f32::from(clip.bottom())) })
            })
            .fold(0.0, f32::max);
        let errors = review
            .findings
            .into_iter()
            .filter(|f| f.level == olo_kit_base::review::Level::Error)
            .map(|f| (f.kind, format!("{} at ({:.1}, {:.1}) {:.1}×{:.1}", f.text, f.x, f.y, f.width, f.height)))
            .collect();
        (errors, over, slack, lowest)
    })
    .unwrap_or_default()
}

impl Lint {
    pub fn new() -> Self {
        program();
        Lint { room: 0.0, _one: ONE.lock().unwrap_or_else(|e| e.into_inner()) }
    }

    /// The next window keeps `points` more room below what it shows than
    /// the window's inset, for a state the lint doesn't draw (`why`: a
    /// QR code to sign in with, a finger being enrolled), so the fit check
    /// allows it.
    pub fn keeps_room(&mut self, points: f32, why: &str) -> &mut Self {
        let _ = why;
        self.room = points;
        self
    }

    /// Window `options` showing the view `build` makes, drawn in every
    /// look at every scale: what's wrong with it, if anything.
    pub fn window<V: View>(&mut self, options: &WindowOptions, build: impl Fn(&mut Cx<V>) -> V + Clone + Send + 'static) -> Vec<Found> {
        let mut found = Vec::new();
        for (look, palette) in looks() {
            crate::theme::set_palette(palette.clone());
            let dark = palette.mode != olo_theme::ThemeMode::Light;
            let window = crate::open_window(options.clone(), build.clone());
            for &scale in SCALES {
                on(&window, move |cx| cx.set_headless_scale(scale));
                until(&window, move |cx| cx.scale_factor() == scale);
                settle(&window);
                // (As tall as the window is with what it shows now: one
                // that sizes itself to what it shows has.)
                let height = |window: &WindowHandle<V>| on(window, |cx| f32::from(cx.viewport_size().height)).unwrap_or(0.0);
                let first = tabs(&window);
                let (errors, over, slack, lowest) = check(&window, dark);
                for (kind, text) in errors {
                    found.push(Found { look, scale, kind, text });
                }
                // The window fits what it shows: each tab ends the
                // window's inset above its bottom, nothing running out of
                // its box or pane (a tab's content under Defaults). Every
                // tab of its tab views: each clicked (as agents find
                // them) and checked, then the first again.
                let mut shown = vec![Shown { tab: first.iter().find(|t| t.2).map(|t| t.0.clone()), height: height(&window), over, slack, lowest }];
                for (name, at, selected) in &first {
                    if *selected {
                        continue;
                    }
                    click_tab(&window, name, *at);
                    let (errors, over, slack, lowest) = check(&window, dark);
                    for (kind, text) in errors {
                        found.push(Found { look, scale, kind, text: format!("{name} tab: {text}") });
                    }
                    shown.push(Shown { tab: Some(name.clone()), height: height(&window), over, slack, lowest });
                }
                if let Some(text) = fit(&shown, options.fullscreen, self.room) {
                    found.push(Found { look, scale, kind: "fit".into(), text });
                }
                if let Some((name, at, _)) = first.iter().find(|t| t.2) {
                    click_tab(&window, name, *at);
                }
            }
            window.close();
        }
        self.room = 0.0;
        found
    }

    /// Window `options` showing the view `build` makes, drawn once at 1×
    /// in the default look: what agents read of it (`agent::tree`), and
    /// of each of its tabs, clicked, by the tab's name (None: as it
    /// opens). For indexes made from what windows show (Preferences'
    /// settings, for the palette).
    pub fn trees<V: View>(&mut self, options: &WindowOptions, build: impl Fn(&mut Cx<V>) -> V + Send + 'static) -> Vec<(Option<String>, Vec<UiNode>)> {
        crate::theme::set_palette(looks()[0].1.clone());
        let window = crate::open_window(options.clone(), build);
        on(&window, |cx| cx.set_headless_scale(1.0));
        settle(&window);
        let first = nodes(&window);
        let all = tabs(&window);
        let mut out = vec![(all.iter().find(|t| t.2).map(|t| t.0.clone()), first)];
        for (name, at, selected) in &all {
            if *selected {
                continue;
            }
            click_tab(&window, name, *at);
            out.push((Some(name.clone()), nodes(&window)));
        }
        window.close();
        out
    }
}

/// A window as the lint saw it with one of its tabs showing (`tab`; None:
/// a window of no tabs): how tall it was then, how far what it shows ran
/// out of its box or pane, the empty space at its bottom, and how far down
/// it's laid out.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    tab: Option<String>,
    height: f32,
    over: f32,
    slack: f32,
    lowest: f32,
}

/// What's wrong with how a window fits what it shows (`shown`: each of
/// its tabs), if anything. Each tab is held against the window as tall as
/// it was with that tab showing: one that fits what it shows is as tall
/// as each tab needs (DiskBackup's), and its tallest tab's content was
/// once held against the height its last tab gave it (2026-11-19, in
/// Chinese: 156 findings that were the lint's own). `kept`: room the
/// window keeps for a state the lint doesn't draw (`Lint::keeps_room`);
/// `fullscreen`: the screen sizes it, and what it shows is centred in it.
fn fit(shown: &[Shown], fullscreen: bool, kept: f32) -> Option<String> {
    let named = |s: &Shown| s.tab.as_ref().map(|t| format!("{t} tab: ")).unwrap_or_default();
    let even = |points: f32| ((points / 2.0).ceil() * 2.0) as u32;
    let worst = shown.iter().max_by(|a, b| a.over.total_cmp(&b.over))?;
    if worst.over > 0.5 {
        return Some(format!("{}what it shows runs {:.1} pt out of its box or pane: make it {} tall", named(worst), worst.over, even(worst.height + worst.over)));
    }
    let tightest = shown.iter().min_by(|a, b| (a.height - a.lowest).total_cmp(&(b.height - b.lowest)))?;
    let room = tightest.height - tightest.lowest;
    if room < m::WINDOW_INSET - 4.0 {
        return Some(format!(
            "{}what it shows ends {room:.1} pt above its bottom (the window's inset is {}): make it {} tall",
            named(tightest),
            m::WINDOW_INSET,
            even(tightest.lowest + m::WINDOW_INSET)
        ));
    }
    // The empty space under its tallest tab: the least any tab leaves (a
    // window as tall as its tallest tab leaves more under the others).
    let fullest = shown.iter().min_by(|a, b| a.slack.total_cmp(&b.slack))?;
    if !fullscreen && fullest.slack > 2.0 * m::WINDOW_INSET + kept + 0.5 {
        return Some(format!(
            "{}its tallest page leaves {:.1} pt empty at the bottom (the inset is {}): make it {} tall",
            named(fullest),
            fullest.slack,
            m::WINDOW_INSET,
            even(fullest.height - (fullest.slack - m::WINDOW_INSET))
        ));
    }
    None
}

/// Fails with everything found, one a line.
pub fn assert_clean(what: &str, found: &[Found]) {
    assert!(found.is_empty(), "{what}: {} found\n{}", found.len(), found.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(tab: &str, height: f32, lowest: f32, slack: f32) -> Shown {
        Shown { tab: Some(tab.into()), height, over: 0.0, slack, lowest }
    }

    #[test]
    fn each_tab_against_its_own_height() {
        // DiskBackup in Chinese, not running, at 1×: its window fits each
        // tab, so each is held against its own height (the tallest
        // tab's content, 564, against the last tab's 571 was the lint's
        // own finding).
        let disk_backup = [
            shown("Schedule", 597.0, 553.0, 20.0),
            shown("Files", 608.0, 564.0, 20.0),
            shown("Backups", 565.0, 521.0, 20.0),
            shown("Speed & Resources", 509.0, 464.0, 20.0),
            shown("Destination", 571.0, 526.0, 20.0),
        ];
        assert_eq!(fit(&disk_backup, false, 0.0), None);
        // A window as tall as it was made, whatever the tab: its tallest
        // tab's content too low in it is found, and named.
        let fixed = [shown("General", 400.0, 300.0, 20.0), shown("Advanced", 400.0, 392.0, 20.0)];
        assert_eq!(fit(&fixed, false, 0.0).as_deref(), Some("Advanced tab: what it shows ends 8.0 pt above its bottom (the window's inset is 16): make it 408 tall"));
        // What runs out of its box, with the height its tab gave the window.
        let mut over = fixed.clone();
        over[1].lowest = 300.0;
        over[0].over = 3.0;
        assert_eq!(fit(&over, false, 0.0).as_deref(), Some("General tab: what it shows runs 3.0 pt out of its box or pane: make it 404 tall"));
        // The empty space under its tallest tab: the least any leaves.
        let roomy = [shown("General", 400.0, 300.0, 60.0), shown("Advanced", 400.0, 300.0, 40.0)];
        assert_eq!(fit(&roomy, false, 0.0).as_deref(), Some("Advanced tab: its tallest page leaves 40.0 pt empty at the bottom (the inset is 16): make it 376 tall"));
        assert_eq!(fit(&roomy, true, 0.0), None);
        assert_eq!(fit(&roomy, false, 8.0), None);
        // A window of no tabs says no tab.
        let plain = [Shown { tab: None, height: 200.0, over: 0.0, slack: 20.0, lowest: 195.0 }];
        assert_eq!(fit(&plain, false, 0.0).as_deref(), Some("what it shows ends 5.0 pt above its bottom (the window's inset is 16): make it 212 tall"));
    }
}

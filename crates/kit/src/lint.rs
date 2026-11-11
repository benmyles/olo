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
    let mut last = None;
    for _ in 0..40 {
        let now = on(window, |cx| {
            cx.flush_now();
            cx.viewport_size()
        });
        if now.is_some() && now == last {
            return;
        }
        last = now;
        std::thread::sleep(Duration::from_millis(4));
    }
}

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
                let (errors, mut over, mut slack, mut lowest) = check(&window, dark);
                for (kind, text) in errors {
                    found.push(Found { look, scale, kind, text });
                }
                // The window fits what it shows: its tallest tab ends the
                // window's inset above its bottom, nothing running out of
                // its box or pane (a tab's content under Defaults). Every
                // tab of its tab views: each clicked (as agents find
                // them) and checked, then the first again.
                let first = tabs(&window);
                for (name, at, selected) in &first {
                    if *selected {
                        continue;
                    }
                    click_tab(&window, name, *at);
                    let (errors, o, s, l) = check(&window, dark);
                    for (kind, text) in errors {
                        found.push(Found { look, scale, kind, text: format!("{name} tab: {text}") });
                    }
                    (over, slack, lowest) = (over.max(o), slack.min(s), lowest.max(l));
                }
                // (As tall as the window is now: one that sizes itself to
                // what it shows has.)
                let height = on(&window, |cx| f32::from(cx.viewport_size().height)).unwrap_or(0.0);
                let room = height - lowest;
                if over > 0.5 {
                    let fits = (((height + over) / 2.0).ceil() * 2.0) as u32;
                    found.push(Found { look, scale, kind: "fit".into(), text: format!("what it shows runs {over:.1} pt out of its box or pane: make it {fits} tall") });
                } else if room < m::WINDOW_INSET - 4.0 {
                    let fits = (((lowest + m::WINDOW_INSET) / 2.0).ceil() * 2.0) as u32;
                    found.push(Found {
                        look,
                        scale,
                        kind: "fit".into(),
                        text: format!("what it shows ends {room:.1} pt above its bottom (the window's inset is {}): make it {fits} tall", m::WINDOW_INSET),
                    });
                } else if !options.fullscreen && slack > 2.0 * m::WINDOW_INSET + self.room + 0.5 {
                    // (Its tallest tab leaves this much empty at the
                    // bottom of its pane, or of the window. Not a full
                    // screen one's: the screen sizes it, and what it shows
                    // is centred in it.)
                    let fits = (((height - (slack - m::WINDOW_INSET)) / 2.0).ceil() * 2.0) as u32;
                    found.push(Found {
                        look,
                        scale,
                        kind: "fit".into(),
                        text: format!("its tallest page leaves {slack:.1} pt empty at the bottom (the inset is {}): make it {fits} tall", m::WINDOW_INSET),
                    });
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

/// Fails with everything found, one a line.
pub fn assert_clean(what: &str, found: &[Found]) {
    assert!(found.is_empty(), "{what}: {} found\n{}", found.len(), found.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n"));
}

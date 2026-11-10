//! The layout lint (`cargo ui-lint`; HACKING, "The layout lint"): a
//! window's view drawn headlessly as it will be shown, in GPUI's test
//! platform with Olo's text system and fonts, at its window's size, in
//! light and dark at 1×, 1.5× and 2×, and checked: the
//! layout review's errors (what `tsession review` finds in a live app),
//! a control outside the window, over another or laid out to nothing,
//! and text over text, over a control or past the window's edge. An app's tests run it on every fixed-size
//! window and dialog with the longest things real machines show
//! (Preferences' panels with made-up long device names).
//!
//! Built with the `lint` feature (GPUI's test support), for tests only.

use std::{sync::Arc, sync::mpsc};

use gpui::{App, Entity, Render, TestAppContext, VisualTestContext, Window};
use olo_theme::Palette;

use crate::{WindowOptions, metrics as m, theme::Theme};

/// The scales every window is checked at.
pub const SCALES: &[f32] = &[1.0, 1.5, 2.0];

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

/// A headless app for linting windows in (one per test: GPUI's test
/// app is the thread's).
pub struct Lint {
    cx: TestAppContext,
    /// Room the next window keeps below what the lint draws of it, for a
    /// state it doesn't draw (`Lint::keeps_room`).
    room: f32,
}

impl Default for Lint {
    fn default() -> Self {
        Self::new()
    }
}

impl Lint {
    pub fn new() -> Self {
        let cx = TestAppContext::with_olo_text();
        cx.update(|cx| {
            cx.set_global(Theme { palette: looks()[0].1.clone() });
            crate::app::init_for_lint(cx);
        });
        Lint { cx, room: 0.0 }
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
    pub fn window<V: Render + 'static>(&mut self, options: &WindowOptions, build: impl Fn(&mut Window, &mut App) -> Entity<V> + Clone + 'static) -> Vec<Found> {
        let mut found = Vec::new();
        for (look, palette) in looks() {
            self.cx.update(|cx| cx.set_global(Theme { palette: palette.clone() }));
            let build = build.clone();
            let opened = options.clone();
            let handle = self.cx.update(move |cx| crate::open_window(cx, opened, build)).expect("the window opens");
            let mut vcx = VisualTestContext::from_window(handle.into(), &self.cx);
            for &scale in SCALES {
                vcx.set_scale_factor(scale);
                settle(&mut vcx);
                for (kind, text) in check(&mut vcx, handle.into()) {
                    found.push(Found { look, scale, kind, text });
                }
                // The window fits what it shows: its tallest tab ends the
                // window's inset above its bottom, nothing running out of
                // its box or pane (a tab's content under Defaults).
                let mut lowest = lowest(&mut vcx, handle.into());
                let (mut over, mut slack) = crate::review::take_overflow(handle.window_id());
                // Every tab of its tab views too: each clicked (as agents
                // find them) and checked, then the first again.
                let tabs = |vcx: &mut VisualTestContext| -> Vec<(String, gpui::Point<gpui::Pixels>, bool)> {
                    let tree = vcx.cx.update(|cx| crate::agent::tree(handle.into(), cx));
                    tree.map(|t| t.nodes)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|n| n.role == "Tab")
                        .map(|n| (n.name.unwrap_or_default(), gpui::point(gpui::px(n.x + n.width / 2.0), gpui::px(n.y + n.height / 2.0)), n.selected == Some(true)))
                        .collect()
                };
                let first = tabs(&mut vcx);
                for (name, at, selected) in &first {
                    if *selected {
                        continue;
                    }
                    vcx.simulate_click(*at, gpui::Modifiers::default());
                    vcx.run_until_parked();
                    settle(&mut vcx);
                    for (kind, text) in check(&mut vcx, handle.into()) {
                        found.push(Found { look, scale, kind, text: format!("{name} tab: {text}") });
                    }
                    lowest = lowest.max(self::lowest(&mut vcx, handle.into()));
                    let (o, s) = crate::review::take_overflow(handle.window_id());
                    (over, slack) = (over.max(o), slack.min(s));
                }
                // (As tall as the window is now: one that sizes itself to
                // what it shows has.)
                let height = vcx.update(|window, _| f32::from(window.viewport_size().height));
                let room = height - lowest;
                if over > 0.5 {
                    let fits = (((height + over) / 2.0).ceil() * 2.0) as u32;
                    let text = format!("what it shows runs {over:.1} pt out of its box or pane: make it {fits} tall");
                    found.push(Found { look, scale, kind: "fit".into(), text });
                } else if room < m::WINDOW_INSET - 4.0 {
                    let fits = (((lowest + m::WINDOW_INSET) / 2.0).ceil() * 2.0) as u32;
                    let text = format!("what it shows ends {room:.1} pt above its bottom (the window's inset is {}): make it {fits} tall", m::WINDOW_INSET);
                    found.push(Found { look, scale, kind: "fit".into(), text });
                } else if !options.fullscreen && slack > 2.0 * m::WINDOW_INSET + self.room + 0.5 {
                    // (Its tallest tab leaves this much empty at the
                    // bottom of its pane, or of the window. Not a full
                    // screen one's: the screen sizes it, and what it shows
                    // is centred in it.)
                    let fits = (((height - (slack - m::WINDOW_INSET)) / 2.0).ceil() * 2.0) as u32;
                    let text = format!("its tallest page leaves {slack:.1} pt empty at the bottom (the inset is {}): make it {fits} tall", m::WINDOW_INSET);
                    found.push(Found { look, scale, kind: "fit".into(), text });
                }
                if let Some((_, at, _)) = first.iter().find(|t| t.2) {
                    vcx.simulate_click(*at, gpui::Modifiers::default());
                    vcx.run_until_parked();
                }
            }
            vcx.update(|window, _| window.remove_window());
            vcx.run_until_parked();
        }
        self.room = 0.0;
        found
    }
}

impl Lint {
    /// Window `options` showing the view `build` makes, drawn once at 1×
    /// in the default look: what agents read of it (`agent::tree`), and
    /// of each of its tabs, clicked, by the tab's name (None: as it
    /// opens). For indexes made from what windows show (Preferences'
    /// settings, for the palette).
    pub fn trees<V: Render + 'static>(&mut self, options: &WindowOptions, build: impl Fn(&mut Window, &mut App) -> Entity<V> + 'static) -> Vec<(Option<String>, Vec<olo_kit_base::commands::UiNode>)> {
        self.cx.update(|cx| cx.set_global(Theme { palette: looks()[0].1.clone() }));
        let options = options.clone();
        let handle = self.cx.update(move |cx| crate::open_window(cx, options, build)).expect("the window opens");
        let mut vcx = VisualTestContext::from_window(handle.into(), &self.cx);
        vcx.set_scale_factor(1.0);
        vcx.run_until_parked();
        let tree = |vcx: &mut VisualTestContext| vcx.cx.update(|cx| crate::agent::tree(handle.into(), cx)).map(|t| t.nodes).unwrap_or_default();
        let first = tree(&mut vcx);
        let tabs: Vec<(String, gpui::Point<gpui::Pixels>, bool)> = first
            .iter()
            .filter(|n| n.role == "Tab")
            .map(|n| (n.name.clone().unwrap_or_default(), gpui::point(gpui::px(n.x + n.width / 2.0), gpui::px(n.y + n.height / 2.0)), n.selected == Some(true)))
            .collect();
        let mut out = vec![(tabs.iter().find(|t| t.2).map(|t| t.0.clone()), first)];
        for (name, at, selected) in &tabs {
            if *selected {
                continue;
            }
            vcx.simulate_click(*at, gpui::Modifiers::default());
            vcx.run_until_parked();
            out.push((Some(name.clone()), tree(&mut vcx)));
        }
        vcx.update(|window, _| window.remove_window());
        vcx.run_until_parked();
        out
    }
}

/// A window that resized itself (to what it shows: `Window::resize`) is
/// given that size, as a compositor gives it, and drawn again, until it
/// rests.
fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..4 {
        let (asked, now) = vcx.update(|window, _| (window.bounds().size, window.viewport_size()));
        if (asked.width - now.width).abs() < gpui::px(0.5) && (asked.height - now.height).abs() < gpui::px(0.5) {
            return;
        }
        vcx.simulate_resize(asked);
        vcx.run_until_parked();
    }
}

/// How far down the window what it shows is laid out (overflowing it,
/// too): each control's and label's bottom, or, in a scroll view or
/// another clip of its own, the clip's.
fn lowest(vcx: &mut VisualTestContext, handle: gpui::AnyWindowHandle) -> f32 {
    let _ = handle;
    vcx.update(|window, _| {
        let size = window.viewport_size();
        window
            .paint_records()
            .iter()
            .filter_map(|(r, moved)| {
                let n = r.downcast_ref::<crate::agent::Node>()?;
                let bottom = f32::from(n.bounds.origin.y + moved.y + n.bounds.size.height);
                let clip = n.clip?;
                // (The window's own clip is the window: overflowing it
                // counts.)
                let whole = clip.origin.y <= gpui::px(0.5) && clip.size.height >= size.height - gpui::px(0.5);
                Some(if whole { bottom } else { bottom.min(f32::from(clip.bottom())) })
            })
            .fold(0.0, f32::max)
    })
}

/// The window drawn once more, recorded and checked.
fn check(vcx: &mut VisualTestContext, handle: gpui::AnyWindowHandle) -> Vec<(String, String)> {
    // (From the app, not inside the window's own update: GPUI takes a
    // window out while it's updated, and the review draws it.)
    let (tx, rx) = mpsc::channel();
    vcx.cx.update(|cx| crate::review::start(cx, tx));
    vcx.run_until_parked();
    let reviews = rx.try_recv().unwrap_or_default();
    // (The review's checks are the lint's: a control or text cut off or
    // past the window, over another, crowded or almost lined up, and the
    // pixel errors.)
    let _ = handle;
    reviews
        .into_iter()
        .flat_map(|r| r.findings)
        .filter(|f| f.level == olo_kit_base::review::Level::Error)
        .map(|f| (f.kind, format!("{} at ({:.1}, {:.1}) {:.1}×{:.1}", f.text, f.x, f.y, f.width, f.height)))
        .collect()
}

/// Fails with everything found, one a line.
pub fn assert_clean(what: &str, found: &[Found]) {
    assert!(found.is_empty(), "{what}: {} found\n{}", found.len(), found.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n"));
}

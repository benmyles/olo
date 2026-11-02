//! The layout lint (`cargo ui-lint`; HACKING, "The layout lint"): a
//! window's view drawn headlessly as it will be shown, in GPUI's test
//! platform with Olo's text system and fonts, at its window's size, in
//! light and dark at 1×, 1.5× and 2×, and checked: the
//! layout review's errors (what `tsession review` finds in a live app),
//! a control outside the window, over another or laid out to nothing,
//! and text over text or past the window's edge. An app's tests run it on every fixed-size
//! window and dialog with the longest things real machines show
//! (Preferences' panels with made-up long device names).
//!
//! Built with the `lint` feature (GPUI's test support), for tests only.

use std::{sync::Arc, sync::mpsc};

use gpui::{App, Entity, Render, TestAppContext, VisualTestContext, Window};
use olo_kit_base::layout_review::{self as checks, Parts};
use olo_theme::Palette;

use crate::{WindowOptions, theme::Theme};

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
        Lint { cx }
    }

    /// Window `options` showing the view `build` makes, drawn in every
    /// look at every scale: what's wrong with it, if anything.
    pub fn window<V: Render + 'static>(&mut self, options: &WindowOptions, build: impl Fn(&mut Window, &mut App) -> Entity<V> + Clone + 'static) -> Vec<Found> {
        let mut found = Vec::new();
        for (look, palette) in looks() {
            self.cx.update(|cx| cx.set_global(Theme { palette: palette.clone() }));
            let build = build.clone();
            let options = options.clone();
            let handle = self.cx.update(move |cx| crate::open_window(cx, options, build)).expect("the window opens");
            let mut vcx = VisualTestContext::from_window(handle.into(), &self.cx);
            for &scale in SCALES {
                vcx.set_scale_factor(scale);
                for (kind, text) in check(&mut vcx, handle.into()) {
                    found.push(Found { look, scale, kind, text });
                }
                // Every tab of its tab views too: each clicked (as agents
                // find them) and checked, then the first again.
                let tabs = |vcx: &mut VisualTestContext| -> Vec<(String, gpui::Point<gpui::Pixels>, bool)> {
                    let tree = vcx.cx.update(|cx| crate::agent::tree(handle.into(), cx));
                    tree.map(|t| t.nodes).unwrap_or_default().into_iter().filter(|n| n.role == "Tab").map(|n| (n.name.unwrap_or_default(), gpui::point(gpui::px(n.x + n.width / 2.0), gpui::px(n.y + n.height / 2.0)), n.selected == Some(true))).collect()
                };
                let first = tabs(&mut vcx);
                for (name, at, selected) in &first {
                    if *selected {
                        continue;
                    }
                    vcx.simulate_click(*at, gpui::Modifiers::default());
                    vcx.run_until_parked();
                    for (kind, text) in check(&mut vcx, handle.into()) {
                        found.push(Found { look, scale, kind, text: format!("{name} tab: {text}") });
                    }
                }
                if let Some((_, at, _)) = first.iter().find(|t| t.2) {
                    vcx.simulate_click(*at, gpui::Modifiers::default());
                    vcx.run_until_parked();
                }
            }
            vcx.update(|window, _| window.remove_window());
            vcx.run_until_parked();
        }
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
        let tabs: Vec<(String, gpui::Point<gpui::Pixels>, bool)> = first.iter().filter(|n| n.role == "Tab").map(|n| (n.name.clone().unwrap_or_default(), gpui::point(gpui::px(n.x + n.width / 2.0), gpui::px(n.y + n.height / 2.0)), n.selected == Some(true))).collect();
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

/// The window drawn once more, recorded and checked.
fn check(vcx: &mut VisualTestContext, handle: gpui::AnyWindowHandle) -> Vec<(String, String)> {
    // (From the app, not inside the window's own update: GPUI takes a
    // window out while it's updated, and the review draws it.)
    let (tx, rx) = mpsc::channel();
    vcx.cx.update(|cx| crate::review::start(cx, tx));
    vcx.run_until_parked();
    let reviews = rx.try_recv().unwrap_or_default();
    let parts = crate::review::take_lint_parts(handle.window_id()).unwrap_or_default();
    let size = vcx.update(|window, _| window.viewport_size());
    let tree = vcx.cx.update(|cx| crate::agent::tree(handle, cx));
    let (w, h) = (f32::from(size.width), f32::from(size.height));
    let mut out: Vec<(String, String)> = reviews
        .into_iter()
        .flat_map(|r| r.findings)
        .filter(|f| f.level == olo_kit_base::review::Level::Error)
        .map(|f| (f.kind, format!("{} at ({:.1}, {:.1}) {:.1}×{:.1}", f.text, f.x, f.y, f.width, f.height)))
        .collect();
    let nodes = tree.map(|t| t.nodes).unwrap_or_default();
    // (What shows: one scrolled out of its view isn't anywhere.)
    let controls: Vec<_> = nodes.iter().filter(|n| n.clickable && !n.offscreen && n.role != "Window" && n.width > 0.0 && n.height > 0.0).collect();
    let named = |n: &olo_kit_base::commands::UiNode| format!("{} {:?}", n.role, n.name.clone().unwrap_or_default());
    // A control laid out to nothing (a width it couldn't have).
    for n in nodes.iter().filter(|n| n.clickable && !n.offscreen && n.role != "Window" && (n.width < 1.0 || n.height < 1.0)) {
        out.push(("collapsed".into(), format!("{} at ({:.1}, {:.1}) is {:.1}×{:.1}", named(n), n.x, n.y, n.width, n.height)));
    }
    const SLACK: f32 = 0.5;
    for n in &controls {
        if n.x < -SLACK || n.y < -SLACK || n.x + n.width > w + SLACK || n.y + n.height > h + SLACK {
            out.push(("outside".into(), format!("{} at ({:.1}, {:.1}) {:.1}×{:.1} runs past the window ({w}×{h})", named(n), n.x, n.y, n.width, n.height)));
        }
    }
    let rect = |n: &olo_kit_base::commands::UiNode| checks::Rect::from_min_size(checks::pos2(n.x, n.y), n.width, n.height);
    for (i, a) in controls.iter().enumerate() {
        for b in &controls[i + 1..] {
            let (ra, rb) = (rect(a), rect(b));
            let over = ra.intersect(rb);
            // (One inside the other is a control in a row, a cell in a
            // list: not over it.)
            if over.width() > 1.0 && over.height() > 1.0 && !ra.contains_rect(rb) && !rb.contains_rect(ra) {
                out.push(("overlap".into(), format!("{} and {} overlap by {:.1}×{:.1}", named(a), named(b), over.width(), over.height())));
            }
        }
    }
    texts(&parts, w, h, &mut out);
    out
}

/// Text over text, or past the window's edge.
fn texts(parts: &Parts, w: f32, h: f32, out: &mut Vec<(String, String)>) {
    for (i, a) in parts.texts.iter().enumerate() {
        if a.rect.max.x > w + 0.5 || a.rect.max.y > h + 0.5 {
            out.push(("text-outside".into(), format!("{:?} ends at ({:.1}, {:.1}), past the window ({w}×{h})", a.label, a.rect.max.x, a.rect.max.y)));
        }
        for b in &parts.texts[i + 1..] {
            let over = a.rect.intersect(b.rect);
            // (Lines' boxes touch by their leading, and keys' symbols set
            // closer than their advance by a point or so; their ink
            // doesn't.)
            if over.width() > 2.0 && over.height() > a.size.min(b.size) * 0.4 {
                out.push(("text-over-text".into(), format!("{:?} and {:?} overlap by {:.1}×{:.1}", a.label, b.label, over.width(), over.height())));
            }
        }
    }
}

/// Fails with everything found, one a line.
pub fn assert_clean(what: &str, found: &[Found]) {
    assert!(found.is_empty(), "{what}: {} found\n{}", found.len(), found.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n"));
}

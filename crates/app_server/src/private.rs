//! Windows kept out of what's shared: a screen share or recording
//! (`capture_elements` for a capture session, a window share of one)
//! shows them blacked out, where the person still sees them. Passwords'
//! windows and the polkit prompt by their app ids, and any window its
//! app tags `private` (the kit's `WindowOptions::private`: Terminal's
//! sudo prompt, Deskbar's approve-without-asking). While the screen is
//! locked, a share shows black altogether, never the lock screen.
//! (macOS keeps a window out of captures likewise, `NSWindowSharingNone`.)
//! The person's own screenshots show them, as do agents' looks at their
//! own screens.
//!
//! A window being shared (olo-portal says which: `Request::SharedWindows`)
//! has a ring round it on screen while it is (`shared_rings`), drawn
//! with the frame, never in what's shared.

use std::{cell::RefCell, collections::HashMap};

use smithay::desktop::WindowSurface;
use smithay::{
    backend::renderer::{
        element::{Id, Kind, solid::SolidColorRenderElement},
        utils::CommitCounter,
    },
    desktop::Space,
    output::Output,
    utils::{Physical, Rectangle},
    wayland::{compositor::with_states, xdg_toplevel_tag::XdgToplevelTagSurfaceData},
};

use crate::shell::WindowElement;

/// Apps whose every window shows secrets.
const PRIVATE_APPS: &[&str] = &["olo.passwords", "olo.polkit-agent"];

/// What blacked-out windows show: a dark grey, as an empty picture.
const SHADE: [f32; 4] = [0.08, 0.08, 0.09, 1.0];

/// Window `w` is kept out of shares.
pub fn is_private(w: &WindowElement) -> bool {
    if PRIVATE_APPS.contains(&w.app_id().as_str()) {
        return true;
    }
    match w.0.underlying_surface() {
        WindowSurface::Wayland(t) => {
            with_states(t.wl_surface(), |s| s.data_map.get::<XdgToplevelTagSurfaceData>().is_some_and(|d| d.tag().is_some_and(|t| t.split_whitespace().any(|t| t == "private"))))
        }
    }
}

thread_local! {
    /// Each blackout's element id (by window, or by output for a locked
    /// screen), kept so a share's damage follows only what moved.
    static IDS: RefCell<HashMap<String, Id>> = RefCell::new(HashMap::new());
}

fn id(key: String) -> Id {
    IDS.with(|ids| ids.borrow_mut().entry(key).or_insert_with(Id::new).clone())
}

fn shade(key: String, rect: Rectangle<i32, Physical>) -> SolidColorRenderElement {
    SolidColorRenderElement::new(id(key), rect, CommitCounter::default(), SHADE, Kind::Unspecified)
}

/// What covers `output`'s private windows (front first) in a capture of
/// it, or the whole of it while `locked`: in the output's physical
/// pixels, to go in front of everything it shows.
pub fn blackouts(output: &Output, space: &Space<WindowElement>, locked: bool) -> Vec<SolidColorRenderElement> {
    let Some(geo) = space.output_geometry(output) else { return Vec::new() };
    let scale = output.current_scale().fractional_scale();
    if locked {
        let size = geo.size.to_f64().to_physical(scale).to_i32_round();
        return vec![shade(format!("locked:{}", output.name()), Rectangle::from_size(size))];
    }
    space
        .elements()
        .filter(|w| is_private(w))
        .filter_map(|w| {
            let bbox = space.element_bbox(w)?;
            let on = bbox.intersection(geo)?;
            let local = Rectangle::new(on.loc - geo.loc, on.size);
            Some(shade(format!("window:{}", w.id()), local.to_f64().to_physical(scale).to_i32_round()))
        })
        .collect()
}

/// A window share of `w` shows it blacked out (its picture's size in
/// pixels), if it's private.
pub fn window_blackout(w: &WindowElement, size: smithay::utils::Size<i32, Physical>) -> Option<SolidColorRenderElement> {
    is_private(w).then(|| shade(format!("share:{}", w.id()), Rectangle::from_size(size)))
}

/// The ring's colour (a recording's red) and its width, in points.
const RING: [f32; 4] = [0.93, 0.27, 0.22, 1.0];
const RING_WIDTH: f64 = 2.0;

/// Rings round the windows `shared` that show on `output`: four bars each
/// just outside its frame, whole device pixels, in the output's physical
/// pixels.
pub fn shared_rings(output: &Output, space: &Space<WindowElement>, shared: &[u64]) -> Vec<SolidColorRenderElement> {
    if shared.is_empty() {
        return Vec::new();
    }
    let Some(geo) = space.output_geometry(output) else { return Vec::new() };
    let scale = output.current_scale().fractional_scale();
    let width = (RING_WIDTH * scale).round().max(1.0) as i32;
    let mut out = Vec::new();
    for w in space.elements().filter(|w| shared.contains(&w.id())) {
        // Its body: what it draws and its border, its tab above the ring
        // (round the tab too, it took in the empty strip beside it). In
        // device pixels as the decoration lays its frame out
        // (`deco::render_elements`: from the element's origin, each length
        // rounded on its own), so the ring sits flush outside the border
        // at every scale.
        let Some(loc) = space.element_location(w) else { continue };
        let (off, size) = (w.content_offset(), w.content_size());
        let px = |l: i32| (l as f64 * scale).round() as i32;
        let origin = (loc - geo.loc).to_f64().to_physical(scale).to_i32_round::<i32>();
        let (border, content_y) = (px(off.x), px(off.y));
        let content = ((size.w as f64 * scale).round() as i32, (size.h as f64 * scale).round() as i32);
        let top = content_y - border;
        let local: Rectangle<i32, Physical> = Rectangle::new((origin.x, origin.y + top).into(), (content.0 + 2 * border, content_y + content.1 + border - top).into());
        let logical = local.to_f64().to_logical(scale);
        if !Rectangle::from_size(geo.size.to_f64()).overlaps(logical) {
            continue;
        }
        let (x, y, w2, h) = (local.loc.x - width, local.loc.y - width, local.size.w + 2 * width, local.size.h + 2 * width);
        let bars = [(x, y, w2, width), (x, y + h - width, w2, width), (x, y + width, width, h - 2 * width), (x + w2 - width, y + width, width, h - 2 * width)];
        for (k, (bx, by, bw, bh)) in bars.into_iter().enumerate() {
            out.push(SolidColorRenderElement::new(id(format!("ring:{}:{k}", w.id())), Rectangle::new((bx, by).into(), (bw, bh).into()), CommitCounter::default(), RING, Kind::Unspecified));
        }
    }
    out
}

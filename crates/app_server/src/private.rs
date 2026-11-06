//! Windows being shared: one a share is of (olo-portal says which:
//! `Request::SharedWindows`) has a ring round it on screen while it is
//! (`shared_rings`), drawn with the frame, never in what's shared.
//! Nothing is kept out of a share or a recording (Ben, 2026-11-06:
//! "nothing should be intentionally blacked out"): Passwords' windows,
//! prompts and the lock screen show as they are, in an agent screen's
//! live view too.

use std::{cell::RefCell, collections::HashMap};

use smithay::{
    backend::renderer::{
        element::{Id, Kind, solid::SolidColorRenderElement},
        utils::CommitCounter,
    },
    desktop::Space,
    output::Output,
    utils::{Physical, Rectangle},
};

use crate::shell::WindowElement;

thread_local! {
    /// Each ring bar's element id, kept so the frame's damage follows
    /// only what moved.
    static IDS: RefCell<HashMap<String, Id>> = RefCell::new(HashMap::new());
}

fn id(key: String) -> Id {
    IDS.with(|ids| ids.borrow_mut().entry(key).or_insert_with(Id::new).clone())
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

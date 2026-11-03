//! A window tab's title whole, where the tab cuts it short ("…": more
//! than a tab shows, or wider than its room): on a tooltip's plate under
//! the tab once the pointer rests on the title, at once while ⌘ is held,
//! as Olo's apps show the text they cut (`olo_ui::cut`). One at a time;
//! gone when the pointer leaves the title, and after a press until it
//! does. Drawn with the display's frame over everything but the pointer,
//! as the level indicator is (`osd`).

use olo_sync::Mutex;
use std::time::Duration;

use olo_theme::Palette;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    output::Output,
    reexports::calloop::{
        RegistrationToken,
        timer::{TimeoutAction, Timer},
    },
    utils::{Buffer, Logical, Physical, Point, Rectangle, Size, Transform},
};
use tiny_skia::Pixmap;

use crate::{
    render::Exact,
    shell::{WindowElement, fill},
    state::{AnvilState, Backend},
    text, theme,
};

/// How long the pointer rests on a title before its tip shows (the
/// kit's tips' pause).
const DELAY: Duration = Duration::from_millis(500);
/// The plate, in points: its text's size, its height and side padding,
/// and how far under the tab it sits.
const FONT: f32 = 12.0;
const PLATE_H: f64 = 20.0;
const PAD: f64 = 6.0;
pub(crate) const GAP: f64 = 4.0;
/// Kept this far from the display's edges.
pub(crate) const MARGIN: f64 = 8.0;

/// The title the pointer is on, and the timer for its tip.
#[derive(Debug, Default)]
pub struct TabTip {
    /// The window, its title, and where the title is in the window
    /// (element-local, as the pointer's motion over it says).
    over: Option<(u64, String, Rectangle<i32, Logical>)>,
    /// Pressed since the pointer came: no tip until it leaves.
    pressed: bool,
    timer: Option<RegistrationToken>,
    /// The display it shows on.
    shown_on: Option<Output>,
}

/// The tip showing on a display: its picture and where (in the
/// display's pixels).
#[derive(Default)]
struct Slot(Mutex<Option<(MemoryRenderBuffer, Point<i32, Physical>, (i32, i32))>>);

fn slot(output: &Output) -> &Slot {
    output.user_data().insert_if_missing_threadsafe(Slot::default);
    output.user_data().get::<Slot>().unwrap()
}

/// The tip on `output`, while it shows.
pub fn element<R>(renderer: &mut R, output: &Output) -> Option<Exact<MemoryRenderBufferRenderElement<R>>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let slot = slot(output).0.lock().unwrap();
    let (buffer, at, size) = slot.as_ref()?;
    crate::render::picture(renderer, at.to_f64(), buffer, *size, None, Kind::Unspecified)
}

/// `title` on a tooltip's plate at scale `s`, at most `max_w` device
/// pixels wide (cut short with "…" past that): app_server's tips (this,
/// the overview's chosen window, Twitcher's label) all draw on it.
pub(crate) fn plate(p: &Palette, title: &str, max_w: i32, s: f64) -> Option<Pixmap> {
    let size = FONT * s as f32;
    let pad = (PAD * s).round() as i32;
    let h = (PLATE_H * s).round() as i32;
    let u = (s.floor() as i32).max(1);
    let shown = text::ellipsize(title, size, false, (max_w - 2 * pad) as f32);
    let w = text::measure(&shown, size, false).ceil() as i32 + 2 * pad;
    if shown.is_empty() {
        return None;
    }
    let mut pm = Pixmap::new(w as u32, h as u32)?;
    fill(&mut pm, 0, 0, w, h, p.control_border);
    fill(&mut pm, u, u, w - 2 * u, h - 2 * u, p.tooltip_background);
    let y = text::top_for_centre(h as f32 / 2.0, size, false).round();
    text::draw(&mut pm, &shown, pad as f32, y, size, false, p.tooltip_text);
    Some(pm)
}

impl<B: Backend + 'static> AnvilState<B> {
    /// The pointer is on `window`'s tab (titled `name`), on its title at
    /// `title` (in the window) if the tab cuts it short, else not on a
    /// cut title. Called
    /// from inside the pointer's motion: nothing here asks the pointer.
    pub(crate) fn tab_tip_hover(&mut self, window: &WindowElement, name: String, title: Option<Rectangle<i32, Logical>>) {
        let Some(rect) = title else {
            return self.tab_tip_off(true);
        };
        let id = window.id();
        if self.tab_tip.over.as_ref().is_some_and(|(i, t, _)| *i == id && *t == name) {
            return;
        }
        self.tab_tip_off(true);
        self.tab_tip.over = Some((id, name, rect));
        if self.keymap.command_held() {
            // (Once the motion is done: showing asks where the pointer is.)
            self.handle.insert_idle(|state| state.tab_tip_show());
            return;
        }
        self.tab_tip.timer = self
            .handle
            .insert_source(Timer::from_duration(DELAY), |_, _, state| {
                state.tab_tip.timer = None;
                state.tab_tip_show();
                TimeoutAction::Drop
            })
            .ok();
    }

    /// ⌘ went down: the tip of the title the pointer is on, at once.
    pub(crate) fn tab_tip_now(&mut self) {
        if self.tab_tip.over.is_some() && self.tab_tip.shown_on.is_none() {
            self.tab_tip_show();
        }
    }

    /// A press on the tab: the tip goes until the pointer leaves it.
    pub(crate) fn tab_tip_pressed(&mut self) {
        if self.tab_tip.over.is_some() {
            self.tab_tip_off(false);
            self.tab_tip.pressed = true;
        }
    }

    /// Something else came forward (the keyboard went elsewhere: a window
    /// opened, one chosen): the tip goes.
    pub(crate) fn tab_tip_hide(&mut self) {
        if self.tab_tip.over.is_some() {
            self.tab_tip_off(true);
        }
    }

    /// The tip goes (`leaving`: and the title the pointer was on).
    fn tab_tip_off(&mut self, leaving: bool) {
        if let Some(t) = self.tab_tip.timer.take() {
            self.handle.remove(t);
        }
        if let Some(o) = self.tab_tip.shown_on.take() {
            *slot(&o).0.lock().unwrap() = None;
            self.backend_data.wake(None);
        }
        if leaving {
            self.tab_tip.over = None;
            self.tab_tip.pressed = false;
        }
    }

    fn tab_tip_show(&mut self) {
        let Some((id, title, local)) = self.tab_tip.over.clone() else { return };
        if self.tab_tip.pressed || self.lock.locked() || self.tab_tip.shown_on.is_some() {
            return;
        }
        // On screen: from the pointer, less where it is in the window.
        let Some(window) = self.space.elements().find(|w| w.id() == id).cloned() else { return };
        let origin = self.pointer.current_location() - window.decoration().pointer;
        let rect = Rectangle::new((origin + local.loc.to_f64()).to_i32_round(), local.size);
        let Some(output) = self.space.output_under(rect.loc.to_f64()).next().cloned() else { return };
        let Some(geo) = self.space.output_geometry(&output) else { return };
        let s = output.current_scale().fractional_scale();
        let Some(pm) = plate(&theme::palette(), &title, ((geo.size.w as f64 - 2.0 * MARGIN) * s) as i32, s) else { return };
        let (w, h) = (pm.width() as i32, pm.height() as i32);
        // Under the title, its text where the title's starts; kept on the
        // display (above the tab if there's no room under it).
        let (pw, ph) = (w as f64 / s, h as f64 / s);
        let mut y = (rect.loc.y + rect.size.h) as f64 + GAP - geo.loc.y as f64;
        if y + ph > geo.size.h as f64 - MARGIN {
            y = (rect.loc.y - geo.loc.y) as f64 - GAP - ph;
        }
        let x = (rect.loc.x as f64 - PAD - geo.loc.x as f64).clamp(MARGIN, (geo.size.w as f64 - MARGIN - pw).max(MARGIN));
        let at = Point::<f64, Logical>::from((x, y.max(0.0))).to_physical(s).to_i32_round();
        let size: Size<i32, Buffer> = (w, h).into();
        let buffer = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, size, 1, Transform::Normal, Some(vec![Rectangle::from_size(size)]));
        *slot(&output).0.lock().unwrap() = Some((buffer, at, (w, h)));
        self.tab_tip.shown_on = Some(output);
        self.backend_data.wake(None);
    }
}

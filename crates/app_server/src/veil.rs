//! "Upgrading Olo": a veil over every display while app_server comes back
//! after a restart in place (an upgrade, `olo-session reload`), until the
//! windows are back, then faded away; and the same as Olo's start-up
//! screen while a session starts ("Starting Olo", till its desktop and
//! Deskbar show: `Why::Starting`, the session telling its first
//! app_server, `OLO_STARTING`). Displays modeset and windows return
//! one by one as their programs reconnect; the veil shows that as an
//! homage to BeOS's boot screen: Olo's wordmark on dark, and under it a
//! row of icons, dim until their part is done, lit one by one (the
//! upgrade; the displays; windows coming back; all of them), with a line
//! saying what's going on. Drawn with Olo's sunburst-made art: the
//! wordmark pre-scaled to each scale's width (`data/brand/sizes`) and the
//! icon set's sizes, each pixel for pixel (one texture pixel to a device
//! pixel), so it's sharp at every scale.
//!
//! It starts only after a restart with windows to bring back
//! (`layout::load` found places saved moments ago), and ends when as many
//! windows are back as were saved, or after `LONGEST`; it never shows over
//! the lock screen. A frame is drawn only when what it shows changes (a
//! stage reached) and through its fade.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{Id, Kind, memory::MemoryRenderBuffer, solid::SolidColorRenderElement},
            utils::CommitCounter,
        },
    },
    output::Output,
    utils::{Buffer, Rectangle, Size, Transform},
};
use tiny_skia::{Pixmap, PixmapPaint};

use crate::render::CustomRenderElements;

/// The longest the veil stays (windows that don't come back by then
/// come back under it gone).
const LONGEST: Duration = Duration::from_secs(6);
/// How long it takes to fade away.
const FADE: Duration = Duration::from_millis(400);
/// It shows at least this long before it fades, so it's seen rather
/// than flashed (windows can be back in a third of a second).
const LEAST: Duration = Duration::from_millis(900);
/// The background: Olo's dark, as the wordmark-on-dark is made for.
const BACKGROUND: [f32; 4] = [0.118, 0.118, 0.133, 1.0];
/// What it's for: an upgrade's restart, or a session starting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Upgrade,
    Starting,
}

/// The stages' icons (`data/icons`), lit in turn, and what each stage
/// says while it's the one going on.
fn stages(why: Why) -> ([&'static str; 4], [&'static str; 4]) {
    match why {
        Why::Upgrade => (["software-update", "display", "windows", "desktop"], ["Upgrading Olo", "Setting up the displays", "Bringing your windows back", "Ready"]),
        Why::Starting => (["system", "display", "desktop", "application"], ["Starting Olo", "Setting up the displays", "Opening the desktop", "Welcome"]),
    }
}
/// The picture's parts, in points: the wordmark's width, an icon's side
/// and the gap between icons, the line's size.
const WORDMARK: u32 = 256;
const ICON: u32 = 64;
const GAP: u32 = 24;
const TEXT: f32 = 15.0;

pub struct Veil {
    why: Why,
    pub since: Instant,
    /// The windows the restart is bringing back.
    expected: usize,
    /// Stages done (0 to 4): its icons lit.
    stage: usize,
    /// Fading since (once every window is back, or it's waited long).
    ending: Option<Instant>,
    /// The picture for each display, by its scale and stage.
    pictures: HashMap<String, (u64, usize, MemoryRenderBuffer, (i32, i32))>,
    ids: HashMap<String, Id>,
}

impl Veil {
    /// A veil for a restart bringing `expected` windows back.
    pub fn new(expected: usize) -> Veil {
        tracing::info!(expected, "the upgrade's veil, till the windows are back");
        Veil { why: Why::Upgrade, since: Instant::now(), expected, stage: 1, ending: None, pictures: HashMap::new(), ids: HashMap::new() }
    }

    /// Olo's start-up screen, as a session starts: till its desktop and
    /// Deskbar are up.
    pub fn starting() -> Veil {
        tracing::info!("the start-up screen, till the desktop is up");
        Veil { why: Why::Starting, since: Instant::now(), expected: 0, stage: 1, ending: None, pictures: HashMap::new(), ids: HashMap::new() }
    }

    /// The least it shows: a start's screen a little longer, to be seen.
    fn least(&self) -> Duration {
        match self.why {
            Why::Upgrade => LEAST,
            Why::Starting => LEAST * 2,
        }
    }

    /// Where it is now: the displays (`displays`: how many have a mode),
    /// the windows back (`back`), and whether the desktop and Deskbar show
    /// (`shell`). True while it still shows (false: done, drop it);
    /// `changed` when a frame should be drawn.
    pub fn update(&mut self, displays: usize, back: usize, shell: (bool, bool)) -> (bool, bool) {
        let mut stage = 1;
        if displays > 0 {
            stage = 2;
        }
        let (third, fourth) = match self.why {
            Why::Upgrade => (self.expected == 0 || back * 2 >= self.expected, back >= self.expected),
            Why::Starting => (shell.0 || shell.1, shell.1),
        };
        if stage == 2 && third {
            stage = 3;
        }
        if stage == 3 && fourth {
            stage = 4;
        }
        let changed = stage > self.stage;
        self.stage = self.stage.max(stage);
        if self.ending.is_none() && ((self.stage == 4 && self.since.elapsed() >= self.least()) || self.since.elapsed() >= LONGEST) {
            tracing::info!(why = ?self.why, stage = self.stage, back, expected = self.expected, ms = self.since.elapsed().as_millis() as u64, "the veil fades");
            self.ending = Some(Instant::now());
        }
        let fading = self.ending.is_some();
        let alive = self.ending.is_none_or(|t| t.elapsed() < FADE);
        (alive, changed || (fading && alive))
    }

    /// How opaque it is now (fading at the end).
    fn alpha(&self) -> f32 {
        match self.ending {
            Some(t) => 1.0 - (t.elapsed().as_secs_f32() / FADE.as_secs_f32()).clamp(0.0, 1.0),
            None => 1.0,
        }
    }

    /// When to look again for the longest wait (a timer wakes the frame).
    pub fn deadline(&self) -> Duration {
        LONGEST.saturating_sub(self.since.elapsed())
    }

    /// When it may fade at the soonest (a timer looks then).
    pub fn soonest(&self) -> Duration {
        self.least().saturating_sub(self.since.elapsed())
    }

    /// What `output` shows of it, in front of everything: the background
    /// and, centred, the picture.
    pub fn elements<R>(&mut self, renderer: &mut R, output: &Output) -> Vec<CustomRenderElements<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        let Some(mode) = output.current_mode() else { return Vec::new() };
        let size = output.current_transform().transform_size(mode.size);
        let scale = output.current_scale().fractional_scale();
        let key = (scale * 1000.0).round() as u64;
        let name = output.name();
        let stale = self.pictures.get(&name).is_none_or(|(k, s, _, _)| *k != key || *s != self.stage);
        if stale && let Some((buffer, dims)) = picture(self.why, scale, self.stage) {
            self.pictures.insert(name.clone(), (key, self.stage, buffer, dims));
        }
        let alpha = self.alpha();
        let mut out = Vec::new();
        if let Some((_, _, buffer, (w, h))) = self.pictures.get(&name) {
            let at = (((size.w - w) / 2) as f64, ((size.h - h) * 2 / 5) as f64);
            if let Some(e) = crate::render::picture(renderer, at.into(), buffer, (*w, *h), Some(alpha), Kind::Unspecified) {
                out.push(CustomRenderElements::Overlay(e));
            }
        }
        let id = self.ids.entry(name).or_insert_with(Id::new).clone();
        let mut bg = BACKGROUND;
        bg[3] = alpha;
        // (Its commit changes with its alpha, so the fade is drawn.)
        let commit = CommitCounter::from((alpha * 255.0) as usize);
        out.push(CustomRenderElements::Warm(SolidColorRenderElement::new(id, Rectangle::from_size(size), commit, bg, Kind::Unspecified)));
        out
    }
}

impl std::fmt::Debug for Veil {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Veil").field("expected", &self.expected).field("stage", &self.stage).field("ending", &self.ending).finish()
    }
}

/// What `output`'s frame shows of the veil, if there is one (in front
/// of everything): `displays` with a mode and `back` windows say where the
/// restart is; while locked, none (and it's done). Whether another frame
/// is wanted (a stage reached, the fade going on, or it just ended).
pub fn frame<R>(veil: &mut Option<Veil>, locked: bool, displays: usize, back: usize, shell: (bool, bool), renderer: &mut R, output: &Output) -> (Vec<CustomRenderElements<R>>, bool)
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let Some(v) = veil.as_mut() else { return (Vec::new(), false) };
    if locked {
        *veil = None;
        return (Vec::new(), true);
    }
    let (alive, changed) = v.update(displays, back, shell);
    if !alive {
        *veil = None;
        return (Vec::new(), true);
    }
    (v.elements(renderer, output), changed)
}

/// Whether the desktop (Tracker's `desktop` layer) and Deskbar (its
/// `deskbar` layer) show on any display: a start's last stages.
pub fn shell_up(space: &smithay::desktop::Space<crate::shell::WindowElement>) -> (bool, bool) {
    let has = |ns: &str| space.outputs().any(|o| smithay::desktop::layer_map_for_output(o).layers().any(|l| l.namespace() == ns));
    (has("desktop"), has("deskbar"))
}

/// A file of Olo's data (`data/…`), where it is.
fn data(rel: &str) -> Option<PathBuf> {
    olo_xdg::data_dirs().iter().map(|d| d.join(rel)).find(|p| p.exists())
}

/// The biggest of `sizes` no bigger than `want` (else the smallest).
fn fitting(sizes: &[u32], want: u32) -> u32 {
    sizes.iter().copied().filter(|&s| s <= want).max().unwrap_or(sizes[0])
}

fn load(path: PathBuf) -> Option<Pixmap> {
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut data = img.into_raw();
    // Premultiplied, as tiny-skia keeps it.
    for p in data.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h)?)
}

/// The picture at `scale` with `stage` stages done, as a buffer to show.
fn picture(why: Why, scale: f64, stage: usize) -> Option<(MemoryRenderBuffer, (i32, i32))> {
    let pm = drawn(why, scale, stage)?;
    let (width, height) = (pm.width() as i32, pm.height() as i32);
    let size: Size<i32, Buffer> = (width, height).into();
    // (Not opaque: the background shows round the wordmark and icons.)
    let buffer = MemoryRenderBuffer::from_slice(pm.data(), Fourcc::Abgr8888, size, 1, Transform::Normal, None);
    Some((buffer, (width, height)))
}

/// The picture at `scale` with `stage` stages done: the wordmark, the
/// icons (dim till lit), the line under them; whole device pixels.
fn drawn(why: Why, scale: f64, stage: usize) -> Option<Pixmap> {
    let (icon_names, says) = stages(why);
    let px = |pt: u32| (pt as f64 * scale).round() as u32;
    let wordmark_w = fitting(&[256, 320, 384, 448, 512], px(WORDMARK));
    let wordmark = load(data(&format!("brand/sizes/olo-wordmark-on-dark-{wordmark_w}.png"))?)?;
    let icon_slot = px(ICON);
    let icon_px = fitting(&[16, 24, 32, 48, 64, 96, 128, 256], icon_slot);
    let icons: Vec<Pixmap> = icon_names.iter().filter_map(|n| load(data(&format!("icons/{icon_px}/{n}.png"))?)).collect();
    let gap = px(GAP);
    let row_w = icon_slot * icon_names.len() as u32 + gap * (icon_names.len() as u32 - 1);
    let text_size = (TEXT as f64 * scale) as f32;
    let line_h = (text_size * 1.6).ceil() as u32;
    let width = row_w.max(wordmark.width()).max(px(360));
    let height = wordmark.height() + px(28) + icon_slot + px(20) + line_h;
    let mut pm = Pixmap::new(width, height)?;
    let paint = |opacity: f32| PixmapPaint { opacity, quality: tiny_skia::FilterQuality::Nearest, ..Default::default() };
    // (Integer places: drawn pixel for pixel.)
    let x = ((width - wordmark.width()) / 2) as i32;
    pm.draw_pixmap(x, 0, wordmark.as_ref(), &paint(1.0), tiny_skia::Transform::identity(), None);
    let top = wordmark.height() + px(28);
    let left = (width - row_w) / 2;
    for (k, icon) in icons.iter().enumerate() {
        let slot_x = left + k as u32 * (icon_slot + gap);
        let (ix, iy) = (slot_x + (icon_slot - icon.width()) / 2, top + (icon_slot - icon.height()) / 2);
        // Done: lit. To come: dim, as BeOS's were.
        let opacity = if k < stage { 1.0 } else { 0.28 };
        pm.draw_pixmap(ix as i32, iy as i32, icon.as_ref(), &paint(opacity), tiny_skia::Transform::identity(), None);
    }
    let said = says[stage.saturating_sub(1).min(says.len() - 1)];
    let line = if stage < 4 { format!("{said}…") } else { said.to_owned() };
    let tw = crate::text::measure(&line, text_size, false);
    let ty = top + icon_slot + px(20);
    crate::text::draw(&mut pm, &line, ((width as f32 - tw) / 2.0).round(), ty as f32, text_size, false, [0xd8, 0xd8, 0xdc, 0xff]);
    Some(pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_go_by_displays_and_windows() {
        let mut v = Veil::new(4);
        assert_eq!(v.update(0, 0, (false, false)), (true, false));
        assert_eq!(v.stage, 1);
        assert!(v.update(1, 0, (false, false)).1);
        assert_eq!(v.stage, 2);
        v.update(1, 2, (false, false));
        assert_eq!(v.stage, 3);
        let (alive, changed) = v.update(1, 4, (false, false));
        // (All back, but shown its least first.)
        assert!(alive && changed && v.ending.is_none());
        v.since -= LEAST;
        v.update(1, 4, (false, false));
        assert!(v.ending.is_some());
        // Never back a stage.
        v.update(0, 0, (false, false));
        assert_eq!(v.stage, 4);
        // A start goes by its desktop and Deskbar.
        let mut s = Veil::starting();
        s.update(1, 0, (true, false));
        assert_eq!(s.stage, 3);
        s.update(1, 0, (true, true));
        assert_eq!(s.stage, 4);
    }

    #[test]
    fn sizes_fit_without_resampling() {
        assert_eq!(fitting(&[256, 320, 384, 448, 512], 384), 384);
        assert_eq!(fitting(&[16, 24, 32, 48, 64, 96, 128, 256], 80), 64);
        assert_eq!(fitting(&[16, 24, 32, 48, 64, 96, 128, 256], 112), 96);
    }

    /// `OLO_VEIL_SHEET=/var/tmp/veil.png cargo test -p app_server veil_sheet`:
    /// stages 1, 3 and 4 at 1, 1.5 and 2×, on the background.
    #[test]
    fn veil_sheet() {
        let Some(out) = std::env::var_os("OLO_VEIL_SHEET") else { return };
        let rows: Vec<Vec<Pixmap>> =
            [(Why::Upgrade, 1.0), (Why::Starting, 1.5), (Why::Upgrade, 2.0)].iter().map(|&(why, s)| [1, 3, 4].iter().map(|&st| drawn(why, s, st).unwrap()).collect()).collect();
        let w = rows.iter().map(|r| r.iter().map(|p| p.width() + 20).sum::<u32>()).max().unwrap() + 20;
        let h = rows.iter().map(|r| r.iter().map(|p| p.height()).max().unwrap() + 20).sum::<u32>() + 20;
        let mut sheet = Pixmap::new(w, h).unwrap();
        sheet.fill(tiny_skia::Color::from_rgba8(30, 30, 34, 255));
        let mut y = 20;
        for row in &rows {
            let mut x = 20;
            for p in row {
                sheet.draw_pixmap(x as i32, y as i32, p.as_ref(), &PixmapPaint::default(), tiny_skia::Transform::identity(), None);
                x += p.width() + 20;
            }
            y += row.iter().map(|p| p.height()).max().unwrap() + 20;
        }
        sheet.save_png(out).unwrap();
    }
}

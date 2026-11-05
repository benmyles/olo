//! Each display's colours: what it can show (its EDID's `DisplayColor`),
//! whether it takes HDR, and how Olo draws for it (Preferences ▸
//! Display): plain (an sRGB display, or Vivid), into its gamut (a wide one
//! shown Accurately: sRGB's red is sRGB's red), or HDR (PQ in BT.2020,
//! SDR's white at `sdr_nits`, highlights up to the display's peak).
//!
//! The renderer converts as it draws (Smithay's `gles::color`). In HDR a
//! frame is composed into a float buffer in BT.2020, gamma 2.2 relative
//! to SDR's white (so blending and text look as they do in SDR), then
//! encoded to PQ over what changed (`udev`'s HDR frame). Nothing changes,
//! and nothing costs more, for a display drawn plain.

use std::cell::RefCell;

use olo_protocol::color::{ColorMode, DisplayColor, SDR_NITS};
use smithay::{
    backend::renderer::{
        damage::OutputDamageTracker,
        element::{Element, Id, Kind, RenderElement},
        gles::{
            GlesError, GlesFrame, GlesRenderer, GlesTexture,
            color::{ColorSource, ColorTarget, Primaries, Transfer},
        },
        utils::{CommitCounter, DamageBag, DamageSet, DamageSnapshot, OpaqueRegions},
    },
    output::Output,
    utils::{Buffer, Physical, Rectangle, Scale, Size, Transform},
};

/// What's set for a display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Applied {
    pub mode: ColorMode,
    pub hdr: bool,
    pub sdr_nits: f32,
}

impl Default for Applied {
    fn default() -> Self {
        Applied { mode: ColorMode::Accurate, hdr: false, sdr_nits: SDR_NITS }
    }
}

/// A display's colours, kept with its `Output`.
#[derive(Debug)]
pub struct OutputColor {
    pub display: DisplayColor,
    /// It takes HDR (PQ in BT.2020): its EDID says, or it's a built-in
    /// panel that gives its HDR luminance and the connector takes the
    /// metadata (an eDP OLED's HDR is the panel's, through Intel's HDR
    /// interface, not its EDID's).
    pub hdr_capable: bool,
    /// Its HDR waits on a setting at start (`kernel_drives_panel_hdr`).
    pub hdr_needs_boot: bool,
    applied: RefCell<Applied>,
}

/// Keep what `output` can show.
pub fn init(output: &Output, display: DisplayColor, hdr_capable: bool) {
    init_waiting(output, display, hdr_capable, false);
}

/// Keep what `output` can show, and whether its HDR waits on a setting
/// at start.
pub fn init_waiting(output: &Output, display: DisplayColor, hdr_capable: bool, hdr_needs_boot: bool) {
    output.user_data().insert_if_missing(|| OutputColor { display, hdr_capable, hdr_needs_boot, applied: RefCell::new(Applied::default()) });
}

pub fn of(output: &Output) -> Option<&OutputColor> {
    output.user_data().get::<OutputColor>()
}

/// What's set for `output` now.
pub fn applied(output: &Output) -> Applied {
    of(output).map(|c| *c.applied.borrow()).unwrap_or_default()
}

/// Set how `output` is drawn; whether that changed.
pub fn apply(output: &Output, a: Applied) -> bool {
    let Some(c) = of(output) else { return false };
    let a = Applied { sdr_nits: a.sdr_nits.clamp(MIN_SDR_NITS, MAX_SDR_NITS), ..a };
    c.applied.replace(a) != a
}

/// SDR's white in HDR, cd/m², at least and at most.
pub use olo_protocol::color::{MAX_SDR_NITS, MIN_SDR_NITS};

/// How a frame for a display is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Drawing {
    /// As given: an sRGB display, or a wide one shown Vivid.
    Plain,
    /// Converted into the display's colours.
    Sdr(ColorTarget),
    /// Composed into `blend` (a float buffer), then encoded into `signal`.
    Hdr { blend: ColorTarget, signal: ColorTarget },
}

impl Drawing {
    /// Whether clients' buffers may go straight to the screen (a plane):
    /// only when nothing's converted.
    pub fn scanout(&self) -> bool {
        matches!(self, Drawing::Plain)
    }
}

/// How `output`'s frames are drawn.
pub fn drawing(output: &Output) -> Drawing {
    let Some(c) = of(output) else { return Drawing::Plain };
    drawing_for(&c.display, c.hdr_capable, &c.applied.borrow())
}

pub fn drawing_for(display: &DisplayColor, hdr_capable: bool, a: &Applied) -> Drawing {
    if a.hdr && hdr_capable {
        let sdr = a.sdr_nits.clamp(MIN_SDR_NITS, MAX_SDR_NITS);
        let max = peak_nits(display) / sdr;
        let blend = ColorTarget { primaries: Primaries::BT2020, transfer: Transfer::Gamma22, sdr_nits: sdr, max };
        let signal = ColorTarget { primaries: signal_primaries(display), transfer: Transfer::Pq, sdr_nits: sdr, max };
        return Drawing::Hdr { blend, signal };
    }
    match display.primaries() {
        Some(p) if a.mode == ColorMode::Accurate && display.wide() => Drawing::Sdr(ColorTarget::sdr(p)),
        _ => Drawing::Plain,
    }
}

/// The primaries an HDR signal is sent in: BT.2020 to a display whose
/// EDID says it takes PQ (an HDR monitor: it converts BT.2020 into its
/// gamut, as the signal's colorimetry says); a built-in panel's own to
/// one whose HDR is its panel's (DisplayID's luminance), which shows the
/// signal's numbers as its own colours. The ThinkPad P1 Gen 9's OLED did
/// (eDP 1.5: the kernel drives it through VESA's interface, never
/// switching on its BT.2020 conversion, 0x344 bit 1): sRGB's colours in
/// BT.2020's numbers came out washed out (Ben, 2026-11-05).
pub fn signal_primaries(display: &DisplayColor) -> Primaries {
    if display.pq { Primaries::BT2020 } else { display.primaries().unwrap_or(Primaries::BT2020) }
}

/// Whether the HDR signal is in BT.2020 (said to the display by the
/// connector's colorimetry), else in the panel's own primaries
/// (colorimetry Default).
pub fn signal_is_bt2020(display: &DisplayColor) -> bool {
    signal_primaries(display) == Primaries::BT2020
}

/// What HDR's blending buffer holds, as a texture drawn into the signal.
pub const BLENDED: ColorSource = ColorSource { primaries: Primaries::BT2020, transfer: Transfer::Gamma22 };

/// The display's peak in HDR, cd/m² (1000 when it doesn't say).
pub fn peak_nits(display: &DisplayColor) -> f32 {
    display.peak_nits.filter(|n| *n >= 100.0).unwrap_or(1000.0) as f32
}

/// The kernel's `struct hdr_output_metadata` for HDR on a display: HDMI
/// static metadata type 1, PQ, the display's own primaries and white,
/// its luminances (the TCON tone-maps nothing within them).
pub fn hdr_metadata(display: &DisplayColor) -> [u8; 32] {
    let p = display.primaries().unwrap_or(Primaries::BT2020);
    let mut b = [0u8; 32];
    // metadata_type (u32) 0: HDMI_STATIC_METADATA_TYPE1.
    b[4] = 2; // eotf: SMPTE ST 2084
    b[5] = 0; // static metadata descriptor 1
    let xy = |v: f64| ((v * 50_000.0).round().clamp(0.0, 50_000.0) as u16).to_le_bytes();
    let mut at = 6;
    for c in [p.red, p.green, p.blue, p.white] {
        b[at..at + 2].copy_from_slice(&xy(c.x));
        b[at + 2..at + 4].copy_from_slice(&xy(c.y));
        at += 4;
    }
    let peak = peak_nits(display).round() as u16;
    let full = display.full_nits.map_or(peak, |n| (n.round() as u16).min(peak));
    let black = display.black_nits.map_or(1, |n| (n * 10_000.0).round().clamp(1.0, 65_535.0) as u16);
    b[22..24].copy_from_slice(&peak.to_le_bytes()); // max display mastering luminance
    b[24..26].copy_from_slice(&black.to_le_bytes()); // min, 0.0001 cd/m²
    b[26..28].copy_from_slice(&peak.to_le_bytes()); // max content light level
    b[28..30].copy_from_slice(&full.to_le_bytes()); // max frame-average light level
    b
}

/// Whether a connector can take HDR: its EDID says PQ, or it's a built-in
/// panel giving an HDR luminance whose HDR the kernel drives; either way
/// the connector has the metadata and BT.2020 properties.
pub fn hdr_capable(display: &DisplayColor, internal: bool, connector_takes_it: bool) -> bool {
    connector_takes_it && (display.pq || (internal && panel_hdr(display) && kernel_drives_panel_hdr()))
}

/// A built-in panel that gives an HDR luminance (DisplayID's): its HDR
/// is the panel's own interface's, not its EDID's.
pub fn panel_hdr(display: &DisplayColor) -> bool {
    display.peak_nits.is_some_and(|n| n >= 400.0)
}

/// Whether the kernel switches a built-in panel into HDR. Intel's driver
/// uses an eDP panel's HDR interface (its decoding of PQ, its brightness
/// in nits) only if the firmware asks, or the EDID has HDR static
/// metadata, or it was started with `enable_dpcd_backlight=3`: the
/// ThinkPad P1 Gen 9's OLED has neither of the first two, and HDR on it
/// sent a PQ signal it read as SDR. Olo Doctor offers the third.
pub fn kernel_drives_panel_hdr() -> bool {
    std::fs::read_to_string("/proc/cmdline").is_ok_and(|c| c.split_whitespace().any(|a| a == "xe.enable_dpcd_backlight=3" || a == "i915.enable_dpcd_backlight=3"))
}

/// What a headless screen pretends to be (`OLO_HEADLESS_COLOR`: "p3",
/// "p3-hdr", "bt2020-hdr"): for trying wide colour and HDR without one.
pub fn simulated() -> Option<(DisplayColor, bool)> {
    let v = std::env::var("OLO_HEADLESS_COLOR").ok()?;
    let primaries = if v.starts_with("bt2020") { Primaries::BT2020 } else { Primaries::DISPLAY_P3 };
    let hdr = v.ends_with("-hdr");
    let display = DisplayColor {
        primaries: Some(primaries),
        gamma: Some(2.2),
        peak_nits: Some(if hdr { 1600.0 } else { 500.0 }),
        full_nits: Some(if hdr { 650.0 } else { 500.0 }),
        black_nits: None,
        pq: hdr,
        hlg: false,
        bt2020: hdr,
    };
    Some((display, hdr))
}

/// HDR's blending buffer for a display, kept from frame to frame: what's
/// on screen, in BT.2020 at gamma 2.2 relative to SDR's white (float,
/// so highlights go above 1), unrotated (the display's compositor turns
/// the encoded frame as it turns everything).
pub struct HdrFrame {
    pub texture: GlesTexture,
    /// Composing into it: only what changed is drawn again.
    pub tracker: OutputDamageTracker,
    damage: DamageBag<i32, Physical>,
    id: Id,
    size: Size<i32, Physical>,
    scale: f64,
}

impl HdrFrame {
    /// Around a float texture of `size` (the display's, as it's turned).
    pub fn new(texture: GlesTexture, size: Size<i32, Physical>, scale: f64) -> HdrFrame {
        texture.set_color_source(Some(BLENDED));
        HdrFrame { texture, tracker: OutputDamageTracker::new(size, scale, Transform::Normal), damage: DamageBag::new(4), id: Id::new(), size, scale }
    }

    pub fn size(&self) -> Size<i32, Physical> {
        self.size
    }

    /// Composed whole at the next frame (SDR's white moved: what's in it
    /// was counted in the old), the texture kept.
    pub fn compose_whole(&mut self) {
        self.tracker = OutputDamageTracker::new(self.size, self.scale, Transform::Normal);
    }

    /// What a frame drew into it (None: nothing new).
    pub fn drawn(&mut self, damage: Option<&Vec<Rectangle<i32, Physical>>>) {
        if let Some(d) = damage.filter(|d| !d.is_empty()) {
            self.damage.add(d.iter().copied());
        }
    }

    /// It, as one element covering the display, damaged where it changed.
    pub fn element(&self) -> HdrElement {
        HdrElement { id: self.id.clone(), texture: self.texture.clone(), damage: self.damage.snapshot(), size: self.size }
    }
}

/// HDR's blending buffer drawn into the display's signal (the renderer's
/// target then: PQ in BT.2020).
pub struct HdrElement {
    id: Id,
    texture: GlesTexture,
    damage: DamageSnapshot<i32, Physical>,
    size: Size<i32, Physical>,
}

impl HdrElement {
    fn whole(&self) -> Rectangle<i32, Physical> {
        Rectangle::from_size(self.size)
    }

    /// Draw it in a GLES frame (what each backend's renderer has).
    pub fn draw_gles(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.render_texture_from_to(&self.texture, src, dst, damage, opaque, Transform::Normal, 1.0, None, &[])
    }
}

impl Element for HdrElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.damage.current_commit()
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size((f64::from(self.size.w), f64::from(self.size.h)).into())
    }

    fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.whole()
    }

    fn damage_since(&self, _scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        self.damage.damage_since(commit).unwrap_or_else(|| DamageSet::from_slice(&[self.whole()]))
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::from_slice(&[self.whole()])
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for HdrElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
        _cache: Option<&smithay::utils::user_data::UserDataMap>,
    ) -> Result<(), GlesError> {
        self.draw_gles(frame, src, dst, damage, opaque_regions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oled() -> DisplayColor {
        // The ThinkPad P1 Gen 9's, as its EDID says.
        let c = |x, y| olo_color::Chromaticity { x, y };
        DisplayColor {
            primaries: Some(Primaries { red: c(0.6809, 0.3181), green: c(0.2200, 0.7251), blue: c(0.1431, 0.0391), white: c(0.3130, 0.3291) }),
            gamma: Some(2.2),
            peak_nits: Some(1600.0),
            full_nits: Some(650.0),
            black_nits: None,
            ..DisplayColor::default()
        }
    }

    #[test]
    fn how_each_display_is_drawn() {
        let d = oled();
        // Wide: converted into its gamut unless Vivid.
        assert!(matches!(drawing_for(&d, true, &Applied::default()), Drawing::Sdr(t) if t.primaries == d.primaries.unwrap()));
        assert_eq!(drawing_for(&d, true, &Applied { mode: ColorMode::Vivid, ..Applied::default() }), Drawing::Plain);
        // HDR: blended in BT.2020 at SDR's white, up to its peak.
        let hdr = drawing_for(&d, true, &Applied { hdr: true, ..Applied::default() });
        let Drawing::Hdr { blend, signal } = hdr else { panic!("{hdr:?}") };
        assert_eq!((blend.transfer, signal.transfer), (Transfer::Gamma22, Transfer::Pq));
        assert!((signal.max - 1600.0 / 203.0).abs() < 1e-3);
        // Not HDR where it can't; sRGB displays plain.
        assert!(matches!(drawing_for(&d, false, &Applied { hdr: true, ..Applied::default() }), Drawing::Sdr(_)));
        assert_eq!(drawing_for(&DisplayColor::default(), false, &Applied::default()), Drawing::Plain);
        assert!(!hdr.scanout() && Drawing::Plain.scanout());
    }

    #[test]
    fn a_panels_hdr_in_its_own_colours_a_monitors_in_bt2020() {
        let applied = Applied { hdr: true, ..Applied::default() };
        let Drawing::Hdr { blend, signal } = drawing_for(&oled(), true, &applied) else { panic!("HDR") };
        assert_eq!(blend.primaries, Primaries::BT2020, "blended in BT.2020 either way");
        assert_eq!(Some(signal.primaries), oled().primaries(), "the panel's own");
        assert!(!signal_is_bt2020(&oled()));
        let monitor = DisplayColor { pq: true, ..oled() };
        let Drawing::Hdr { signal, .. } = drawing_for(&monitor, true, &applied) else { panic!("HDR") };
        assert_eq!(signal.primaries, Primaries::BT2020);
        assert!(signal_is_bt2020(&monitor));
    }

    #[test]
    fn hdr_metadata_as_the_kernel_reads_it() {
        let m = hdr_metadata(&oled());
        assert_eq!(&m[..6], &[0, 0, 0, 0, 2, 0]);
        // Red x 0.6809 → 34045; white y 0.3291 → 16455.
        assert_eq!(u16::from_le_bytes([m[6], m[7]]), 34045);
        assert_eq!(u16::from_le_bytes([m[20], m[21]]), 16455);
        assert_eq!(u16::from_le_bytes([m[22], m[23]]), 1600);
        assert_eq!(u16::from_le_bytes([m[28], m[29]]), 650);
        // (Built in: only as the kernel drives its HDR.)
        assert_eq!(hdr_capable(&oled(), true, true), kernel_drives_panel_hdr());
        assert!(!hdr_capable(&oled(), false, true) && !hdr_capable(&oled(), true, false));
        let hdr10 = DisplayColor { pq: true, ..oled() };
        assert!(hdr_capable(&hdr10, false, true));
    }

    /// The renderer's conversions on this computer's GPU, read back and
    /// compared with the CPU's (`gles::color::convert`): sRGB into P3,
    /// and HDR's two passes. Skipped without a render node.
    #[test]
    fn the_gpu_converts_as_the_cpu_does() {
        use smithay::backend::{
            allocator::{Fourcc, gbm::GbmDevice},
            egl::{EGLContext, EGLDisplay},
            renderer::{Bind, Color32F, ExportMem, Frame, ImportMem, Offscreen, Renderer, element::Element, gles::color::convert},
        };
        let Some(node) = crate::headless::render_node() else { return };
        let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).open(&node) else { return };
        let gbm = GbmDevice::new(file).unwrap();
        let egl = unsafe { EGLDisplay::new(gbm) }.unwrap();
        let mut r = unsafe { GlesRenderer::new(EGLContext::new(&egl).unwrap()) }.unwrap();
        let size: Size<i32, Physical> = (4, 1).into();
        let whole = Rectangle::from_size(size);
        // An sRGB texture: red, a grey, green, white.
        let pixels: Vec<u8> = [[255, 0, 0, 255], [128, 128, 128, 255], [0, 255, 0, 255], [255, 255, 255, 255]].concat();
        let tex = r.import_memory(&pixels, Fourcc::Abgr8888, (4, 1).into(), false).unwrap();
        let read = |r: &mut GlesRenderer, target: &mut GlesTexture, fourcc: Fourcc| -> Vec<u8> {
            let fb = r.bind(target).unwrap();
            let m = r.copy_framebuffer(&fb, Rectangle::from_size((4, 1).into()), fourcc).unwrap();
            r.map_texture(&m).unwrap().to_vec()
        };
        let f16 = |b: &[u8], i: usize| half(u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]));

        // sRGB into P3, into a float target (no rounding to compare).
        let p3 = ColorTarget::sdr(Primaries::DISPLAY_P3);
        let mut target: GlesTexture = r.create_buffer(Fourcc::Abgr16161616f, (4, 1).into()).unwrap();
        r.set_color_target(Some(p3));
        {
            let mut fb = r.bind(&mut target).unwrap();
            let mut frame = r.render(&mut fb, size, Transform::Normal).unwrap();
            frame.clear(Color32F::TRANSPARENT, &[whole]).unwrap();
            frame.render_texture_from_to(&tex, Rectangle::from_size((4.0, 1.0).into()), whole, &[whole], &[], Transform::Normal, 1.0, None, &[]).unwrap();
            // A solid sRGB red over the last pixel.
            frame.draw_solid(Rectangle::new((3, 0).into(), (1, 1).into()), &[Rectangle::from_size((1, 1).into())], Color32F::new(1.0, 0.0, 0.0, 1.0)).unwrap();
            frame.finish().unwrap();
        }
        r.set_color_target(None);
        let b = read(&mut r, &mut target, Fourcc::Abgr16161616f);
        let want = |rgb: [f32; 3]| convert([rgb[0], rgb[1], rgb[2], 1.0], &ColorSource::SRGB, &p3);
        for (i, rgb) in [[1.0, 0.0, 0.0], [128.0 / 255.0; 3], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]].iter().enumerate() {
            let got = [f16(&b, i * 4), f16(&b, i * 4 + 1), f16(&b, i * 4 + 2)];
            let w = want(*rgb);
            for c in 0..3 {
                assert!((got[c] - f64::from(w[c])).abs() < 2e-3, "pixel {i}: GPU {got:?}, CPU {w:?}");
            }
        }

        // HDR: composed into the blending buffer, then encoded to PQ.
        let d = DisplayColor { primaries: Some(Primaries::DISPLAY_P3), peak_nits: Some(1600.0), pq: true, ..DisplayColor::default() };
        let Drawing::Hdr { blend, signal } = drawing_for(&d, true, &Applied { hdr: true, ..Applied::default() }) else { panic!() };
        let blend_tex: GlesTexture = r.create_buffer(Fourcc::Abgr16161616f, (4, 1).into()).unwrap();
        let mut hdr = HdrFrame::new(blend_tex, size, 1.0);
        r.set_color_target(Some(blend));
        {
            let mut fb = r.bind(&mut hdr.texture).unwrap();
            let mut frame = r.render(&mut fb, size, Transform::Normal).unwrap();
            frame.clear(Color32F::TRANSPARENT, &[whole]).unwrap();
            frame.render_texture_from_to(&tex, Rectangle::from_size((4.0, 1.0).into()), whole, &[whole], &[], Transform::Normal, 1.0, None, &[]).unwrap();
            frame.finish().unwrap();
        }
        hdr.drawn(Some(&vec![whole]));
        let element = hdr.element();
        assert_eq!(element.geometry(Scale::from(1.0)), whole);
        r.set_color_target(Some(signal));
        let mut out: GlesTexture = r.create_buffer(Fourcc::Abgr16161616f, (4, 1).into()).unwrap();
        {
            let mut fb = r.bind(&mut out).unwrap();
            let mut frame = r.render(&mut fb, size, Transform::Normal).unwrap();
            element.draw_gles(&mut frame, element.src(), whole, &[whole], &[]).unwrap();
            frame.finish().unwrap();
        }
        r.set_color_target(None);
        let b = read(&mut r, &mut out, Fourcc::Abgr16161616f);
        // SDR's white at 203 cd/m²: PQ 0.5807, every channel.
        let white = [f16(&b, 12), f16(&b, 13), f16(&b, 14)];
        assert!(white.iter().all(|v| (v - 0.5807).abs() < 2e-3), "{white:?}");
        // sRGB's red as the CPU puts it straight into the signal.
        let red = convert([1.0, 0.0, 0.0, 1.0], &ColorSource::SRGB, &signal);
        let got = [f16(&b, 0), f16(&b, 1), f16(&b, 2)];
        for c in 0..3 {
            assert!((got[c] - f64::from(red[c])).abs() < 3e-3, "GPU {got:?}, CPU {red:?}");
        }
    }

    /// What converting costs the GPU, a 3200 × 2000 frame (the ThinkPad's
    /// panel) drawn whole: plain, into P3, and HDR's two passes
    /// (`cargo test --release -p app_server --lib color::tests::gpu_cost -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn gpu_cost() {
        use smithay::backend::{
            allocator::{Fourcc, gbm::GbmDevice},
            egl::{EGLContext, EGLDisplay},
            renderer::{Bind, Color32F, Frame, ImportMem, Offscreen, Renderer},
        };
        let Some(node) = crate::headless::render_node() else { return };
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&node).unwrap();
        let egl = unsafe { EGLDisplay::new(GbmDevice::new(file).unwrap()) }.unwrap();
        let mut r = unsafe { GlesRenderer::new(EGLContext::new(&egl).unwrap()) }.unwrap();
        let (w, h) = (3200, 2000);
        let size: Size<i32, Physical> = (w, h).into();
        let whole = Rectangle::from_size(size);
        let pixels: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, (i % 241) as u8, (i % 239) as u8, 255]).collect();
        let tex = r.import_memory(&pixels, Fourcc::Abgr8888, (w, h).into(), false).unwrap();
        let mut target: GlesTexture = r.create_buffer(Fourcc::Abgr2101010, (w, h).into()).unwrap();
        let src = Rectangle::from_size((f64::from(w), f64::from(h)).into());
        let draw = |r: &mut GlesRenderer, target: &mut GlesTexture, tex: &GlesTexture| {
            let mut fb = r.bind(target).unwrap();
            let mut frame = r.render(&mut fb, size, Transform::Normal).unwrap();
            frame.render_texture_from_to(tex, src, whole, &[whole], &[whole], Transform::Normal, 1.0, None, &[]).unwrap();
            frame.finish().unwrap().wait().unwrap();
        };
        let time = |r: &mut GlesRenderer, target: &mut GlesTexture, f: &mut dyn FnMut(&mut GlesRenderer, &mut GlesTexture)| {
            for _ in 0..5 {
                f(r, target);
            }
            let n = 60;
            let t = std::time::Instant::now();
            for _ in 0..n {
                f(r, target);
            }
            t.elapsed().as_secs_f64() * 1e3 / f64::from(n)
        };
        let plain = time(&mut r, &mut target, &mut |r, t| draw(r, t, &tex));
        r.set_color_target(Some(ColorTarget::sdr(Primaries::DISPLAY_P3)));
        let p3 = time(&mut r, &mut target, &mut |r, t| draw(r, t, &tex));
        let d = DisplayColor { primaries: Some(Primaries::DISPLAY_P3), peak_nits: Some(1600.0), pq: true, ..DisplayColor::default() };
        let Drawing::Hdr { blend, signal } = drawing_for(&d, true, &Applied { hdr: true, ..Applied::default() }) else { panic!() };
        let mut blend_tex: GlesTexture = r.create_buffer(Fourcc::Abgr16161616f, (w, h).into()).unwrap();
        blend_tex.set_color_source(Some(BLENDED));
        let hdr = time(&mut r, &mut target, &mut |r, t| {
            r.set_color_target(Some(blend));
            draw(r, &mut blend_tex, &tex);
            r.set_color_target(Some(signal));
            draw(r, t, &blend_tex.clone());
        });
        r.set_color_target(None);
        eprintln!("a whole 3200 × 2000 frame, GPU: plain {plain:.3} ms, into P3 {p3:.3} ms, HDR (blend + PQ) {hdr:.3} ms");
    }

    fn half(h: u16) -> f64 {
        let (exp, frac) = (i32::from((h >> 10) & 0x1f), f64::from(h & 0x3ff));
        let v = match exp {
            0 => frac * 2f64.powi(-24),
            _ => (1.0 + frac / 1024.0) * 2f64.powi(exp - 15),
        };
        if h & 0x8000 != 0 { -v } else { v }
    }
}

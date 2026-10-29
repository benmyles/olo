//! Fonts, egui visuals and the small set of pixel-exact drawing primitives
//! every Olo widget is built from. All geometry is in logical pixels; a
//! "line" is one logical pixel (two device pixels at 2x).

use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Painter, Pos2, Rect,
    Shadow, Stroke, Vec2, Visuals, pos2, vec2,
};
use olo_theme::{Palette, Rgba, mix};

/// be_plain_font, in logical pixels.
pub const PLAIN_SIZE: f32 = 12.0;
pub const SMALL_SIZE: f32 = 10.5;
pub const MONO_SIZE: f32 = 12.0;

pub fn c(rgba: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3])
}

pub fn darken(rgba: Rgba, tint: f32) -> Color32 {
    c(olo_theme::tint(rgba, tint))
}

pub fn lighten(rgba: Rgba, tint: f32) -> Color32 {
    c(olo_theme::tint(rgba, tint))
}

pub fn disabled_text(p: &Palette) -> Color32 {
    c(mix(p.panel_text, p.panel_background, 0.55))
}

/// The palette in effect for this context.
pub fn palette(ctx: &Context) -> Arc<Palette> {
    ctx.data(|d| d.get_temp::<Arc<Palette>>(egui::Id::NULL)).unwrap_or_else(|| {
        static DEFAULT: OnceLock<Arc<Palette>> = OnceLock::new();
        DEFAULT.get_or_init(|| Arc::new(olo_theme::looks::palette(olo_theme::looks::DEFAULT, "", olo_theme::ThemeMode::Light))).clone()
    })
}

pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// Italic plain text (Tracker shows symbolic links this way, as BeOS did).
pub fn italic() -> FontFamily {
    FontFamily::Name("italic".into())
}

pub fn italic_font() -> FontId {
    FontId::new(PLAIN_SIZE, italic())
}

pub fn plain_font() -> FontId {
    FontId::proportional(PLAIN_SIZE)
}

/// A point smaller than plain: secondary lines under a name.
pub fn small_font() -> FontId {
    FontId::proportional(PLAIN_SIZE - 1.0)
}

pub fn bold_font() -> FontId {
    FontId::new(PLAIN_SIZE, bold())
}

pub fn mono_font(size: f32) -> FontId {
    FontId::monospace(size)
}

pub fn mono_bold() -> FontFamily {
    FontFamily::Name("mono-bold".into())
}

pub fn mono_italic() -> FontFamily {
    FontFamily::Name("mono-italic".into())
}

// ---- fonts ------------------------------------------------------------

/// The first installed font of `families` (data and face index), for
/// documents that name fonts of their own. Scans the system fonts: call it
/// off the UI thread.
pub fn find_font(families: &[&str], weight: u16) -> Option<(Vec<u8>, u32)> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    load_face(&db, families, weight)
}

fn load_face(db: &fontdb::Database, families: &[&str], weight: u16) -> Option<(Vec<u8>, u32)> {
    load_styled(db, families, weight, fontdb::Style::Normal)
}

fn load_styled(db: &fontdb::Database, families: &[&str], weight: u16, style: fontdb::Style) -> Option<(Vec<u8>, u32)> {
    let id = query(db, families, weight, style)?;
    db.with_face_data(id, |data, index| (data.to_vec(), index))
}

fn query(db: &fontdb::Database, families: &[&str], weight: u16, style: fontdb::Style) -> Option<fontdb::ID> {
    families.iter().find_map(|name| {
        db.query(&fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            weight: fontdb::Weight(weight),
            stretch: fontdb::Stretch::Normal,
            style,
        })
    })
}

/// A face for the UI fonts: its file mapped rather than read, so the
/// bytes live in the page cache, shared by every app, and only the parts
/// actually used are resident (the CJK font alone is over 30 MB).
fn system_face(db: &fontdb::Database, families: &[&str], weight: u16, style: fontdb::Style) -> Option<(Cow<'static, [u8]>, u32)> {
    let id = query(db, families, weight, style)?;
    let face = db.face(id)?;
    let path = match &face.source {
        fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => Some(p.clone()),
        fontdb::Source::Binary(_) => None,
    };
    if let Some(data) = path.as_deref().and_then(mapped) {
        return Some((Cow::Borrowed(data), face.index));
    }
    db.with_face_data(id, |data, index| (Cow::Owned(data.to_vec()), index))
}

/// `path` mapped for the life of the process (once per file).
pub(crate) fn mapped(path: &Path) -> Option<&'static [u8]> {
    static MAPS: OnceLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = OnceLock::new();
    let mut maps = MAPS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(data) = maps.get(path) {
        return Some(data);
    }
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: installed font files are replaced by rename when packages
    // update, never rewritten in place, so the mapping stays valid.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let data: &'static [u8] = &Box::leak(Box::new(map))[..];
    maps.insert(path.to_owned(), data);
    Some(data)
}

/// System fonts, loaded once per process and shared by every window.
pub fn font_definitions() -> FontDefinitions {
    static DEFS: OnceLock<FontDefinitions> = OnceLock::new();
    DEFS.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut defs = FontDefinitions::default();
        let mut add = |key: &str, face: Option<(Cow<'static, [u8]>, u32)>| -> bool {
            let Some((data, index)) = face else {
                return false;
            };
            let mut fd = match data {
                Cow::Borrowed(d) => FontData::from_static(d),
                Cow::Owned(d) => FontData::from_owned(d),
            };
            fd.index = index;
            // Hinted vertically only ("slight", as FreeType, GTK 4.16 and
            // Chrome do it), with egui's subpixel positioning across: stems
            // and the baseline on device pixels at any scale, and shapes and
            // spacing as designed. Nimbus Sans (CFF) and JetBrains Mono (its
            // own instructions) hint that way anyway; this keeps fallback
            // fonts the autohinter does (symbols, CJK) the same.
            fd.tweak.hinting = Some(true);
            fd.tweak.hinting_target = egui::epaint::text::HintingTarget::Smooth(egui::epaint::text::SmoothHinting { light: true, ..Default::default() });
            defs.font_data.insert(key.to_owned(), Arc::new(fd));
            true
        };
        let ui = add("ui", system_face(&db, olo_theme::fonts::UI, 400, fontdb::Style::Normal));
        let ui_bold = add("ui-bold", system_face(&db, olo_theme::fonts::UI, 700, fontdb::Style::Normal));
        let ui_italic = add("ui-italic", system_face(&db, olo_theme::fonts::UI, 400, fontdb::Style::Italic));
        let mono = add("mono", system_face(&db, olo_theme::fonts::MONO, 400, fontdb::Style::Normal));
        let mono_bold = add("mono-bold", system_face(&db, olo_theme::fonts::MONO, 700, fontdb::Style::Normal));
        let mono_italic = add("mono-italic", system_face(&db, olo_theme::fonts::MONO, 400, fontdb::Style::Italic));
        // Wide Unicode coverage (arrows, ⌘, box drawing, ...).
        let fallback = add("fallback", system_face(&db, &["DejaVu Sans", "Noto Sans"], 400, fontdb::Style::Normal));
        let symbols = add("symbols", system_face(&db, &["Noto Sans Symbols 2", "Noto Sans Symbols"], 400, fontdb::Style::Normal));
        // Chinese, Japanese and Korean, then emoji (monochrome: egui can't
        // draw colour glyphs; see `emoji` for those).
        let cjk = add("cjk", system_face(&db, &["Noto Sans Mono CJK JP", "Noto Sans CJK JP"], 400, fontdb::Style::Normal));
        let emoji = add("emoji", system_face(&db, &["Noto Emoji"], 400, fontdb::Style::Normal));
        crate::emoji::locate(&db);

        let prop = defs.families.entry(FontFamily::Proportional).or_default();
        let mut front = Vec::new();
        if ui {
            front.push("ui".to_owned());
        }
        if fallback {
            front.push("fallback".to_owned());
        }
        if symbols {
            front.push("symbols".to_owned());
        }
        if cjk {
            front.push("cjk".to_owned());
        }
        if emoji {
            front.push("emoji".to_owned());
        }
        prop.splice(0..0, front.clone());

        let monos = defs.families.entry(FontFamily::Monospace).or_default();
        let mut mfront = Vec::new();
        if mono {
            mfront.push("mono".to_owned());
        }
        for (have, key) in [(fallback, "fallback"), (symbols, "symbols"), (cjk, "cjk"), (emoji, "emoji")] {
            if have {
                mfront.push(key.to_owned());
            }
        }
        monos.splice(0..0, mfront);
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let chain = defs.families.remove(&family).unwrap_or_default();
            let chain = on_baseline(&mut defs, chain);
            defs.families.insert(family, chain);
        }

        let base = defs.families[&FontFamily::Proportional].clone();
        let mut b = Vec::new();
        if ui_bold {
            b.push("ui-bold".to_owned());
        }
        b.extend(base.iter().cloned());
        defs.families.insert(bold(), b);

        let mut it = Vec::new();
        if ui_italic {
            it.push("ui-italic".to_owned());
        }
        it.extend(base.iter().cloned());
        defs.families.insert(italic(), it);

        let mbase = defs.families[&FontFamily::Monospace].clone();
        let mut mb = Vec::new();
        if mono_bold {
            mb.push("mono-bold".to_owned());
        }
        mb.extend(mbase.iter().cloned());
        defs.families.insert(self::mono_bold(), mb);
        let mut mi = Vec::new();
        if mono_italic {
            mi.push("mono-italic".to_owned());
        }
        mi.extend(mbase.iter().cloned());
        defs.families.insert(self::mono_italic(), mi);
        defs
    })
    .clone()
}

/// The faces we load that [`on_baseline`] moves.
const ON_BASELINE: [&str; 5] = ["ui", "fallback", "symbols", "cjk", "emoji"];

/// `chain` (a family's faces, primary first) with each of our fallback
/// faces swapped for a copy shifted onto the primary's baseline. egui
/// centres a fallback face's line in the primary's instead, so DejaVu's
/// taller line put ▸, ⌘ and → a few pixels below the Nimbus Sans text
/// around them. The copies share the font bytes.
fn on_baseline(defs: &mut FontDefinitions, chain: Vec<String>) -> Vec<String> {
    let Some(primary) = chain.first().cloned() else {
        return chain;
    };
    let Some((ascent, height)) = defs.font_data.get(&primary).and_then(|d| em_metrics(d)) else {
        return chain;
    };
    let mut out = vec![primary.clone()];
    for key in chain.into_iter().skip(1) {
        let face = key.split('@').next().unwrap_or(&key).to_owned();
        let data = defs.font_data.get(&face).filter(|_| ON_BASELINE.contains(&face.as_str())).cloned();
        let Some((data, (a, h))) = data.and_then(|d| em_metrics(&d).map(|m| (d, m))) else {
            out.push(key);
            continue;
        };
        let shifted = format!("{face}@{primary}");
        let mut copy = (*data).clone();
        copy.tweak.y_offset_factor = (ascent - a) + 0.5 * (h - height);
        defs.font_data.insert(shifted.clone(), Arc::new(copy));
        out.push(shifted);
    }
    out
}

/// A face's ascent and line height (ascent, descent and gap) in ems, as
/// egui measures them.
fn em_metrics(data: &FontData) -> Option<(f32, f32)> {
    use skrifa::{
        MetadataProvider,
        instance::{LocationRef, Size},
    };
    let font = skrifa::FontRef::from_index((*data.font).as_ref(), data.index).ok()?;
    let m = font.metrics(Size::unscaled(), LocationRef::default());
    let em = f32::from(m.units_per_em);
    (em > 0.0 && m.ascent > 0.0).then(|| (m.ascent / em, (m.ascent - m.descent + m.leading) / em))
}

/// The height of `text` in the plain font wrapped to `width` points,
/// from the UI face's own advances and line height, wrapped as egui
/// wraps (after a space, the space counting towards the row): for sizing
/// a window before its first frame, when there is nothing yet to lay
/// text out with. Kerning is left out, so it errs towards a line too
/// many rather than too few.
pub fn plain_text_height(text: &str, width: f32) -> f32 {
    use skrifa::{
        MetadataProvider,
        instance::{LocationRef, Size},
    };
    let defs = font_definitions();
    let data = defs.families.get(&FontFamily::Proportional).and_then(|chain| chain.first()).and_then(|key| defs.font_data.get(key));
    let font = data.and_then(|d| skrifa::FontRef::from_index((*d.font).as_ref(), d.index).ok());
    // Rows land on whole device pixels: a whole point covers any scale.
    let line = data.and_then(|d| em_metrics(d)).map_or(PLAIN_SIZE + 4.0, |(_, h)| (h * PLAIN_SIZE).ceil());
    let metrics = font.as_ref().map(|f| (f.charmap(), f.glyph_metrics(Size::new(PLAIN_SIZE), LocationRef::default())));
    // Glyphs the face lacks come from a fallback: about as wide as an n.
    let advance = |ch: char| metrics.as_ref().and_then(|(map, m)| m.advance_width(map.map(ch)?)).unwrap_or(0.6 * PLAIN_SIZE);
    let mut lines = 0;
    for paragraph in text.split('\n') {
        lines += 1;
        let mut x = 0.0;
        for word in paragraph.split_inclusive(' ') {
            let w: f32 = word.chars().map(advance).sum();
            if x > 0.0 && x + w > width {
                lines += 1;
                x = 0.0;
            }
            // A word wider than the line breaks anywhere.
            x += w;
            while x > width {
                lines += 1;
                x -= width;
            }
        }
    }
    lines as f32 * line
}

// ---- document fonts ---------------------------------------------------

/// Fonts an app added at run time, as (font key, data, family).
static EXTRA_FONTS: std::sync::Mutex<Vec<(String, Arc<FontData>, String)>> = std::sync::Mutex::new(Vec::new());
static FONTS_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Make a font a document can name (a PDF's, a StyledEdit family) usable
/// as `FontFamily::Name(family)` in every window of the app: now, in
/// windows opened later and after a reconnect. Glyphs it lacks come from
/// the UI fonts. It takes effect from each window's next frame, so check
/// [`family_ready`] before naming it (egui panics on an unknown family).
pub fn add_document_font(family: &str, data: Vec<u8>, index: u32) {
    let mut fd = FontData::from_owned(data);
    fd.index = index;
    let mut extra = EXTRA_FONTS.lock().unwrap();
    if extra.iter().any(|(_, _, f)| f == family) {
        return;
    }
    extra.push((format!("doc:{family}"), Arc::new(fd), family.to_owned()));
    FONTS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Bumped by every [`add_document_font`].
pub fn fonts_generation() -> u64 {
    FONTS_GENERATION.load(std::sync::atomic::Ordering::SeqCst)
}

/// The UI fonts plus the document fonts added so far.
pub fn all_font_definitions() -> FontDefinitions {
    let mut defs = font_definitions();
    let fallbacks = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (key, data, family) in EXTRA_FONTS.lock().unwrap().iter() {
        defs.font_data.insert(key.clone(), data.clone());
        let mut list = vec![key.clone()];
        list.extend(fallbacks.iter().cloned());
        let list = on_baseline(&mut defs, list);
        defs.families.insert(FontFamily::Name(family.as_str().into()), list);
    }
    defs
}

/// Can this frame lay out text in `family`?
pub fn family_ready(ctx: &Context, family: &str) -> bool {
    ctx.fonts(|f| f.definitions().families.contains_key(&FontFamily::Name(family.into())))
}

/// Apply the palette to a context: visuals, and stash it for widgets.
pub fn apply(ctx: &Context, palette: &Arc<Palette>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::NULL, palette.clone()));
    let p = palette;
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let square = CornerRadius::ZERO;
    v.override_text_color = Some(c(p.panel_text));
    v.panel_fill = c(p.panel_background);
    v.window_fill = c(p.panel_background);
    v.window_corner_radius = square;
    v.menu_corner_radius = square;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.window_stroke = Stroke::new(1.0, darken(p.panel_background, 1.55));
    v.extreme_bg_color = c(p.document_background);
    v.text_edit_bg_color = Some(c(p.document_background));
    v.faint_bg_color = darken(p.list_background, 1.04);
    v.code_bg_color = darken(p.document_background, 1.05);
    v.hyperlink_color = c(p.navigation_base);
    v.selection.bg_fill = c(p.control_highlight).gamma_multiply(if dark { 0.75 } else { 0.55 });
    v.selection.stroke = Stroke::new(1.0, c(p.document_text));
    v.text_cursor.stroke = Stroke::new(1.0, c(p.navigation_base));
    v.button_frame = true;
    v.striped = false;
    v.indent_has_left_vline = false;
    v.interact_cursor = None;
    v.image_loading_spinners = false;
    for (w, bg) in [
        (&mut v.widgets.noninteractive, p.panel_background),
        (&mut v.widgets.inactive, p.control_background),
        (&mut v.widgets.hovered, p.control_background),
        (&mut v.widgets.active, p.control_background),
        (&mut v.widgets.open, p.control_background),
    ] {
        w.corner_radius = square;
        w.bg_fill = c(bg);
        w.weak_bg_fill = c(bg);
        w.bg_stroke = Stroke::new(1.0, darken(p.panel_background, 1.4));
        w.fg_stroke = Stroke::new(1.0, c(p.control_text));
        w.expansion = 0.0;
    }
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, c(p.panel_text));
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, c(p.navigation_base));
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(crate::metrics::SPACING, crate::metrics::SPACING);
        s.spacing.button_padding = vec2(crate::metrics::CONTROL_PAD, 4.0);
        s.spacing.interact_size = vec2(24.0, crate::metrics::CONTROL_HEIGHT);
        s.spacing.window_margin = egui::Margin::same(crate::metrics::WINDOW_INSET as i8);
        s.spacing.scroll = egui::style::ScrollStyle::solid();
        s.spacing.scroll.bar_width = 14.0;
        s.text_styles.insert(egui::TextStyle::Body, plain_font());
        s.text_styles.insert(egui::TextStyle::Button, plain_font());
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(SMALL_SIZE));
        s.text_styles.insert(egui::TextStyle::Monospace, mono_font(MONO_SIZE));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::new(15.0, bold()));
        s.animation_time = 0.12;
    });
}

// ---- primitives ---------------------------------------------------------

/// How many device pixels thick a one-point line is at `ppp` device
/// pixels a point: a whole number, and no more than the point holds (1 up
/// to 2×, then 2 …), so at any scale every line is crisp, all are one
/// weight, and none runs into what is beside it. (Each edge rounded on
/// its own, as egui does, makes some 1 px and some 2 px at 1.5×.)
pub fn line_pixels(ppp: f32) -> f32 {
    ppp.floor().max(1.0)
}

/// What in the style depends on the scale, set for `ppp` device pixels a
/// point: the text caret is a hairline (a one-point stroke is a pixel and
/// a half at 1.5×, and smears over two). The platform calls it before
/// every frame; it writes only when the scale has changed.
pub fn fit_scale(ctx: &Context, ppp: f32) {
    let width = line_pixels(ppp) / ppp;
    if (ctx.global_style().visuals.text_cursor.stroke.width - width).abs() > 1e-4 {
        ctx.all_styles_mut(|s| s.visuals.text_cursor.stroke.width = width);
    }
}

/// A one-point line's width in points at this scale: a stroke's, so
/// outlines are whole device pixels too (`Stroke::new(hairline(ctx), c)`;
/// a 1.0 stroke blurs at 1.5×).
pub fn hairline(ctx: &Context) -> f32 {
    let ppp = ctx.pixels_per_point();
    line_pixels(ppp) / ppp
}

/// `r` on device pixels, `lines` one-point lines in from each side: where
/// to fill or draw inside a frame or bevel of that many lines, flush
/// against them at any scale (`r.shrink(1.0)` leaves a crack at 1.5×,
/// where a line is one pixel and a point a pixel and a half).
pub fn inset(ctx: &Context, r: Rect, lines: f32) -> Rect {
    let ppp = ctx.pixels_per_point();
    let s = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(pos2(s(r.min.x), s(r.min.y)), pos2(s(r.max.x), s(r.max.y))).shrink(lines * line_pixels(ppp) / ppp)
}

/// A one-point line's span across, in points on device pixels: from
/// `edge` (snapped) onwards, or back from it when the line ends there.
fn line_span(ppp: f32, edge: f32, ends_there: bool) -> (f32, f32) {
    let e = (edge * ppp).round();
    let w = line_pixels(ppp);
    if ends_there { ((e - w) / ppp, e / ppp) } else { (e / ppp, (e + w) / ppp) }
}

/// One-point horizontal line from x0 to x1 (exclusive) whose top is at y.
pub fn hline(p: &Painter, x0: f32, x1: f32, y: f32, color: Color32) {
    if x1 > x0 {
        let (t, b) = line_span(p.pixels_per_point(), y, false);
        p.rect_filled(Rect::from_min_max(pos2(x0, t), pos2(x1, b)), 0.0, color);
    }
}

/// One-point horizontal line whose bottom is at y: the last line of
/// something ending there (rather than `hline` at `bottom - 1.0`, a
/// pixel off where a fractional scale rounds the edge).
pub fn hline_above(p: &Painter, x0: f32, x1: f32, y: f32, color: Color32) {
    if x1 > x0 {
        let (t, b) = line_span(p.pixels_per_point(), y, true);
        p.rect_filled(Rect::from_min_max(pos2(x0, t), pos2(x1, b)), 0.0, color);
    }
}

/// One-point vertical line whose left edge is at x.
pub fn vline(p: &Painter, x: f32, y0: f32, y1: f32, color: Color32) {
    if y1 > y0 {
        let (l, r) = line_span(p.pixels_per_point(), x, false);
        p.rect_filled(Rect::from_min_max(pos2(l, y0), pos2(r, y1)), 0.0, color);
    }
}

/// One-point vertical line whose right edge is at x: the last column of
/// something ending there (as [`hline_above`]).
pub fn vline_left_of(p: &Painter, x: f32, y0: f32, y1: f32, color: Color32) {
    if y1 > y0 {
        let (l, r) = line_span(p.pixels_per_point(), x, true);
        p.rect_filled(Rect::from_min_max(pos2(l, y0), pos2(r, y1)), 0.0, color);
    }
}

/// `r` on device pixels, and a line's width there (points).
fn on_pixels(p: &Painter, r: Rect) -> (Rect, f32) {
    let ppp = p.pixels_per_point();
    let s = |v: f32| (v * ppp).round() / ppp;
    (Rect::from_min_max(pos2(s(r.min.x), s(r.min.y)), pos2(s(r.max.x), s(r.max.y))), line_pixels(ppp) / ppp)
}

/// One-point outline just inside `r`.
pub fn frame_rect(p: &Painter, r: Rect, color: Color32) {
    let (d, w) = on_pixels(p, r);
    frame_on_pixels(p, d, w, color);
}

fn frame_on_pixels(p: &Painter, d: Rect, w: f32, color: Color32) {
    let fill = |min: Pos2, max: Pos2| {
        if max.x > min.x && max.y > min.y {
            p.rect_filled(Rect::from_min_max(min, max), 0.0, color);
        }
    };
    fill(d.min, pos2(d.max.x, d.min.y + w));
    fill(pos2(d.min.x, d.max.y - w), d.max);
    fill(pos2(d.min.x, d.min.y + w), pos2(d.min.x + w, d.max.y - w));
    fill(pos2(d.max.x - w, d.min.y + w), pos2(d.max.x, d.max.y - w));
}

/// Two-tone bevel just inside `r`: `tl` on top/left, `br` on bottom/right.
pub fn bevel(p: &Painter, r: Rect, tl: Color32, br: Color32) {
    let (d, w) = on_pixels(p, r);
    bevel_on_pixels(p, d, w, tl, br);
}

/// [`bevel`] for `d` on device pixels, lines `w` wide (points).
fn bevel_on_pixels(p: &Painter, d: Rect, w: f32, tl: Color32, br: Color32) {
    let fill = |min: Pos2, max: Pos2, color| {
        if max.x > min.x && max.y > min.y {
            p.rect_filled(Rect::from_min_max(min, max), 0.0, color);
        }
    };
    fill(d.min, pos2(d.max.x - w, d.min.y + w), tl);
    fill(pos2(d.min.x, d.min.y + w), pos2(d.min.x + w, d.max.y - w), tl);
    fill(pos2(d.min.x, d.max.y - w), d.max, br);
    fill(pos2(d.max.x - w, d.min.y), pos2(d.max.x, d.max.y - w), br);
}

/// The BeOS "sunken" well used by text fields, lists and scroll views:
/// a dark/light outer bevel and a darker inner line on top/left.
pub fn sunken(p: &Painter, r: Rect, palette: &Palette, focused: bool) {
    let bg = palette.panel_background;
    let (d, w) = on_pixels(p, r);
    bevel_on_pixels(p, d, w, darken(bg, 1.147), c(palette.shine));
    // The inner lines a line in, on device pixels as the outer are.
    let inner = d.shrink(w);
    if focused {
        frame_on_pixels(p, inner, w, c(palette.keyboard_navigation));
    } else {
        bevel_on_pixels(p, inner, w, darken(bg, 1.55), darken(bg, 1.06));
    }
}

/// A raised panel edge (light top/left, dark bottom/right).
pub fn raised(p: &Painter, r: Rect, palette: &Palette) {
    bevel(p, r, c(palette.shine), darken(palette.panel_background, 1.3));
}

/// The colours of cards laid out on a document (a notification's in the
/// history): the `gutter` between them and the `card`, the lighter of
/// the two (the paper itself in light mode; in dark, a lift above it, as
/// a raised surface is lit), its hairline `edge` a step towards the
/// text, and `well`, a step further in for something set into a card (a
/// message's balloon). `well` and `dim` are the text colour, see-through,
/// so they sit as well on a selected card ([`selection_ink`]) as on a
/// plain one.
pub struct CardInk {
    pub gutter: Color32,
    pub card: Color32,
    pub edge: Color32,
    pub well: Color32,
    /// The card under the pointer.
    pub hover: Color32,
    /// Text on the card: the document's, and quieter (a header, labels).
    pub text: Color32,
    pub dim: Color32,
}

pub fn card_ink(palette: &Palette) -> CardInk {
    let (gutter, card) = card_paper(palette);
    let ink = palette.document_text;
    let dark = palette.mode == olo_theme::ThemeMode::Dark;
    let see = |a: f32| Color32::from_rgba_unmultiplied(ink[0], ink[1], ink[2], (a * 255.0).round() as u8);
    CardInk {
        gutter: c(gutter),
        card: c(card),
        edge: c(mix(card, ink, if dark { 0.09 } else { 0.13 })),
        well: see(if dark { 0.06 } else { 0.05 }),
        hover: c(mix(card, ink, 0.025)),
        text: c(ink),
        dim: see(0.52),
    }
}

/// The gutter between cards and a card, as [`card_ink`]'s (the gutter
/// as an `Rgba` for a `ScrollView`'s background).
pub fn card_paper(palette: &Palette) -> (Rgba, Rgba) {
    let paper = palette.document_background;
    let ink = palette.document_text;
    if palette.mode == olo_theme::ThemeMode::Dark { (paper, mix(paper, ink, 0.055)) } else { (mix(paper, ink, 0.045), paper) }
}

/// A card at `r` in [`card_ink`]'s colours: rounded by
/// `metrics::CARD_RADIUS`, edged with a hairline, resting on a soft
/// shadow; `hover` lifts it a touch towards the text. `selected` lays it
/// in [`selection_ink`]'s colours, as a list's chosen row is, and raises
/// it on a deeper shadow: picked up, not outlined.
pub fn card(p: &Painter, r: Rect, palette: &Palette, hover: bool, selected: bool) {
    let ink = card_ink(palette);
    let r = snap(p.ctx(), r);
    let radius = crate::metrics::CARD_RADIUS;
    let dark = palette.mode == olo_theme::ThemeMode::Dark;
    let shadow = if selected {
        Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(if dark { 110 } else { 40 }) }
    } else {
        Shadow { offset: [0, 1], blur: 4, spread: 0, color: Color32::from_black_alpha(if dark { 70 } else { 22 }) }
    };
    p.add(shadow.as_shape(r, radius));
    let (fill, edge) = if selected {
        let sel = selection_ink(palette);
        (sel.fill, sel.edge)
    } else {
        (if hover { ink.hover } else { ink.card }, ink.edge)
    };
    p.rect(r, radius, fill, Stroke::new(hairline(p.ctx()), edge), egui::StrokeKind::Inside);
}

/// How Olo shows that a thing is selected, wherever it is: a list's row,
/// an icon's name, a card, a picture or swatch in a picker, a part of a
/// map. Always the palette's list selection (BeOS's
/// `B_LIST_SELECTED_BACKGROUND_COLOR`) under or over the thing, never a
/// ring in the keyboard-navigation colour: that says where the keys go,
/// which is something else.
pub struct SelectionInk {
    /// The ground of what is selected.
    pub fill: Color32,
    /// A hairline round a selected thing that stands on its own (a card,
    /// a [`selection_plate`]), so its shape stays crisp: the fill, taken
    /// towards the highlight colour.
    pub edge: Color32,
    /// Text on `fill`.
    pub text: Color32,
    /// Quieter text on `fill` (a caption, a detail): `text`, see-through,
    /// so it reads on any selection colour where the panel's dim grey
    /// would sink into a grey one.
    pub dim: Color32,
}

pub fn selection_ink(palette: &Palette) -> SelectionInk {
    let fill = palette.list_selected_background;
    let [r, g, b, _] = palette.list_selected_item_text;
    SelectionInk {
        fill: c(fill),
        edge: c(palette.selection_edge()),
        text: c(palette.list_selected_item_text),
        dim: Color32::from_rgba_unmultiplied(r, g, b, 150),
    }
}

/// The plate a selected picture, swatch or preview sits on: a rounded
/// ground in [`selection_ink`]'s colours, `metrics::SELECTION_PAD` beyond
/// `r` all round. Paint it before the thing, as Tracker sets a selected
/// icon on its highlight.
pub fn selection_plate(p: &Painter, r: Rect, palette: &Palette) {
    let ink = selection_ink(palette);
    let r = snap(p.ctx(), r.expand(crate::metrics::SELECTION_PAD));
    p.rect(r, crate::metrics::CARD_RADIUS, ink.fill, Stroke::new(hairline(p.ctx()), ink.edge), egui::StrokeKind::Inside);
}

/// Selection over `r`, for a thing selected in place with nothing round
/// it to lay a plate in (a part of a bar, a cell flush with its
/// neighbours): [`selection_ink`]'s fill, see-through, and its hairline
/// just inside. Paint it after the thing's ground and before its text.
pub fn selection_wash(p: &Painter, r: Rect, palette: &Palette) {
    let ink = selection_ink(palette);
    p.rect(snap(p.ctx(), r), 0.0, ink.fill.gamma_multiply(0.7), Stroke::new(hairline(p.ctx()), ink.edge), egui::StrokeKind::Inside);
}

/// A short tag in a pill (a badge, a count, a group's name), its left
/// edge at `x` and centred on `cy`. Returns its rect.
pub fn pill(p: &Painter, x: f32, cy: f32, text: &str, font: FontId, fill: Color32, ink: Color32) -> Rect {
    let r = snap(p.ctx(), Rect::from_min_size(pos2(x, cy - pill_height(&font) / 2.0), vec2(pill_width(p.ctx(), text, font.clone()), pill_height(&font))));
    p.rect_filled(r, r.height() / 2.0, fill);
    label(p, r.center().x, r.center().y, egui::Align::Center, text, font, ink);
    r
}

/// Black or white, whichever reads on `fill` (text on a pill or tile in
/// an app's own colour).
pub fn ink_on(fill: Color32) -> Color32 {
    let l = 0.2126 * fill.r() as f32 + 0.7152 * fill.g() as f32 + 0.0722 * fill.b() as f32;
    if l > 150.0 { Color32::from_gray(20) } else { Color32::WHITE }
}

/// A rounded tile in `fill` with `letter` on it, filling `r`: the icon of
/// something that has a colour but no icon of its own (a website, a
/// contact), like a monogram. Lit along its top, edged a shade darker.
pub fn monogram(p: &Painter, r: Rect, letter: &str, fill: Color32) {
    let r = snap(p.ctx(), r);
    let radius = (r.height() * 0.22).round();
    let rgba = [fill.r(), fill.g(), fill.b(), 255];
    p.rect_filled(r, radius, fill);
    p.rect_stroke(r, radius, egui::Stroke::new(hairline(p.ctx()), darken(rgba, 1.25)), egui::StrokeKind::Inside);
    hline(p, r.left() + radius, r.right() - radius, r.top() + 1.0, lighten(rgba, 0.7));
    let font = FontId::new((r.height() * 0.62).round(), bold());
    label(p, r.center().x, r.center().y, egui::Align::Center, letter, font, ink_on(fill));
}

/// How wide a [`pill`] of `text` is.
pub fn pill_width(ctx: &Context, text: &str, font: FontId) -> f32 {
    let h = pill_height(&font);
    (label_width(ctx, text, font) + h * 0.9).max(h).round()
}

/// How tall a [`pill`] in `font` is.
pub fn pill_height(font: &FontId) -> f32 {
    (font.size + 5.0).round()
}

/// Pixel-snapped round to the device grid.
pub fn snap(ctx: &Context, r: Rect) -> Rect {
    let ppp = ctx.pixels_per_point();
    let s = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(pos2(s(r.min.x), s(r.min.y)), pos2(s(r.max.x), s(r.max.y)))
}

/// A rect of `size` centred on `center` as nearly as device pixels
/// allow, its size whole device pixels too: one size wherever it goes (a
/// slider's thumb as it moves), where `snap` can make it a pixel wider in
/// one place than another.
pub fn snap_centred(ctx: &Context, center: Pos2, size: Vec2) -> Rect {
    let ppp = ctx.pixels_per_point();
    let (w, h) = ((size.x * ppp).round(), (size.y * ppp).round());
    let min = pos2((center.x * ppp - w / 2.0).round(), (center.y * ppp - h / 2.0).round());
    Rect::from_min_size(min / ppp, vec2(w, h) / ppp)
}

/// A check mark inside `r`, as one joined stroke.
pub fn check_mark(p: &Painter, r: Rect, color: Color32) {
    let pts = vec![
        pos2(r.left(), r.top() + r.height() * 0.5),
        pos2(r.left() + r.width() * 0.38, r.bottom()),
        pos2(r.right(), r.top()),
    ];
    p.add(egui::Shape::line(pts, Stroke::new(1.6, color)));
}

/// A hollow diamond in `r` (a minimized window in a menu of windows, where
/// a check mark would go).
pub fn diamond(p: &Painter, r: Rect, color: Color32) {
    let m = snap(p.ctx(), Rect::from_center_size(r.center(), vec2(8.0, 8.0))).center();
    let h = r.width().min(r.height()) / 2.0 - 0.5;
    let pts = vec![pos2(m.x, m.y - h), pos2(m.x + h, m.y), pos2(m.x, m.y + h), pos2(m.x - h, m.y)];
    p.add(egui::Shape::closed_line(pts, Stroke::new(1.2, color)));
}

/// A small stop sign centred in `r`, for text that reports a failure (an
/// error's title, a failed row) where an alert's stop icon would be too
/// big to read: an octagon in the failure colour, edged a shade darker,
/// with an exclamation mark.
pub fn stop_mark(p: &Painter, r: Rect, palette: &Palette) {
    let side = r.width().min(r.height());
    let r = snap(p.ctx(), Rect::from_center_size(r.center(), vec2(side, side)));
    let (m, h) = (r.center(), side / 2.0 - 0.5);
    let corner = h / (std::f32::consts::PI / 8.0).cos();
    let pts: Vec<Pos2> = (0..8)
        .map(|k| {
            let a = std::f32::consts::PI / 8.0 + k as f32 * std::f32::consts::PI / 4.0;
            pos2(m.x + corner * a.cos(), m.y + corner * a.sin())
        })
        .collect();
    let fill = c(palette.failure);
    p.add(egui::Shape::convex_polygon(pts, fill, Stroke::new(1.0, darken(palette.failure, 1.3))));
    let ink = ink_on(fill);
    let w = (side * 0.15).max(1.5);
    p.rect_filled(Rect::from_min_max(pos2(m.x - w / 2.0, m.y - h * 0.6), pos2(m.x + w / 2.0, m.y + h * 0.12)), w / 2.0, ink);
    p.circle_filled(pos2(m.x, m.y + h * 0.45), w * 0.6, ink);
}

/// BOutlineListView's latch: a small solid triangle centred in `r`,
/// pointing right while its group is folded and down while open.
pub fn latch(p: &Painter, r: Rect, open: bool, color: Color32) {
    let m = snap(p.ctx(), Rect::from_center_size(r.center(), vec2(8.0, 8.0))).center();
    let pts = if open {
        vec![pos2(m.x - 4.0, m.y - 2.0), pos2(m.x + 4.0, m.y - 2.0), pos2(m.x, m.y + 2.5)]
    } else {
        vec![pos2(m.x - 2.0, m.y - 4.0), pos2(m.x + 2.5, m.y), pos2(m.x - 2.0, m.y + 4.0)]
    };
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// Right-aligned keys with any modifiers, as on a Mac ("⌃⌥⇧⌘←"), each
/// symbol on the text's baseline.
pub fn key_combo(p: &Painter, right_center: Pos2, keys: &str, color: Color32) {
    let parts = combo_parts(keys);
    let font = FontId::proportional(PLAIN_SIZE);
    let mut x = right_center.x;
    for (i, part) in parts.iter().enumerate().rev() {
        let g = p.layout_no_wrap(part.clone(), font.clone(), color);
        x -= g.size().x;
        let pos = text_origin(p.ctx(), &g, x, right_center.y, PLAIN_SIZE);
        p.galley(pos, g, color);
        if i > 0 {
            x += SHORTCUT_KERN;
        }
    }
}

/// ⌘ and ⇧ sit this much closer to what follows than their advance width.
const SHORTCUT_KERN: f32 = 1.0;

/// The keys split so each symbol (⌘ ⇧ ↑ ⌫ …) is laid out on its own, to
/// be set [`SHORTCUT_KERN`] closer to what follows.
fn combo_parts(label: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut run = String::new();
    for ch in label.chars() {
        if ch.is_ascii() {
            run.push(ch);
        } else {
            if !run.is_empty() {
                parts.push(std::mem::take(&mut run));
            }
            parts.push(ch.to_string());
        }
    }
    if !run.is_empty() {
        parts.push(run);
    }
    parts
}

/// Width of keys as [`key_combo`] draws them.
pub fn key_combo_width(ctx: &Context, keys: &str) -> f32 {
    let font = FontId::proportional(PLAIN_SIZE);
    let parts = combo_parts(keys);
    ctx.fonts_mut(|f| {
        parts.iter().map(|t| f.layout_no_wrap(t.clone(), font.clone(), Color32::BLACK).size().x).sum::<f32>()
            - SHORTCUT_KERN * (parts.len() - 1) as f32
    })
}

/// Contrast-aware tint: bevels need a stronger push on dark panels to
/// read at all, so darkening is exaggerated in dark mode.
pub fn shade(palette: &Palette, rgba: Rgba, tint: f32) -> Color32 {
    c(palette.shade(rgba, tint))
}

/// A vertical gradient fill.
pub fn vgradient(p: &Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.left_top(), top);
    mesh.colored_vertex(r.right_top(), top);
    mesh.colored_vertex(r.left_bottom(), bottom);
    mesh.colored_vertex(r.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    p.add(mesh);
}

// ---- text placement -------------------------------------------------------

/// Cap height as a fraction of the font size (Helvetica / Nimbus Sans).
pub(crate) const CAP_HEIGHT: f32 = 0.718;

/// Distance from a galley's top to its first baseline: the primary
/// face's, even when the line starts with a glyph from a fallback face
/// (egui gives that glyph its own face's centred line).
pub fn baseline_offset(galley: &egui::Galley) -> f32 {
    galley
        .rows
        .first()
        .and_then(|r| {
            r.row.glyphs.first().map(|g| {
                r.pos.y + g.pos.y + g.font_ascent
                    - g.font_face_ascent
                    - 0.5 * (g.font_height - g.font_face_height)
            })
        })
        .unwrap_or(galley.size().y * 0.8)
}

/// Distance from a galley's top to the middle of its first line's
/// capitals (to set a mark beside the text, level with it).
pub fn cap_middle(galley: &egui::Galley, font_size: f32) -> f32 {
    baseline_offset(galley) - font_size * CAP_HEIGHT / 2.0
}

/// Where to put `galley` so its capitals are optically centred on
/// `center_y` (a box centre would sit text high: descender space counts),
/// snapped to device pixels.
pub fn text_origin(ctx: &Context, galley: &egui::Galley, x: f32, center_y: f32, font_size: f32) -> Pos2 {
    let ppp = ctx.pixels_per_point();
    let baseline = center_y + font_size * CAP_HEIGHT / 2.0;
    let y = baseline - baseline_offset(galley);
    pos2((x * ppp).round() / ppp, (y * ppp).round() / ppp)
}

/// Draw one line of text with its capitals centred on `center_y`. `align`
/// says which edge of the text `x` is (left, centre or right). Returns the
/// text's rect.
pub fn label(
    p: &Painter,
    x: f32,
    center_y: f32,
    align: egui::Align,
    text: impl Into<String>,
    font: FontId,
    color: Color32,
) -> Rect {
    label_with_baseline(p, x, center_y, align, text, font, color).0
}

/// [`label`], and the baseline it was drawn on: for text of another size
/// to go beside it on that line ([`label_on_baseline`]).
pub fn label_with_baseline(
    p: &Painter,
    x: f32,
    center_y: f32,
    align: egui::Align,
    text: impl Into<String>,
    font: FontId,
    color: Color32,
) -> (Rect, f32) {
    let baseline = center_y + font.size * CAP_HEIGHT / 2.0;
    draw_on_baseline(p, x, baseline, align, text.into(), font, color)
}

/// One line of text on `baseline`, as near as device pixels allow: beside
/// a label of another size, on the baseline it was drawn on
/// ([`label_with_baseline`]). Within half a device pixel of it, where
/// centring each on one middle can leave them a pixel apart. Returns the
/// text's rect.
pub fn label_on_baseline(
    p: &Painter,
    x: f32,
    baseline: f32,
    align: egui::Align,
    text: impl Into<String>,
    font: FontId,
    color: Color32,
) -> Rect {
    draw_on_baseline(p, x, baseline, align, text.into(), font, color).0
}

fn draw_on_baseline(p: &Painter, x: f32, baseline: f32, align: egui::Align, text: String, font: FontId, color: Color32) -> (Rect, f32) {
    let ppp = p.ctx().pixels_per_point();
    let galley = p.layout_no_wrap(text, font, color);
    let x = match align {
        egui::Align::Min => x,
        egui::Align::Center => x - galley.size().x / 2.0,
        egui::Align::Max => x - galley.size().x,
    };
    let offset = baseline_offset(&galley);
    let pos = pos2((x * ppp).round() / ppp, ((baseline - offset) * ppp).round() / ppp);
    let r = Rect::from_min_size(pos, galley.size());
    describe_text(p.ctx(), egui::accesskit::Role::Label, galley.text(), r, None);
    p.galley(pos, galley, color);
    (r, pos.y + offset)
}

/// [`label`] in at most `max_width`: text that doesn't fit is cut short
/// with "…" (never clipped mid-letter); agents read the whole of it.
#[allow(clippy::too_many_arguments)]
pub fn label_fit(
    p: &Painter,
    x: f32,
    center_y: f32,
    align: egui::Align,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Rect {
    let size = font.size;
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    let galley = p.layout_job(job);
    let x = match align {
        egui::Align::Min => x,
        egui::Align::Center => x - galley.size().x / 2.0,
        egui::Align::Max => x - galley.size().x,
    };
    let pos = text_origin(p.ctx(), &galley, x, center_y, size);
    let r = Rect::from_min_size(pos, galley.size());
    describe_text(p.ctx(), egui::accesskit::Role::Label, text, r, None);
    p.galley(pos, galley, color);
    r
}

/// Text drawn with the painter, for the AccessKit tree agents read
/// (`commands::tree`); nothing unless one is being made.
pub fn describe_text(ctx: &Context, role: egui::accesskit::Role, text: &str, r: Rect, selected: Option<bool>) {
    // Empty, or scrolled out of sight: nothing to read.
    if text.trim().is_empty() || !r.is_positive() {
        return;
    }
    let id = egui::Id::unique(("olo-text", text, r.min.x as i32, r.min.y as i32));
    ctx.accesskit_node_builder(id, |n| {
        n.set_role(role);
        n.set_label(text);
        n.set_bounds(egui::accesskit::Rect { x0: r.min.x as f64, y0: r.min.y as f64, x1: r.max.x as f64, y1: r.max.y as f64 });
        if let Some(on) = selected {
            n.set_selected(on);
        }
    });
}

/// A painted item that takes clicks (an icon, a column's row), for the
/// AccessKit tree: agents and keyboard hints pick it by its middle.
pub fn describe_item(ctx: &Context, text: &str, r: Rect, selected: bool) {
    if text.trim().is_empty() || !r.is_positive() {
        return;
    }
    let id = egui::Id::unique(("olo-item", text, r.min.x as i32, r.min.y as i32));
    ctx.accesskit_node_builder(id, |n| {
        n.set_role(egui::accesskit::Role::ListItem);
        n.set_label(text);
        n.set_bounds(egui::accesskit::Rect { x0: r.min.x as f64, y0: r.min.y as f64, x1: r.max.x as f64, y1: r.max.y as f64 });
        n.add_action(egui::accesskit::Action::Click);
        n.set_selected(selected);
    });
}

/// A rubber-band selection rectangle (Tracker, desktop, any icon view):
/// a one-pixel navigation-blue frame over a faint tint of the same colour.
pub fn selection_band(p: &Painter, r: Rect, palette: &Palette) {
    let r = snap(p.ctx(), r);
    let [red, green, blue, _] = palette.keyboard_navigation;
    p.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(red, green, blue, 40));
    frame_rect(p, r, c(palette.keyboard_navigation));
}

/// Background rect for highlighted text (a selected icon label): `pad`
/// beyond the text's sides, and vertically balanced on what the eye sees —
/// from the cap height of the first row to the baseline of the last —
/// rather than on the line box, which would leave the text sitting high.
pub fn text_highlight(galley: &egui::Galley, origin: Pos2, font_size: f32, pad: egui::Vec2) -> Rect {
    let first = baseline_offset(galley);
    let last_row_top = galley.rows.last().map_or(0.0, |r| r.pos.y);
    let top = origin.y + first - font_size * CAP_HEIGHT - pad.y;
    let bottom = origin.y + last_row_top + first + pad.y;
    let r = galley.rect.translate(origin.to_vec2());
    Rect::from_min_max(pos2(r.left() - pad.x, top), pos2(r.right() + pad.x, bottom))
}

/// Token classes for syntax colouring (Quick Look, StyledEdit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    Plain,
    Comment,
    String,
    Keyword,
    Number,
    Type,
    Function,
    Constant,
    Tag,
    Attribute,
    Heading,
    Link,
}

/// The colour of a token class on document backgrounds: the look's hues
/// (`Palette::hue_text`, so each clears 4.5:1 against
/// `document_background`), comments a step toward the background.
pub fn syntax_color(p: &Palette, s: Syntax) -> Color32 {
    use olo_theme::Hue;
    let hue = match s {
        Syntax::Plain => return c(p.document_text),
        Syntax::Comment => return c(olo_theme::readable(mix(p.document_text, p.document_background, 0.45), p.document_background, 4.5)),
        Syntax::String => Hue::Green,
        Syntax::Keyword => Hue::Purple,
        Syntax::Number | Syntax::Attribute => Hue::Orange,
        Syntax::Type => Hue::Teal,
        Syntax::Function | Syntax::Tag | Syntax::Link | Syntax::Heading => Hue::Blue,
        Syntax::Constant => Hue::Red,
    };
    c(p.hue_text(hue))
}

/// The width of `text` set in `font` (for sizing buttons and tabs).
pub fn label_width(ctx: &Context, text: &str, font: FontId) -> f32 {
    ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_reads_on_its_colour() {
        assert_eq!(ink_on(Color32::from_rgb(0x25, 0xd3, 0x66)), Color32::from_gray(20));
        assert_eq!(ink_on(Color32::from_rgb(0x61, 0x1f, 0x69)), Color32::WHITE);
    }

    /// "▸ Developer ▸ Integrate", at 2x.
    fn menu_path(text: &str) -> Option<Arc<egui::Galley>> {
        let ctx = Context::default();
        ctx.set_fonts(all_font_definitions());
        let mut raw = egui::RawInput::default();
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(2.0);
        let mut galley = None;
        ctx.run_ui(raw, |ui| {
            let font = FontId::proportional(PLAIN_SIZE);
            galley = Some(ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, Color32::BLACK)));
        })
        .drop_without_applying_deltas();
        // Without the system fonts (DejaVu Sans) there is nothing to check.
        font_definitions().font_data.contains_key("fallback@ui").then_some(galley?)
    }

    #[test]
    fn fallback_glyphs_sit_on_the_baseline() {
        let Some(galley) = menu_path("Settings ▸ Developer") else { return };
        let glyphs = &galley.rows[0].row.glyphs;
        let letter = glyphs.iter().find(|g| g.chr == 'S').unwrap();
        let arrow = glyphs.iter().find(|g| g.chr == '▸').unwrap();
        // DejaVu's ▸ stands on the baseline, as the letters do.
        let bottom = arrow.pos.y + arrow.uv_rect.offset.y + arrow.uv_rect.size.y;
        assert!((bottom - letter.pos.y).abs() <= 0.5, "▸ ends at {bottom}, the baseline is {}", letter.pos.y);
    }

    #[test]
    fn baseline_is_the_letters_when_a_symbol_leads() {
        let Some(led) = menu_path("▸ Developer") else { return };
        let Some(plain) = menu_path("Developer") else { return };
        assert!((baseline_offset(&led) - baseline_offset(&plain)).abs() <= 0.5);
    }

    #[test]
    fn plain_text_height_is_what_egui_lays_out() {
        let texts = [
            "This computer is locked.",
            "An application wants to create a new keyring called “Default keyring”. Choose the password you want to use for it.",
            "Google Chrome isn't on this computer yet.\n\nOlo can download it from Google and keep it in your home folder, \
             updating it when there is a newer one.",
            "Authentication is needed to run `/usr/bin/true' as the super user",
            "A_path_with_no_spaces_at_all_that_must_break_somewhere_in_the_middle_of_itself",
        ];
        let ctx = Context::default();
        ctx.set_fonts(all_font_definitions());
        for scale in [1.0, 2.0] {
            let mut raw = egui::RawInput::default();
            raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(scale);
            ctx.run_ui(raw, |ui| {
                for text in texts {
                    for width in [180.0, 292.0, 350.0] {
                        let galley = ui.fonts_mut(|f| f.layout(text.to_owned(), FontId::proportional(PLAIN_SIZE), Color32::BLACK, width));
                        let (laid, guessed) = (galley.size().y, plain_text_height(text, width));
                        let line = galley.rows[0].row.size.y.max(1.0);
                        // Never short (the text would be cut off); at most a line over.
                        assert!(guessed + 0.5 >= laid && guessed <= laid + line + 0.5, "{text:?} at {width}, {scale}x: {guessed} for {laid}");
                    }
                }
            })
            .drop_without_applying_deltas();
        }
    }
}

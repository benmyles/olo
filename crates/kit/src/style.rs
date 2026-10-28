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
    Shadow, Stroke, Visuals, pos2, vec2,
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
    ctx.data(|d| d.get_temp::<Arc<Palette>>(egui::Id::NULL))
        .unwrap_or_else(|| Arc::new(Palette::light()))
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
            fd.tweak.hinting = Some(true);
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

/// One-pixel horizontal line from x0 to x1 (exclusive) whose top is at y.
pub fn hline(p: &Painter, x0: f32, x1: f32, y: f32, color: Color32) {
    if x1 > x0 {
        p.rect_filled(Rect::from_min_max(pos2(x0, y), pos2(x1, y + 1.0)), 0.0, color);
    }
}

/// One-pixel vertical line whose left edge is at x.
pub fn vline(p: &Painter, x: f32, y0: f32, y1: f32, color: Color32) {
    if y1 > y0 {
        p.rect_filled(Rect::from_min_max(pos2(x, y0), pos2(x + 1.0, y1)), 0.0, color);
    }
}

/// One-pixel outline just inside `r`.
pub fn frame_rect(p: &Painter, r: Rect, color: Color32) {
    hline(p, r.left(), r.right(), r.top(), color);
    hline(p, r.left(), r.right(), r.bottom() - 1.0, color);
    vline(p, r.left(), r.top() + 1.0, r.bottom() - 1.0, color);
    vline(p, r.right() - 1.0, r.top() + 1.0, r.bottom() - 1.0, color);
}

/// Two-tone bevel just inside `r`: `tl` on top/left, `br` on bottom/right.
pub fn bevel(p: &Painter, r: Rect, tl: Color32, br: Color32) {
    hline(p, r.left(), r.right() - 1.0, r.top(), tl);
    vline(p, r.left(), r.top() + 1.0, r.bottom() - 1.0, tl);
    hline(p, r.left(), r.right(), r.bottom() - 1.0, br);
    vline(p, r.right() - 1.0, r.top(), r.bottom() - 1.0, br);
}

/// The BeOS "sunken" well used by text fields, lists and scroll views:
/// a dark/light outer bevel and a darker inner line on top/left.
pub fn sunken(p: &Painter, r: Rect, palette: &Palette, focused: bool) {
    let bg = palette.panel_background;
    bevel(p, r, darken(bg, 1.147), c(palette.shine));
    let inner = r.shrink(1.0);
    if focused {
        frame_rect(p, inner, c(palette.keyboard_navigation));
    } else {
        bevel(p, inner, darken(bg, 1.55), darken(bg, 1.06));
    }
}

/// A raised panel edge (light top/left, dark bottom/right).
pub fn raised(p: &Painter, r: Rect, palette: &Palette) {
    bevel(p, r, c(palette.shine), darken(palette.panel_background, 1.3));
}

/// Pixel-snapped round to the device grid.
pub fn snap(ctx: &Context, r: Rect) -> Rect {
    let ppp = ctx.pixels_per_point();
    let s = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(pos2(s(r.min.x), s(r.min.y)), pos2(s(r.max.x), s(r.max.y)))
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

/// Right-aligned shortcut label: the ⌘ glyph set tight against the key,
/// both on one baseline.
pub fn shortcut(p: &Painter, right_center: Pos2, label: &str, color: Color32) {
    key_combo(p, right_center, &format!("⌘{label}"), color);
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

/// The keys split so each symbol (⌘ ⇧ ↑ ⌫ …, which come from a fallback
/// font) is laid out on its own: in one galley with the letters they
/// would land on a different baseline.
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

/// Width of a shortcut label as [`shortcut`] draws it.
pub fn shortcut_width(ctx: &Context, label: &str) -> f32 {
    key_combo_width(ctx, &format!("⌘{label}"))
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
    let t = if palette.mode == olo_theme::ThemeMode::Dark && tint > 1.0 {
        1.0 + (tint - 1.0) * 1.9
    } else {
        tint
    };
    c(olo_theme::tint(rgba, t))
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
const CAP_HEIGHT: f32 = 0.718;

/// Distance from a galley's top to its first baseline.
pub fn baseline_offset(galley: &egui::Galley) -> f32 {
    galley
        .rows
        .first()
        .and_then(|r| r.row.glyphs.first().map(|g| r.pos.y + g.pos.y))
        .unwrap_or(galley.size().y * 0.8)
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
    let size = font.size;
    let galley = p.layout_no_wrap(text.into(), font, color);
    let x = match align {
        egui::Align::Min => x,
        egui::Align::Center => x - galley.size().x / 2.0,
        egui::Align::Max => x - galley.size().x,
    };
    let pos = text_origin(p.ctx(), &galley, x, center_y, size);
    let r = Rect::from_min_size(pos, galley.size());
    describe_text(p.ctx(), egui::accesskit::Role::Label, galley.text(), r, None);
    p.galley(pos, galley, color);
    r
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

/// The colour of a token class on document backgrounds. The same hues as
/// Terminal's Olo schemes, so code looks alike everywhere; each clears
/// 4.5:1 against `document_background` in its mode.
pub fn syntax_color(p: &Palette, s: Syntax) -> Color32 {
    let dark = p.mode == olo_theme::ThemeMode::Dark;
    let hex = |h: u32| Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8);
    match (s, dark) {
        (Syntax::Plain, _) => c(p.document_text),
        (Syntax::Comment, false) => hex(0x6a6e75),
        (Syntax::Comment, true) => hex(0x8a909a),
        (Syntax::String, false) => hex(0x2b7a38),
        (Syntax::String, true) => hex(0x8fcf7d),
        (Syntax::Keyword, false) => hex(0x9333ad),
        (Syntax::Keyword, true) => hex(0xc98ae6),
        (Syntax::Number | Syntax::Attribute, false) => hex(0x9a5a00),
        (Syntax::Number | Syntax::Attribute, true) => hex(0xe8c17a),
        (Syntax::Type, false) => hex(0x0b7380),
        (Syntax::Type, true) => hex(0x5cc4cf),
        (Syntax::Function | Syntax::Tag | Syntax::Link | Syntax::Heading, false) => hex(0x2a62c9),
        (Syntax::Function | Syntax::Tag | Syntax::Link | Syntax::Heading, true) => hex(0x7cb4ff),
        (Syntax::Constant, false) => hex(0xb92c45),
        (Syntax::Constant, true) => hex(0xec6a78),
    }
}

/// The width of `text` set in `font` (for sizing buttons and tabs).
pub fn label_width(ctx: &Context, text: &str, font: FontId) -> f32 {
    ctx.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x)
}

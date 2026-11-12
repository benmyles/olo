//! Olo's text, the same as the egui kit's: the faces Olo ships
//! (data/fonts: Noto Sans, Noto Sans Mono, their symbols) before the
//! system's copies of them, shaped by harfrust, their outlines hinted by
//! skrifa (vertically only, "light", as FreeType's autohinter, GTK and
//! Chrome do it: Noto ships unhinted) and filled to each pixel's exact
//! coverage (`fill`; colour glyphs drawn by vello_cpu). Variable
//! faces give each weight from their axis. A character a face lacks comes
//! from the kit's chain of others (Olo's symbol faces, the other of Noto
//! Sans and Noto Sans Mono, DejaVu Sans, CJK, emoji), every glyph on the
//! line's one baseline. (GPUI drew with cosmic-text and swash: no
//! autohinter, so Noto was unhinted, and no weights from variable faces.)

use crate::{
    Bounds, DevicePixels, Font, FontId, FontMetrics, FontRun, FontStyle, GlyphId, LineLayout, Pixels, PlatformTextSystem, RenderGlyphParams, SUBPIXEL_VARIANTS_X, ShapedGlyph, ShapedRun, Size,
    SubpixelLayout, TextRendering, point, px, size,
};
use anyhow::{Context as _, Result, anyhow};
use collections::HashMap;
use parking_lot::RwLock;
use skrifa::{
    FontRef, MetadataProvider,
    instance::{Location, Size as Ppem},
    metrics::GlyphMetrics,
    outline::{DrawSettings, HintingInstance, OutlinePen, SmoothMode, Target},
    raw::TableProvider,
};
use std::{borrow::Cow, sync::Arc};
use vello_cpu::kurbo;

type Data = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// One face in a font file.
struct Source {
    data: Data,
    index: u32,
    family: String,
    italic: bool,
    /// A static face's weight; a variable one's default.
    weight: f32,
    /// The `wght` axis's range, if it has one.
    wght: Option<(f32, f32)>,
    /// Colour glyphs (COLR, CBDT, sbix): emoji.
    color: bool,
    shaper: harfrust::ShaperData,
}

impl Source {
    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index((*self.data).as_ref(), self.index).ok()
    }
}

/// A face at one weight, with its OpenType features: what a [`FontId`]
/// names.
struct Instance {
    source: usize,
    weight: f32,
    location: Location,
    shaper_instance: Option<harfrust::ShaperInstance>,
    features: Vec<harfrust::Feature>,
    units_per_em: f32,
    metrics: FontMetrics,
    /// The faces after it for characters it lacks.
    fallbacks: Option<Arc<[FontId]>>,
}

pub(crate) struct OloTextSystem(RwLock<State>);

struct State {
    /// Olo's own faces, and any an app adds.
    own: fontdb::Database,
    /// The system's, loaded the first time they're needed.
    system: Option<fontdb::Database>,
    sources: Vec<Source>,
    source_ids: HashMap<(bool, fontdb::ID), usize>,
    instances: Vec<Instance>,
    instance_ids: HashMap<(usize, u32, Vec<(String, u32)>), FontId>,
    font_ids: HashMap<(String, u32, bool, Vec<(String, u32)>), FontId>,
    /// The face found for a character none of a chain has, if any.
    last_resort: HashMap<char, Option<usize>>,
    hinting: HashMap<(usize, u32), HintingInstance>,
    /// Glyphs drawn for their bounds, until their pixels are asked for;
    /// colour ones (emoji: slow to draw, ~60 µs) kept for every window
    /// after, up to `KEEP` bytes of them (`kept`).
    drawn: HashMap<RenderGlyphParams, (Bounds<DevicePixels>, Vec<u8>)>,
    kept: usize,
    buffer: Option<harfrust::UnicodeBuffer>,
    /// Each face's shape plans, by the text's direction, script and
    /// language: harfrust's `shape` made one for every run it shaped, most
    /// of the time a row of a log took (a screen of new rows, 20–30 ms).
    plans: HashMap<(usize, harfrust::Direction, harfrust::Script, Option<harfrust::Language>), harfrust::ShapePlan>,
    /// Words of plain ASCII text as each face shaped them, in its units,
    /// by a hash of the face and the word (the word kept to check): a word
    /// is shaped once, as browsers do (harfrust takes ~220 ns a character
    /// on Noto Sans; a screen of log rows, mostly the same words, 7–12
    /// ms). Emptied when it grows past `WORDS`.
    words: HashMap<u64, (Box<str>, Vec<WordGlyph>)>,
    /// Fills glyphs' outlines: one, kept, as every glyph is small.
    raster: ab_glyph_rasterizer::Rasterizer,
}

/// A shaped glyph in its face's units (`State::words`).
struct WordGlyph {
    id: u32,
    cluster: u32,
    x_advance: i32,
    x_offset: i32,
    y_offset: i32,
}

/// How many shaped words `State::words` keeps before starting afresh.
const WORDS: usize = 40_000;

/// Olo's families, for the names GPUI had of its own and the generic ones.
fn families(name: &str) -> Vec<&str> {
    match name {
        ".SystemUIFont" | "Zed Plex Sans" | "IBM Plex Sans" | "system-ui" | "sans-serif" => olo_theme::fonts::UI.to_vec(),
        ".ZedMono" | "Zed Plex Mono" | "Zed Mono" | "Lilex" | "monospace" => olo_theme::fonts::MONO.to_vec(),
        name => {
            let mut v = vec![name];
            v.extend_from_slice(olo_theme::fonts::UI);
            v
        }
    }
}

const WGHT: skrifa::Tag = skrifa::Tag::new(b"wght");

impl OloTextSystem {
    pub(crate) fn new() -> Self {
        let mut own = fontdb::Database::new();
        if let Some(dir) = olo_xdg::fonts_dir() {
            own.load_fonts_dir(dir);
        }
        Self(RwLock::new(State {
            own,
            system: None,
            sources: Vec::new(),
            source_ids: HashMap::default(),
            instances: Vec::new(),
            instance_ids: HashMap::default(),
            font_ids: HashMap::default(),
            last_resort: HashMap::default(),
            hinting: HashMap::default(),
            drawn: HashMap::default(),
            kept: 0,
            buffer: Some(harfrust::UnicodeBuffer::new()),
            plans: HashMap::default(),
            words: HashMap::default(),
            raster: ab_glyph_rasterizer::Rasterizer::new(0, 0),
        }))
    }
}

impl Default for OloTextSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformTextSystem for OloTextSystem {
    fn add_fonts(&self, fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
        let mut state = self.0.write();
        for bytes in fonts {
            state.own.load_font_data(bytes.into_owned());
        }
        state.font_ids.clear();
        Ok(())
    }

    fn preload_system_fonts(&self) {
        if self.0.read().system.is_some() {
            return;
        }
        // Read without the lock held: text goes on being laid out (in the
        // faces Olo ships) meanwhile.
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut state = self.0.write();
        if state.system.is_none() {
            state.system = Some(db);
        }
    }

    fn all_font_names(&self) -> Vec<String> {
        self.preload_system_fonts();
        let mut state = self.0.write();
        state.system();
        let mut names: Vec<String> = state.own.faces().chain(state.system.iter().flat_map(|db| db.faces())).filter_map(|face| face.families.first().map(|f| f.0.clone())).collect();
        names.sort();
        names.dedup();
        names
    }

    fn font_id(&self, font: &Font) -> Result<FontId> {
        let features: Vec<(String, u32)> = font.features.0.iter().map(|(name, value)| (name.clone(), *value)).collect();
        let italic = font.style != FontStyle::Normal;
        let key = (font.family.to_string(), font.weight.0.to_bits(), italic, features);
        if let Some(id) = self.0.read().font_ids.get(&key) {
            return Ok(*id);
        }
        let mut state = self.0.write();
        let source = state.find(&families(&font.family), font.weight.0, italic).with_context(|| format!("no font for {}", font.family))?;
        let id = state.instance(source, font.weight.0, &key.3)?;
        state.font_ids.insert(key, id);
        Ok(id)
    }

    fn font_metrics(&self, font_id: FontId) -> FontMetrics {
        self.0.read().instances[font_id.0].metrics
    }

    fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
        let state = self.0.read();
        let instance = &state.instances[font_id.0];
        let font = state.sources[instance.source].font().context("font")?;
        let metrics = GlyphMetrics::new(&font, Ppem::unscaled(), &instance.location);
        let gid = skrifa::GlyphId::new(glyph_id.0);
        Ok(match metrics.bounds(gid) {
            Some(b) => Bounds { origin: point(b.x_min, b.y_min), size: size(b.x_max - b.x_min, b.y_max - b.y_min) },
            None => Bounds { origin: point(0.0, 0.0), size: size(metrics.advance_width(gid).unwrap_or(0.0), 0.0) },
        })
    }

    fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
        let state = self.0.read();
        let instance = &state.instances[font_id.0];
        let font = state.sources[instance.source].font().context("font")?;
        let metrics = GlyphMetrics::new(&font, Ppem::unscaled(), &instance.location);
        Ok(size(metrics.advance_width(skrifa::GlyphId::new(glyph_id.0)).unwrap_or(0.0), 0.0))
    }

    fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
        let state = self.0.read();
        let source = &state.sources[state.instances[font_id.0].source];
        let gid = source.font()?.charmap().map(ch)?;
        Some(GlyphId(gid.to_u32()))
    }

    fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
        let mut state = self.0.write();
        let Some((bounds, pixels)) = state.draw(params) else {
            return Ok(Bounds::default());
        };
        state.keep(params.clone(), (bounds, pixels));
        Ok(bounds)
    }

    fn rasterize_glyph(&self, params: &RenderGlyphParams, raster_bounds: Bounds<DevicePixels>) -> Result<(Size<DevicePixels>, Vec<u8>)> {
        if raster_bounds.size.width.0 == 0 || raster_bounds.size.height.0 == 0 {
            anyhow::bail!("glyph bounds are empty");
        }
        let mut state = self.0.write();
        // A colour glyph stays for the next window; another goes to the
        // atlas that asked.
        let drawn = if params.is_emoji { state.drawn.get(params).cloned() } else { state.drawn.remove(params) };
        let drawn = match drawn {
            Some(drawn) => drawn,
            None => {
                let drawn = state.draw(params).ok_or_else(|| anyhow!("no glyph"))?;
                if params.is_emoji {
                    state.keep(params.clone(), drawn.clone());
                }
                drawn
            }
        };
        Ok((drawn.0.size, drawn.1))
    }

    /// Olo: draws `params` ahead of need, each the first time, without
    /// holding the text system while it draws (colour glyphs as outline
    /// ones are in `draw_ahead`).
    fn draw_glyphs_ahead(&self, params: &[RenderGlyphParams]) -> Vec<(RenderGlyphParams, Bounds<DevicePixels>)> {
        let mut out = Vec::new();
        for p in params {
            let job = {
                let state = self.0.read();
                if state.drawn.contains_key(p) {
                    continue;
                }
                let Some(instance) = state.instances.get(p.font_id.0) else {
                    continue;
                };
                let source = &state.sources[instance.source];
                if !source.color {
                    continue;
                }
                (source.data.clone(), source.index, instance.location.clone())
            };
            let (data, index, location) = job;
            let ppem = p.font_size.0 * p.scale_factor;
            if let Some(drawn) = draw_color_data(data, index, &location, p.glyph_id, ppem) {
                out.push((p.clone(), drawn.0));
                self.0.write().keep(p.clone(), drawn);
            }
        }
        out
    }

    fn forget_kept(&self) {
        {
            let mut state = self.0.write();
            state.drawn.retain(|p, _| !p.is_emoji);
            state.kept = 0;
            state.drawn.shrink_to_fit();
        }
        // Thousands of small blocks freed: back to the system, not kept
        // by the allocator for later.
        // SAFETY: glibc's malloc_trim takes no pointers.
        unsafe { libc::malloc_trim(0) };
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        self.0.write().layout_line(text, font_size, runs)
    }

    /// Olo: draws `chars` of `font_id` at every subpixel offset without
    /// holding the text system while it draws (a frame shaping text or
    /// drawing a glyph waits for nothing): what it needs taken under a
    /// moment's read, drawn on a rasterizer of its own, the pixels kept
    /// for frames to take in a moment's write.
    fn draw_ahead(&self, font_id: FontId, font_size: Pixels, scale_factor: f32, rendering: TextRendering, chars: &[char]) -> Vec<(RenderGlyphParams, Bounds<DevicePixels>)> {
        let (data, index, location) = {
            let state = self.0.read();
            let Some(instance) = state.instances.get(font_id.0) else {
                return Vec::new();
            };
            let source = &state.sources[instance.source];
            if source.color {
                return Vec::new();
            }
            (source.data.clone(), source.index, instance.location.clone())
        };
        let Ok(font) = FontRef::from_index((*data).as_ref(), index) else {
            return Vec::new();
        };
        let ppem = font_size.0 * scale_factor;
        let outlines = font.outline_glyphs();
        let hinting = if matches!(rendering, TextRendering::Unhinted) { None } else { hinting_at(&outlines, ppem, &location) };
        let charmap = font.charmap();
        let mut raster = ab_glyph_rasterizer::Rasterizer::new(0, 0);
        let mut drawn = Vec::new();
        for &ch in chars {
            let Some(gid) = charmap.map(ch) else { continue };
            for x in 0..SUBPIXEL_VARIANTS_X {
                let params = RenderGlyphParams {
                    font_id,
                    glyph_id: GlyphId(gid.to_u32()),
                    font_size,
                    // (Vertically, glyphs keep to whole pixels here.)
                    subpixel_variant: point(x, 0),
                    scale_factor,
                    is_emoji: false,
                    rendering,
                };
                if let Some(glyph) = draw_outline(&outlines, &location, hinting.as_ref(), &mut raster, &params, ppem) {
                    drawn.push((params, glyph));
                }
            }
        }
        let bounds = drawn.iter().map(|(p, g)| (p.clone(), g.0)).collect();
        let mut state = self.0.write();
        for (params, glyph) in drawn {
            state.drawn.entry(params).or_insert(glyph);
        }
        bounds
    }
}

impl State {
    fn system(&mut self) -> &fontdb::Database {
        self.system.get_or_insert_with(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            db
        })
    }

    /// The best face of the first of `names` there is, Olo's own first.
    fn find(&mut self, names: &[&str], weight: f32, italic: bool) -> Option<usize> {
        for name in names {
            if let Some(found) = self.find_in(false, name, weight, italic) {
                return Some(found);
            }
            self.system();
            if let Some(found) = self.find_in(true, name, weight, italic) {
                return Some(found);
            }
        }
        None
    }

    fn find_in(&mut self, system: bool, name: &str, weight: f32, italic: bool) -> Option<usize> {
        let db = if system { self.system.as_ref()? } else { &self.own };
        let faces: Vec<(fontdb::ID, bool, f32)> =
            db.faces().filter(|f| f.families.iter().any(|(family, _)| family == name)).map(|f| (f.id, f.style != fontdb::Style::Normal, f32::from(f.weight.0))).collect();
        if faces.is_empty() {
            return None;
        }
        // The style asked for if there is one, then the nearest weight
        // (a variable face's covers its range).
        let mut best: Option<(usize, f32)> = None;
        for (id, face_italic, _) in faces {
            let Some(source) = self.source(system, id) else {
                continue;
            };
            let s = &self.sources[source];
            let off = match s.wght {
                Some((lo, hi)) => (weight.clamp(lo, hi) - weight).abs(),
                None => (s.weight - weight).abs(),
            } + if face_italic == italic { 0.0 } else { 10_000.0 };
            if best.is_none_or(|(_, b)| off < b) {
                best = Some((source, off));
            }
        }
        best.map(|(source, _)| source)
    }

    fn source(&mut self, system: bool, id: fontdb::ID) -> Option<usize> {
        if let Some(source) = self.source_ids.get(&(system, id)) {
            return Some(*source);
        }
        let db = if system { self.system.as_ref()? } else { &self.own };
        let info = db.face(id)?;
        let family = info.families.first()?.0.clone();
        let italic = info.style != fontdb::Style::Normal;
        let weight = f32::from(info.weight.0);
        let data: Data = match &info.source {
            fontdb::Source::Binary(data) => data.clone(),
            fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => {
                let file = std::fs::File::open(path).ok()?;
                // SAFETY: font files aren't written to while in use; a
                // file replaced on disk keeps the mapped copy.
                let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
                Arc::new(map)
            }
        };
        let index = info.index;
        let font = FontRef::from_index((*data).as_ref(), index).ok()?;
        let wght = font.axes().iter().find(|a| a.tag() == WGHT).map(|a| (a.min_value(), a.max_value()));
        let weight = font.axes().iter().find(|a| a.tag() == WGHT).map_or(weight, |a| a.default_value());
        let color = font.colr().is_ok() || font.cbdt().is_ok() || font.sbix().is_ok() || font.ebdt().is_ok();
        let shaper = harfrust::ShaperData::new(&font);
        let source = self.sources.len();
        self.sources.push(Source { data: data.clone(), index, family, italic, weight, wght, color, shaper });
        self.source_ids.insert((system, id), source);
        Some(source)
    }

    /// `source` at `weight`, with `features`.
    fn instance(&mut self, source: usize, weight: f32, features: &[(String, u32)]) -> Result<FontId> {
        let s = &self.sources[source];
        let weight = s.wght.map_or(s.weight, |(lo, hi)| weight.clamp(lo, hi));
        let key = (source, weight.to_bits(), features.to_vec());
        if let Some(id) = self.instance_ids.get(&key) {
            return Ok(*id);
        }
        let font = s.font().context("font")?;
        let (location, shaper_instance) = if s.wght.is_some() {
            (font.axes().location([(WGHT, weight)]), Some(harfrust::ShaperInstance::from_variations(&font, [harfrust::Variation { tag: WGHT, value: weight }])))
        } else {
            (Location::default(), None)
        };
        let m = font.metrics(Ppem::unscaled(), &location);
        let units_per_em = f32::from(m.units_per_em);
        let ascent = m.ascent;
        let descent = -m.descent;
        let metrics = FontMetrics {
            units_per_em: u32::from(m.units_per_em),
            ascent,
            descent,
            line_gap: m.leading,
            underline_position: m.underline.map_or(-units_per_em * 0.1, |u| u.offset),
            underline_thickness: m.underline.map_or(units_per_em * 0.05, |u| u.thickness),
            cap_height: m.cap_height.unwrap_or(ascent * 0.7),
            x_height: m.x_height.unwrap_or(ascent * 0.5),
            bounding_box: m.bounds.map_or(Bounds { origin: point(0.0, -descent), size: size(units_per_em, ascent + descent) }, |b| Bounds {
                origin: point(b.x_min, b.y_min),
                size: size(b.x_max - b.x_min, b.y_max - b.y_min),
            }),
        };
        let features = features
            .iter()
            .filter_map(|(name, value)| {
                let tag: [u8; 4] = name.as_bytes().try_into().ok()?;
                Some(harfrust::Feature::new(harfrust::Tag::new(&tag), *value, ..))
            })
            .collect();
        let id = FontId(self.instances.len());
        self.instances.push(Instance { source, weight, location, shaper_instance, features, units_per_em, metrics, fallbacks: None });
        self.instance_ids.insert(key, id);
        Ok(id)
    }

    /// The faces after `id` for what it lacks: Olo's symbol faces, the
    /// other of the UI and mono faces, DejaVu Sans, CJK, emoji.
    fn fallbacks(&mut self, id: FontId) -> Arc<[FontId]> {
        if let Some(chain) = &self.instances[id.0].fallbacks {
            return chain.clone();
        }
        let weight = self.instances[id.0].weight;
        let family = self.sources[self.instances[id.0].source].family.clone();
        let mono = olo_theme::fonts::MONO.contains(&family.as_str());
        let mut names: Vec<&[&str]> = olo_theme::fonts::SYMBOLS.iter().map(std::slice::from_ref).collect();
        names.push(if mono { olo_theme::fonts::UI } else { olo_theme::fonts::MONO });
        names.push(&["DejaVu Sans"]);
        names.push(&["Noto Sans CJK JP", "Noto Sans Mono CJK JP"]);
        names.push(&["Noto Color Emoji"]);
        names.push(&["Noto Emoji"]);
        let mut chain = Vec::new();
        for list in names {
            if let Some(source) = self.find(list, weight, false)
                && let Ok(fid) = self.instance(source, weight, &[])
                && fid != id
                && !chain.contains(&fid)
            {
                chain.push(fid);
            }
        }
        let chain: Arc<[FontId]> = chain.into();
        self.instances[id.0].fallbacks = Some(chain.clone());
        chain
    }

    fn has(&self, id: FontId, ch: char) -> bool {
        self.sources[self.instances[id.0].source].font().and_then(|f| f.charmap().map(ch)).is_some_and(|g| g.to_u32() != 0)
    }

    /// The face for `ch` in a run of `primary`.
    fn face_for(&mut self, primary: FontId, ch: char, emoji: bool) -> FontId {
        let chain = self.fallbacks(primary);
        let is_color = |s: &Self, id: FontId| s.sources[s.instances[id.0].source].color;
        if emoji && let Some(&id) = chain.iter().find(|&&id| is_color(self, id) && self.has(id, ch)) {
            return id;
        }
        if self.has(primary, ch) {
            return primary;
        }
        if let Some(&id) = chain.iter().find(|&&id| self.has(id, ch)) {
            return id;
        }
        // Anything the system has (once per character).
        let source = match self.last_resort.get(&ch) {
            Some(found) => *found,
            None => {
                self.system();
                let found = self.system.as_ref().and_then(|db| {
                    db.faces()
                        .find(|f| {
                            db.with_face_data(f.id, |data, index| FontRef::from_index(data, index).ok().and_then(|font| font.charmap().map(ch)).is_some_and(|g| g.to_u32() != 0)).unwrap_or(false)
                        })
                        .map(|f| f.id)
                });
                let source = found.and_then(|id| self.source(true, id));
                self.last_resort.insert(ch, source);
                source
            }
        };
        let weight = self.instances[primary.0].weight;
        source.and_then(|s| self.instance(s, weight, &[]).ok()).unwrap_or(primary)
    }

    fn layout_line(&mut self, text: &str, font_size: Pixels, font_runs: &[FontRun]) -> LineLayout {
        let mut runs: Vec<ShapedRun> = Vec::new();
        let mut x = 0.0f32;
        let (mut ascent, mut descent) = (0.0f32, 0.0f32);
        let mut start = 0;
        for run in font_runs {
            let end = (start + run.len).min(text.len());
            let primary = run.font_id;
            let size = run.size.unwrap_or(font_size);
            let m = &self.instances[primary.0].metrics;
            let scale = size.0 / m.units_per_em as f32;
            ascent = ascent.max(m.ascent * scale);
            descent = descent.max(m.descent * scale);
            // Split by the face each character comes from; marks,
            // joiners and selectors stay with what they follow.
            let mut pieces: Vec<(usize, usize, FontId)> = Vec::new();
            let chars: Vec<(usize, char)> = text[start..end].char_indices().map(|(i, c)| (start + i, c)).collect();
            for (n, &(i, ch)) in chars.iter().enumerate() {
                let next = chars.get(n + 1).map(|&(_, c)| c);
                let face = match pieces.last() {
                    Some(&(_, _, face)) if follows(ch) => face,
                    _ => self.face_for(primary, ch, emoji(ch, next)),
                };
                let len = ch.len_utf8();
                match pieces.last_mut() {
                    Some(last) if last.2 == face => last.1 = i + len,
                    _ => pieces.push((i, i + len, face)),
                }
            }
            for (from, to, face) in pieces {
                x = self.shape(text, from, to, face, size, x, &mut runs);
            }
            start = end;
        }
        LineLayout { font_size, width: px(x), ascent: px(ascent), descent: px(descent), runs, len: text.len(), painted: Default::default() }
    }

    /// Shapes `text[from..to]` in `face` from `x`; returns where it ends.
    /// Plain ASCII goes by words ([`State::words`]): each with the spaces
    /// after it shaped once (nothing joins or kerns across a space in
    /// such text), the rest as a whole.
    #[allow(clippy::too_many_arguments)]
    fn shape(&mut self, text: &str, from: usize, to: usize, face: FontId, font_size: Pixels, mut x: f32, runs: &mut Vec<ShapedRun>) -> f32 {
        let instance = &self.instances[face.0];
        let source = &self.sources[instance.source];
        let scale = font_size.0 / instance.units_per_em;
        let is_emoji = source.color;
        let mut shaped = Vec::with_capacity(to - from);
        let mut place = |x: &mut f32, at: usize, glyphs: &[WordGlyph]| {
            for g in glyphs {
                shaped.push(ShapedGlyph { id: GlyphId(g.id), position: point(px(*x + g.x_offset as f32 * scale), px(-g.y_offset as f32 * scale)), index: at + g.cluster as usize, is_emoji });
                *x += g.x_advance as f32 * scale;
            }
        };
        let part = &text[from..to];
        if part.is_ascii() && !is_emoji {
            let bytes = part.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let start = i;
                while i < bytes.len() && bytes[i] != b' ' {
                    i += 1;
                }
                while i < bytes.len() && bytes[i] == b' ' {
                    i += 1;
                }
                let word = &part[start..i];
                let key = {
                    use std::hash::{Hash, Hasher};
                    let mut h = collections::FxHasher::default();
                    (face.0, word).hash(&mut h);
                    h.finish()
                };
                match self.words.get(&key) {
                    Some((w, glyphs)) if &**w == word => place(&mut x, from + start, glyphs),
                    _ => {
                        let glyphs = self.shape_units(word, face);
                        place(&mut x, from + start, &glyphs);
                        if self.words.len() >= WORDS {
                            self.words.clear();
                        }
                        self.words.insert(key, (word.into(), glyphs));
                    }
                }
            }
        } else {
            let glyphs = self.shape_units(part, face);
            place(&mut x, from, &glyphs);
        }
        match runs.last_mut() {
            Some(last) if last.font_id == face && last.font_size == font_size => last.glyphs.extend(shaped),
            _ => runs.push(ShapedRun { font_id: face, font_size, glyphs: shaped }),
        }
        x
    }

    /// `text` shaped in `face` (harfrust, with the face's plan for its
    /// direction, script and language), in the face's units, each glyph's
    /// cluster from `text`'s start.
    fn shape_units(&mut self, text: &str, face: FontId) -> Vec<WordGlyph> {
        let instance = &self.instances[face.0];
        let source = &self.sources[instance.source];
        let Some(font) = source.font() else {
            return Vec::new();
        };
        let shaper = source.shaper.shaper(&font).instance(instance.shaper_instance.as_ref()).build();
        let mut buffer = self.buffer.take().unwrap_or_default();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let key = (face.0, buffer.direction(), buffer.script(), buffer.language());
        let plan = self.plans.entry(key).or_insert_with_key(|(_, direction, script, language)| harfrust::ShapePlan::new(&shaper, *direction, Some(*script), language.as_ref(), &instance.features));
        let glyphs = shaper.shape(buffer, harfrust::ShapeOptions::new().features(&instance.features).plan(Some(plan)));
        let out = glyphs
            .glyph_infos()
            .iter()
            .zip(glyphs.glyph_positions())
            .map(|(info, pos)| WordGlyph { id: info.glyph_id, cluster: info.cluster, x_advance: pos.x_advance, x_offset: pos.x_offset, y_offset: pos.y_offset })
            .collect();
        self.buffer = Some(glyphs.clear());
        out
    }

    /// Draws a glyph: its bounds (in device pixels from its origin on
    /// the baseline) and its pixels (coverage; BGRA for colour glyphs).
    /// Keeps a glyph drawn: an outline one until a frame takes it, a
    /// colour one for good, the oldest let go past `KEEP` bytes of them.
    fn keep(&mut self, params: RenderGlyphParams, drawn: (Bounds<DevicePixels>, Vec<u8>)) {
        let emoji = params.is_emoji;
        let now_scale = params.scale_factor;
        let bytes = drawn.1.len();
        if let Some(old) = self.drawn.insert(params, drawn)
            && emoji
        {
            // (Drawn again: counted once.)
            self.kept -= old.1.len().min(self.kept);
        }
        if !emoji {
            return;
        }
        self.kept += bytes;
        if self.kept > KEEP {
            // Rare (thousands of emoji, or a window gone to a display of
            // another scale): about half go, those drawn for other scales
            // first, then any.
            let mut freed = 0;
            for other_scales_only in [true, false] {
                self.drawn.retain(|p, d| {
                    if !p.is_emoji || freed > KEEP / 2 || (other_scales_only && p.scale_factor == now_scale) {
                        return true;
                    }
                    freed += d.1.len();
                    false
                });
            }
            self.kept -= freed.min(self.kept);
        }
    }

    fn draw(&mut self, params: &RenderGlyphParams) -> Option<(Bounds<DevicePixels>, Vec<u8>)> {
        let ppem = params.font_size.0 * params.scale_factor;
        if !(ppem > 0.0 && ppem.is_finite()) {
            return None;
        }
        let instance = &self.instances[params.font_id.0];
        let source = &self.sources[instance.source];
        if source.color {
            return draw_color_data(source.data.clone(), source.index, &instance.location, params.glyph_id, ppem);
        }
        let font = source.font()?;
        let outlines = font.outline_glyphs();
        let hinted = !matches!(params.rendering, TextRendering::Unhinted);
        let key = (params.font_id.0, ppem.to_bits());
        if hinted && !self.hinting.contains_key(&key) {
            self.hinting.insert(key, hinting_at(&outlines, ppem, &instance.location)?);
        }
        let hinting = if hinted { self.hinting.get(&key) } else { None };
        draw_outline(&outlines, &instance.location, hinting, &mut self.raster, params, ppem)
    }
}

/// The light autohinting (vertical only) for a face at `ppem`.
fn hinting_at(outlines: &skrifa::outline::OutlineGlyphCollection, ppem: f32, location: &Location) -> Option<HintingInstance> {
    HintingInstance::new(outlines, Ppem::new(ppem), location, Target::Smooth { mode: SmoothMode::Light, symmetric_rendering: true, preserve_linear_metrics: true }).ok()
}

/// Draws an outline glyph (not a colour one) as `params` say, hinted by
/// `hinting` unless it is to be drawn unhinted: its bounds and coverage.
fn draw_outline(
    outlines: &skrifa::outline::OutlineGlyphCollection,
    location: &Location,
    hinting: Option<&HintingInstance>,
    raster: &mut ab_glyph_rasterizer::Rasterizer,
    params: &RenderGlyphParams,
    ppem: f32,
) -> Option<(Bounds<DevicePixels>, Vec<u8>)> {
    let glyph = outlines.get(skrifa::GlyphId::new(params.glyph_id.0))?;
    // LCD subpixel glyphs are drawn three times as wide, a sample per
    // colour stripe.
    let (lcd, unhinted) = match params.rendering {
        TextRendering::Subpixel(layout) => (Some(layout), false),
        TextRendering::Unhinted => (None, true),
        TextRendering::Hinted => (None, false),
    };
    let across = if lcd.is_some() { 3.0 } else { 1.0 };
    let mut path = kurbo::BezPath::new();
    let mut pen = Pen { path: &mut path, x: f64::from(params.subpixel_variant.x) / f64::from(SUBPIXEL_VARIANTS_X) * across, across };
    match hinting {
        Some(hinting) if !unhinted => glyph.draw(DrawSettings::hinted(hinting, false), &mut pen).ok()?,
        _ => glyph.draw(DrawSettings::unhinted(Ppem::new(ppem), location), &mut pen).ok()?,
    };
    let darken = darkening(params.font_size.0) * params.scale_factor;
    let reach = f64::from(darken / 2.0);
    let path = if darken > 0.0 { embolden(&path, reach) } else { path };
    // (The box of what's filled, emboldened: a pixel the outline reaches
    // outside the grid would leave its row's coverage unsummed, a grey
    // bar across the glyph.)
    let b = path.control_box();
    // Whole pixels, and for LCD glyphs one more each side, where the
    // filter spreads them.
    let spread = if lcd.is_some() { 1.0 } else { 0.0 };
    let (x0, x1) = ((b.x0 / across).floor() - spread, (b.x1 / across).ceil() + spread);
    let (y0, y1) = (b.y0.floor(), b.y1.ceil());
    let (width, height) = ((x1 - x0) as u16, (y1 - y0) as u16);
    if width == 0 || height == 0 {
        return None;
    }
    let samples = (f64::from(width) * across) as u16;
    let coverage = fill(raster, &path, (-x0 * across, -y0), samples.into(), height.into());
    let pixels = match lcd {
        Some(layout) => lcd_filter(&coverage, width.into(), height.into(), layout),
        None => coverage,
    };
    Some((Bounds { origin: point(DevicePixels(x0 as i32), DevicePixels(y0 as i32)), size: size(DevicePixels(width.into()), DevicePixels(height.into())) }, pixels))
}

/// Three samples a pixel across, one per colour stripe, into a pixel's
/// red, green and blue coverage (RGBA, alpha the most of them), through
/// FreeType's default LCD filter (a five-tap FIR, 8 77 86 77 8 over
/// 256): sharp, yet with no colour fringes a person sees.
fn lcd_filter(samples: &[u8], width: usize, height: usize, layout: SubpixelLayout) -> Vec<u8> {
    const TAPS: [u32; 5] = [8, 77, 86, 77, 8];
    let across = width * 3;
    let mut out = Vec::with_capacity(width * height * 4);
    for row in samples.chunks_exact(across).take(height) {
        let at = |k: isize| -> u32 { if k < 0 || k as usize >= across { 0 } else { u32::from(row[k as usize]) } };
        let filtered = |k: usize| -> u8 {
            let k = k as isize;
            let sum: u32 = TAPS.iter().enumerate().map(|(t, w)| w * at(k + t as isize - 2)).sum();
            ((sum + 128) / 256).min(255) as u8
        };
        for i in 0..width {
            let (a, b, c) = (filtered(3 * i), filtered(3 * i + 1), filtered(3 * i + 2));
            let (r, g, bl) = match layout {
                SubpixelLayout::Rgb => (a, b, c),
                SubpixelLayout::Bgr => (c, b, a),
            };
            out.extend_from_slice(&[r, g, bl, r.max(g).max(bl)]);
        }
    }
    out
}

/// How much text is thickened, in logical pixels: the stroke drawn
/// around its outline, a tenth of a pixel for 12-point text and smaller,
/// tapering to none at 24 points (where strokes are thick enough).
/// Grey-scale anti-aliasing leaves small text's stems thinner and paler
/// than the design (a stem a pixel and a third wide becomes one dark
/// pixel and a pale one); a little dilation, as macOS's font smoothing
/// and FreeType's stem darkening do, gives back its colour: fuller,
/// smoother text, without turning it bold. `OLO_TEXT_DARKEN` sets the
/// amount at 12 points (0: none).
fn darkening(points: f32) -> f32 {
    static AT_12: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    let at_12 = *AT_12.get_or_init(|| std::env::var("OLO_TEXT_DARKEN").ok().and_then(|v| v.parse().ok()).unwrap_or(0.1));
    at_12 * ((24.0 - points) / 12.0).clamp(0.0, 1.0)
}

/// A colour glyph (an emoji), unhinted, cropped to what it paints.
/// The bytes of colour glyphs kept for every window (`State::keep`).
const KEEP: usize = 32 << 20;

/// Draws a colour glyph of the face in `data` (face `index`) at `ppem`,
/// on a canvas its clip box's size where the font gives one.
fn draw_color_data(data: Data, index: u32, location: &Location, glyph: GlyphId, ppem: f32) -> Option<(Bounds<DevicePixels>, Vec<u8>)> {
    use vello_cpu::peniko;
    let canvas = (3.0 * ppem).ceil().clamp(1.0, 1024.0) as u16;
    let (ox, oy) = (ppem, 2.0 * ppem);
    let blob: Arc<dyn AsRef<[u8]> + Send + Sync> = data;
    let font = peniko::FontData::new(peniko::Blob::new(blob), index);
    let coords: Vec<i16> = location.coords().iter().map(|c| c.to_bits()).collect();
    let mut ctx = vello_cpu::RenderContext::new(canvas, canvas);
    let mut resources = vello_cpu::Resources::new();
    ctx.set_paint(vello_cpu::color::OpaqueColor::<vello_cpu::color::Srgb>::WHITE);
    ctx.glyph_run(&mut resources, &font).font_size(ppem).hint(false).normalized_coords(&coords).fill_glyphs(std::iter::once(vello_cpu::Glyph { id: glyph.0, x: ox, y: oy }));
    let mut pixmap = vello_cpu::Pixmap::new(canvas, canvas);
    ctx.render(&mut pixmap, &mut resources);
    let n = canvas as usize;
    let px = pixmap.data_as_u8_slice();
    let (mut x0, mut y0, mut x1, mut y1) = (n, n, 0, 0);
    for y in 0..n {
        for x in 0..n {
            if px[4 * (y * n + x) + 3] != 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let mut bgra = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = 4 * (y * n + x);
            let (r, g, b, a) = (px[i], px[i + 1], px[i + 2], px[i + 3]);
            // Premultiplied to straight, as the kit's pictures are.
            let un = |c: u8| {
                if a == 0 { 0 } else { ((u16::from(c) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8 }
            };
            bgra.extend_from_slice(&[un(b), un(g), un(r), a]);
        }
    }
    Some((Bounds { origin: point(DevicePixels(x0 as i32 - ox as i32), DevicePixels(y0 as i32 - oy as i32)), size: size(DevicePixels((x1 - x0) as i32), DevicePixels((y1 - y0) as i32)) }, bgra))
}

/// Marks, joiners, variation selectors, emoji modifiers and tags: drawn
/// from the face of what they follow.
fn follows(ch: char) -> bool {
    matches!(ch as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF
        | 0xFE20..=0xFE2F | 0x200C | 0x200D | 0xFE00..=0xFE0F
        | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
}

/// Drawn as a colour emoji: pictographs, the symbols shown as emoji by
/// default (✋ ⚡ ✨), and a character asked to be one (U+FE0F, a skin
/// tone, a keycap after it), as the rest of Olo has it.
fn emoji(ch: char, next: Option<char>) -> bool {
    olo_emoji::starts_emoji(ch, next)
}

/// The coverage of `path`, moved by `offset`, on a `width` × `height`
/// grid of pixels: each pixel's area inside it, exactly (the signed area
/// each edge sweeps, summed along the row, as FreeType's and font-rs's
/// rasterizers do), its curves as lines within a fiftieth of a pixel
/// (kurbo's flattening: the rasterizer's own leaves a gentle curve a
/// single line, a sixth of a pixel off at text sizes). A glyph is small,
/// so this takes a few microseconds where a vector renderer's setup alone
/// took tens.
fn fill(raster: &mut ab_glyph_rasterizer::Rasterizer, path: &kurbo::BezPath, offset: (f64, f64), width: usize, height: usize) -> Vec<u8> {
    use ab_glyph_rasterizer::point as pt;
    use kurbo::PathEl;
    raster.reset(width, height);
    let at = |p: kurbo::Point| pt((p.x + offset.0) as f32, (p.y + offset.1) as f32);
    let (mut start, mut now) = (pt(0.0, 0.0), pt(0.0, 0.0));
    kurbo::flatten(path, 0.02, |el| match el {
        PathEl::MoveTo(p) => {
            // An open contour is closed.
            if now != start {
                raster.draw_line(now, start);
            }
            start = at(p);
            now = start;
        }
        PathEl::LineTo(p) => {
            let p = at(p);
            raster.draw_line(now, p);
            now = p;
        }
        PathEl::ClosePath => {
            if now != start {
                raster.draw_line(now, start);
            }
            now = start;
        }
        // (Flattened: lines only.)
        PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
    });
    if now != start {
        raster.draw_line(now, start);
    }
    let mut coverage = vec![0u8; width * height];
    raster.for_each_pixel(|i, a| coverage[i] = (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    coverage
}

/// `path` grown by `r` all round, as filling it and stroking it `2r`
/// wide does (stem darkening): FreeType's emboldening, each point moved
/// out along its corner's bisector so the edges either side of it move
/// out by `r`. At the reach darkening has (a tenth of a pixel or less) it
/// is the stroke to within a hundredth of a pixel, at a tenth of the
/// cost: the stroke's expansion was two thirds of drawing a glyph.
fn embolden(path: &kurbo::BezPath, r: f64) -> kurbo::BezPath {
    use kurbo::{PathEl, Point, Shape as _, Vec2};
    // Out is to the right of the way the outer contours run (y down) for
    // an outline whose area is positive; holes run the other way, so
    // they shrink, as their edges should.
    let side = if path.area() >= 0.0 { -1.0 } else { 1.0 };
    let normal = |d: Vec2| Vec2::new(-d.y, d.x) * side;
    let mut out = kurbo::BezPath::new();
    let els = path.elements();
    let mut start = 0;
    while start < els.len() {
        // A contour: from a move to the next (or the end).
        let end = els[start + 1..].iter().position(|e| matches!(e, PathEl::MoveTo(_))).map_or(els.len(), |i| start + 1 + i);
        let contour = &els[start..end];
        let mut points: Vec<Point> = Vec::new();
        for el in contour {
            match *el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => points.push(p),
                PathEl::QuadTo(a, p) => points.extend([a, p]),
                PathEl::CurveTo(a, b, p) => points.extend([a, b, p]),
                PathEl::ClosePath => {}
            }
        }
        // (A closing point on the first is the first.)
        let n = if points.len() > 1 && points[points.len() - 1] == points[0] { points.len() - 1 } else { points.len() };
        let moved: Vec<Point> = (0..points.len())
            .map(|i| {
                let k = i % n.max(1);
                let p = points[k];
                // The nearest different points either side.
                let before = (1..n).map(|j| points[(k + n - j) % n]).find(|q| *q != p);
                let after = (1..n).map(|j| points[(k + j) % n]).find(|q| *q != p);
                let (Some(before), Some(after)) = (before, after) else {
                    return p;
                };
                let (n_in, n_out) = (normal((p - before).normalize()), normal((after - p).normalize()));
                // v·n_in = v·n_out = r; a hairpin (the normals opposed)
                // stays put rather than shoot out, and a sharp corner (a
                // w's points) moves at most 2r, as FreeType limits its
                // miters: further, it would spike past the glyph's box.
                let d = 1.0 + n_in.dot(n_out);
                if d < 0.05 {
                    return p;
                }
                let v = (n_in + n_out) * (r / d);
                let most = 2.0 * r;
                p + if v.hypot() > most { v * (most / v.hypot()) } else { v }
            })
            .collect();
        let mut at = moved.iter().copied();
        for el in contour {
            let mut next = || at.next().unwrap_or_default();
            match el {
                PathEl::MoveTo(_) => out.move_to(next()),
                PathEl::LineTo(_) => out.line_to(next()),
                PathEl::QuadTo(..) => {
                    let a = next();
                    out.quad_to(a, next());
                }
                PathEl::CurveTo(..) => {
                    let (a, b) = (next(), next());
                    out.curve_to(a, b, next());
                }
                PathEl::ClosePath => out.close_path(),
            }
        }
        start = end;
    }
    out
}

/// An outline into a path, y down, `x` pixels to the right, stretched
/// `across` times as wide.
struct Pen<'a> {
    path: &'a mut kurbo::BezPath,
    x: f64,
    across: f64,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> (f64, f64) {
        (f64::from(x) * self.across + self.x, -f64::from(y))
    }
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.move_to(p);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.line_to(p);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (c, p) = (self.at(cx0, cy0), self.at(x, y));
        self.path.quad_to(c, p);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, p) = (self.at(cx0, cy0), self.at(cx1, cy1), self.at(x, y));
        self.path.curve_to(c0, c1, p);
    }

    fn close(&mut self) {
        self.path.close_path();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long a line never laid out before takes, as a table's cells
    /// are (`cargo test --release -p olo-interface shaping_cost -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn shaping_cost() {
        let system = OloTextSystem::new();
        let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/fonts/NotoSans[wght].ttf")).expect("Olo's Noto Sans");
        system.add_fonts(vec![Cow::Owned(data)]).unwrap();
        let id = system.font_id(&crate::font("Noto Sans")).unwrap();
        let cells = |n: usize, round: usize| -> Vec<String> {
            (0..n)
                .map(|i| match i % 6 {
                    0 => format!("{}", 100_000 + i * 7 + round * 1_000_000),
                    1 => format!("{}.{} MB", i % 997, round),
                    2 => format!("kworker/u{}:{}-events_unbound{round}", i % 64, i % 9),
                    3 => format!("{}:{:02}.{:02}", i % 60, i % 59, round % 100),
                    4 => format!("{}.{} KB/s", i % 512, round),
                    _ => format!("Nov {} {:02}:{:02}", 1 + i % 30, i % 24, round % 60),
                })
                .collect()
        };
        // (Warm: the faces' tables, plans.)
        for line in cells(500, 0) {
            system.layout_line(&line, px(13.0), &[FontRun { len: line.len(), font_id: id, size: None }]);
        }
        let mut best = f64::MAX;
        for round in 1..8 {
            let lines = cells(1000, round);
            let t = std::time::Instant::now();
            for line in &lines {
                system.layout_line(line, px(13.0), &[FontRun { len: line.len(), font_id: id, size: None }]);
            }
            best = best.min(t.elapsed().as_secs_f64() * 1e6 / lines.len() as f64);
        }
        eprintln!("a new line laid out in {best:.2} µs");
    }

    /// A glyph with sharp corners (w, v, A), emboldened as small text
    /// is, never spills out of its bitmap: a pixel its outline doesn't
    /// touch stays empty, at every size, scale and subpixel offset. (A
    /// w's points once reached past its box, and the rows they crossed
    /// were left grey: a box round the letter.)
    #[test]
    fn sharp_glyphs_stay_in_their_box() {
        use kurbo::Shape as _;
        let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/fonts/NotoSans[wght].ttf")).expect("Olo's Noto Sans");
        let font = FontRef::new(&data).expect("a font");
        let outlines = font.outline_glyphs();
        let charmap = font.charmap();
        let mut raster = ab_glyph_rasterizer::Rasterizer::new(0, 0);
        let mut worst = (0u8, String::new());
        for ch in "wvWVyAMk".chars() {
            let glyph_id = charmap.map(ch).expect("in the font");
            let glyph = outlines.get(glyph_id).expect("an outline");
            for points in [9.0f32, 10.5, 11.0, 12.0, 13.0, 15.0, 18.0, 20.0] {
                for scale in [1.0f32, 1.5, 2.0] {
                    for x in 0..SUBPIXEL_VARIANTS_X {
                        let params = RenderGlyphParams {
                            font_id: FontId(0),
                            glyph_id: GlyphId(glyph_id.to_u32()),
                            font_size: px(points),
                            subpixel_variant: point(x, 0),
                            scale_factor: scale,
                            is_emoji: false,
                            rendering: TextRendering::Unhinted,
                        };
                        let ppem = points * scale;
                        let (b, pixels) = draw_outline(&outlines, &Location::default(), None, &mut raster, &params, ppem).expect("drawn");
                        // The outline as drawn: placed and emboldened alike.
                        let mut path = kurbo::BezPath::new();
                        let mut pen = Pen { path: &mut path, x: f64::from(x) / f64::from(SUBPIXEL_VARIANTS_X), across: 1.0 };
                        glyph.draw(DrawSettings::unhinted(Ppem::new(ppem), &Location::default()), &mut pen).expect("an outline");
                        let path = embolden(&path, f64::from(darkening(points) * scale / 2.0));
                        let (w, h) = (b.size.width.0 as usize, b.size.height.0 as usize);
                        for (i, &a) in pixels.iter().enumerate() {
                            let (px0, py0) = ((i % w) as f64 + f64::from(b.origin.x.0), (i / w) as f64 + f64::from(b.origin.y.0));
                            let touched = (0..64).any(|k| path.winding(kurbo::Point::new(px0 + (k % 8) as f64 / 8.0 + 1.0 / 16.0, py0 + (k / 8) as f64 / 8.0 + 1.0 / 16.0)) != 0);
                            if !touched && a > worst.0 {
                                worst = (a, format!("{ch:?} at {points} pt, {scale}×, variant {x}, pixel {} of {w}×{h}", i));
                            }
                        }
                    }
                }
            }
        }
        // (A pixel the outline only grazes between samples may hold a few
        // levels.)
        assert!(worst.0 <= 24, "ink outside the outline: {}/255 ({})", worst.0, worst.1);
    }

    /// Glyphs are filled to their true coverage: each pixel of Noto
    /// Sans's letters, emboldened as small text is, at the sizes text
    /// takes at every scale and a subpixel offset, within a few levels of
    /// grey of 16 × 16 samples of the outline (vello_cpu's fill, before,
    /// was up to 57 levels off on curves).
    #[test]
    fn fill_is_exact() {
        use kurbo::Shape as _;
        let data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/fonts/NotoSans[wght].ttf")).expect("Olo's Noto Sans");
        let font = FontRef::new(&data).expect("a font");
        let outlines = font.outline_glyphs();
        let charmap = font.charmap();
        let mut raster = ab_glyph_rasterizer::Rasterizer::new(0, 0);
        let (mut worst, mut total, mut n) = (0u32, 0u64, 0u64);
        for ppem in [10.5f32, 13.0, 16.5, 22.0, 36.0] {
            for ch in "AaBgQ@&8e(ß".chars() {
                let Some(glyph) = charmap.map(ch).and_then(|g| outlines.get(g)) else {
                    continue;
                };
                let mut path = kurbo::BezPath::new();
                let mut pen = Pen { path: &mut path, x: 0.3, across: 1.0 };
                glyph.draw(DrawSettings::unhinted(Ppem::new(ppem), &Location::default()), &mut pen).expect("an outline");
                let path = embolden(&path, 0.05);
                let b = path.control_box();
                let (x0, y0) = (b.x0.floor(), b.y0.floor());
                let (w, h) = ((b.x1.ceil() - x0) as usize, (b.y1.ceil() - y0) as usize);
                let ours = fill(&mut raster, &path, (-x0, -y0), w, h);
                for (i, &a) in ours.iter().enumerate() {
                    let (x, y) = ((i % w) as f64 + x0, (i / w) as f64 + y0);
                    let inside = (0..256).filter(|k| path.winding(kurbo::Point::new(x + (k % 16) as f64 / 16.0 + 1.0 / 32.0, y + (k / 16) as f64 / 16.0 + 1.0 / 32.0)) != 0).count();
                    let d = (i32::from(a) - (inside as f64 * 255.0 / 256.0).round() as i32).unsigned_abs();
                    worst = worst.max(d);
                    total += u64::from(d);
                    n += 1;
                }
            }
        }
        let mean = total as f64 / n as f64;
        // (The samples are within a few levels themselves.)
        assert!(worst <= 10 && mean < 1.5, "worst {worst}/255, mean {mean:.3}/255 over {n} pixels");
    }

    /// Emboldening grows a square by `r` on every side, and shrinks a
    /// hole in it, whichever way its contours run.
    #[test]
    fn embolden_grows_outlines_and_shrinks_holes() {
        use kurbo::Shape as _;
        for flip in [false, true] {
            let square = |x0: f64, y0: f64, s: f64, cw: bool| {
                let mut pts = vec![(x0, y0), (x0 + s, y0), (x0 + s, y0 + s), (x0, y0 + s)];
                if !cw {
                    pts.reverse();
                }
                pts
            };
            let mut path = kurbo::BezPath::new();
            for (contour, cw) in [(square(0.0, 0.0, 10.0, !flip), !flip), (square(3.0, 3.0, 4.0, flip), flip)] {
                let _ = cw;
                path.move_to(contour[0]);
                for p in &contour[1..] {
                    path.line_to(*p);
                }
                path.close_path();
            }
            let grown = embolden(&path, 0.5);
            // Outer 11 × 11, hole 3 × 3.
            assert!((grown.area().abs() - (121.0 - 9.0)).abs() < 1e-9, "{}", grown.area());
        }
    }
}

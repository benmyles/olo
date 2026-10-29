//! Text too big for the glyph atlas: a screensaver's words, hundreds of
//! pixels high. Laid out as other text is, each new glyph was rasterised
//! on the UI thread (a word's took a frame and more) and grew egui's
//! atlas, which then went to the GPU again whole (17 ms). Here a worker
//! with fonts of its own lays the text out and rasterises its glyphs,
//! each into a texture of its own (a page of Message's words is about 20
//! MB of them, at every scale), and the text shows once they're in, a
//! frame or two after it was first asked for.
//!
//! Each glyph is drawn pixel for pixel, at a whole device pixel.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc},
};

use egui::{Color32, Context, FontId, Painter, Pos2, Rect, TextureHandle, TextureOptions, Vec2, pos2, vec2};

/// Text laid out and rasterised ([`get`]).
pub struct BigText {
    /// Each glyph's rect in device pixels from the text's top left, and
    /// its texture.
    glyphs: Vec<(Rect, GlyphKey, TextureHandle)>,
    /// Its size, in points.
    size: Vec2,
    pixels_per_point: f32,
}

impl BigText {
    /// How big it is, in points.
    pub fn size(&self) -> Vec2 {
        self.size
    }

    /// Paints it in `color` with its top left at `at` (moved onto the
    /// device grid).
    pub fn paint(&self, painter: &Painter, at: Pos2, color: Color32) {
        let ppp = self.pixels_per_point;
        let at = pos2((at.x * ppp).round(), (at.y * ppp).round());
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        for (r, _, texture) in &self.glyphs {
            let r = r.translate(at.to_vec2());
            let r = Rect::from_min_max(pos2(r.min.x / ppp, r.min.y / ppp), pos2(r.max.x / ppp, r.max.y / ppp));
            painter.image(texture.id(), r, uv, color);
        }
    }
}

/// `text` in `font` at the context's scale, if it's ready; else `None`
/// while a worker makes it (a repaint follows).
pub fn get(ctx: &Context, text: &str, font: &FontId) -> Option<Arc<BigText>> {
    let state = ctx.data_mut(|d| d.get_temp_mut_or_default::<Shared>(egui::Id::unique("olo-big-text")).clone());
    let mut s = state.lock().unwrap();
    let pass = ctx.cumulative_pass_nr();
    if s.pass != pass {
        s.pass = pass;
        s.take_replies(ctx);
        // Words not shown for a while (5 s at 120 Hz) go, and with them
        // glyphs no word shows.
        let words = s.words.len();
        s.words.retain(|_, w| !matches!(w, Word::Ready(_, used) if *used + 600 < pass));
        if s.words.len() < words {
            let shown: std::collections::HashSet<GlyphKey> =
                s.words.values().filter_map(|w| if let Word::Ready(t, _) = w { Some(t.glyphs.iter().map(|g| g.1)) } else { None }).flatten().collect();
            s.glyphs.retain(|k, _| shown.contains(k));
        }
    }
    let key = (text.to_owned(), font.size.to_bits(), font.family.clone(), ctx.pixels_per_point().to_bits());
    match s.words.get_mut(&key) {
        Some(Word::Ready(t, used)) => {
            *used = pass;
            Some(t.clone())
        }
        Some(Word::Waiting) => None,
        None => {
            let options = ctx.fonts(|f| f.options().clone());
            let job = Job { key: key.clone(), options, ctx: ctx.clone(), reply: s.reply.clone() };
            if s.jobs.send(job).is_ok() {
                s.words.insert(key, Word::Waiting);
            }
            None
        }
    }
}

/// Text, font size and family, scale.
type Key = (String, u32, egui::FontFamily, u32);

/// Where the worker's atlas had a glyph: one glyph while its galleys
/// last (`Fonts::galley_generation`).
type GlyphKey = (u64, [u16; 2], [u16; 2]);

type Shared = Arc<Mutex<State>>;

enum Word {
    Waiting,
    Ready(Arc<BigText>, u64),
}

struct State {
    words: HashMap<Key, Word>,
    glyphs: HashMap<GlyphKey, TextureHandle>,
    jobs: mpsc::Sender<Job>,
    reply: mpsc::Sender<Reply>,
    replies: mpsc::Receiver<Reply>,
    pass: u64,
}

impl Default for State {
    fn default() -> Self {
        let (jobs, rx) = mpsc::channel::<Job>();
        let _ = std::thread::Builder::new().name("olo-big-text".into()).spawn(move || work(rx));
        let (reply, replies) = mpsc::channel();
        State { words: HashMap::new(), glyphs: HashMap::new(), jobs, reply, replies, pass: 0 }
    }
}

/// How many bytes of glyphs go to the GPU in a frame, at most.
const UPLOAD_PER_FRAME: usize = 3 << 19;

impl State {
    /// Words the worker has made: their glyphs to textures (those not
    /// already one), a frame's worth at a time.
    fn take_replies(&mut self, ctx: &Context) {
        let mut bytes = 0;
        while bytes < UPLOAD_PER_FRAME {
            let Ok(reply) = self.replies.try_recv() else { break };
            let mut glyphs = Vec::with_capacity(reply.glyphs.len());
            for (r, key, image) in reply.glyphs {
                let texture = match self.glyphs.get(&key) {
                    Some(t) => t.clone(),
                    None => {
                        bytes += image.width() * image.height() * 4;
                        let t = ctx.load_texture(format!("big-text {key:?}"), image, TextureOptions::NEAREST);
                        self.glyphs.insert(key, t.clone());
                        t
                    }
                };
                glyphs.push((r, key, texture));
            }
            let text = BigText { glyphs, size: reply.size, pixels_per_point: f32::from_bits(reply.key.3) };
            self.words.insert(reply.key, Word::Ready(Arc::new(text), self.pass));
        }
    }
}

struct Job {
    key: Key,
    options: egui::epaint::text::TextOptions,
    ctx: Context,
    reply: mpsc::Sender<Reply>,
}

struct Reply {
    key: Key,
    size: Vec2,
    glyphs: Vec<(Rect, GlyphKey, egui::ColorImage)>,
}

/// The worker: fonts of its own, made with the first job's options.
fn work(jobs: mpsc::Receiver<Job>) {
    let mut fonts: Option<egui::epaint::text::Fonts> = None;
    for job in jobs {
        let fonts = fonts.get_or_insert_with(|| egui::epaint::text::Fonts::new(job.options.clone(), crate::style::all_font_definitions()));
        fonts.begin_pass(job.options.clone(), Default::default());
        let (text, font, ppp) = (&job.key.0, FontId::new(f32::from_bits(job.key.1), job.key.2.clone()), f32::from_bits(job.key.3));
        let galley = fonts.with_pixels_per_point(ppp).layout_job(egui::text::LayoutJob::simple_singleline(text.clone(), font, Color32::WHITE));
        let generation = fonts.galley_generation();
        let atlas = fonts.texture_atlas().image();
        let mut glyphs = Vec::new();
        for row in &galley.rows {
            for g in &row.glyphs {
                let uv = g.uv_rect;
                if uv.is_nothing() {
                    continue;
                }
                let (w, h) = ((uv.max[0] - uv.min[0]) as usize, (uv.max[1] - uv.min[1]) as usize);
                let mut image = egui::ColorImage::filled([w, h], Color32::TRANSPARENT);
                for y in 0..h {
                    let from = (uv.min[1] as usize + y) * atlas.width() + uv.min[0] as usize;
                    image.pixels[y * w..(y + 1) * w].copy_from_slice(&atlas.pixels[from..from + w]);
                }
                let at = (row.pos + g.pos.to_vec2() + uv.offset) * ppp;
                let r = Rect::from_min_size(pos2(at.x.round(), at.y.round()), vec2(w as f32, h as f32));
                glyphs.push((r, (generation, uv.min, uv.max), image));
            }
        }
        if job.reply.send(Reply { key: job.key, size: galley.size(), glyphs }).is_err() {
            continue;
        }
        job.ctx.request_repaint();
    }
}

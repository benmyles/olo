//! Images in the terminal: the kitty graphics protocol, Sixel and iTerm2's
//! inline images.
//!
//! An image is anchored to the grid by tagging the cells it covers with a
//! private hyperlink (`tabula-img:<placement>:<row>`). The image then
//! scrolls, reflows into the scrollback and gets erased exactly like the
//! text it replaced, with no bookkeeping of line numbers; drawing finds the
//! tagged cells and paints the matching slice of the image over them.

use std::{
    collections::HashMap,
    io::Read,
    sync::{Arc, Mutex},
};

use alacritty_terminal::{
    event::EventListener,
    grid::Dimensions,
    index::Column,
    term::Term,
    vte::ansi::{Handler, Hyperlink},
};
use base64::Engine;

pub const URI_PREFIX: &str = "tabula-img:";
/// Most image data kept per window; the oldest images go first.
const BUDGET: usize = 320 * 1024 * 1024;
/// Largest single image accepted (pixels).
const MAX_PIXELS: usize = 10_000 * 10_000;

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
    /// Bumped when the image is replaced, so textures get rebuilt.
    pub generation: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub image: u32,
    pub start_col: usize,
    pub cols: usize,
    pub rows: usize,
    /// Source rectangle in image pixels (kitty's x, y, w, h).
    pub src: [u32; 4],
}

#[derive(Default)]
pub struct Store {
    pub images: HashMap<u32, Image>,
    pub placements: HashMap<u32, Placement>,
    /// kitty's image ids and numbers → ours.
    kitty_ids: HashMap<u32, u32>,
    kitty_numbers: HashMap<u32, u32>,
    next_image: u32,
    next_placement: u32,
    generation: u64,
    order: Vec<u32>,
    /// A kitty transmission arriving in chunks (m=1).
    partial: Option<(Params, Vec<u8>)>,
}

pub type Shared = Arc<Mutex<Store>>;

impl Store {
    fn add_image(&mut self, width: u32, height: u32, rgba: Vec<u8>, id: Option<u32>) -> u32 {
        self.generation += 1;
        let id = id.unwrap_or_else(|| {
            self.next_image += 1;
            self.next_image
        });
        self.images.insert(id, Image { width, height, rgba: Arc::new(rgba), generation: self.generation });
        self.order.retain(|&i| i != id);
        self.order.push(id);
        // Stay within budget: drop the oldest images (their cells simply
        // show nothing).
        let mut total: usize = self.images.values().map(|i| i.rgba.len()).sum();
        while total > BUDGET && self.order.len() > 1 {
            let old = self.order.remove(0);
            if let Some(img) = self.images.remove(&old) {
                total -= img.rgba.len();
            }
            self.placements.retain(|_, p| p.image != old);
        }
        id
    }

    fn new_image_id(&mut self) -> u32 {
        // Our own ids live above kitty's 32-bit space by starting high and
        // counting up; kitty ids map through `kitty_ids`.
        self.next_image += 1;
        self.next_image
    }
}

/// What graphics output needs from the session.
pub struct Context<'a> {
    pub store: &'a Shared,
    /// Cell size in device pixels.
    pub cell: (u32, u32),
    /// Replies for the program (kitty's OK/errors).
    pub reply: &'a mut Vec<u8>,
}

/// Write the image placement into the grid at the cursor: `rows` rows of
/// `cols` tagged blank cells. `after` says where the cursor ends up.
#[derive(Clone, Copy, PartialEq)]
enum After {
    /// Right of the image on its last row (kitty).
    Right,
    /// Start of the line below the image (Sixel, iTerm2).
    Below,
    /// Where it was (kitty's C=1).
    Stay,
}

fn place<T: EventListener>(term: &mut Term<T>, store: &Shared, mut p: Placement, after: After) {
    let cols = term.columns();
    let start = term.grid().cursor.point;
    p.start_col = start.column.0;
    p.cols = p.cols.min(cols - p.start_col).max(1);
    let uid = {
        let mut s = store.lock().unwrap();
        s.next_placement += 1;
        let uid = s.next_placement;
        s.placements.insert(uid, p);
        uid
    };
    let template = term.grid().cursor.template.clone();
    let saved = term.grid().cursor.point;
    let mut t = template.clone();
    t.fg = alacritty_terminal::vte::ansi::Color::Named(alacritty_terminal::vte::ansi::NamedColor::Foreground);
    t.bg = alacritty_terminal::vte::ansi::Color::Named(alacritty_terminal::vte::ansi::NamedColor::Background);
    t.flags = alacritty_terminal::term::cell::Flags::empty();
    t.extra = None;
    term.grid_mut().cursor.template = t;
    for row in 0..p.rows {
        term.set_hyperlink(Some(Hyperlink { id: Some(format!("tabula-img-{uid}-{row}")), uri: format!("{URI_PREFIX}{uid}:{row}") }));
        for _ in 0..p.cols {
            term.input(' ');
        }
        term.set_hyperlink(None);
        if row + 1 < p.rows {
            term.linefeed();
            term.carriage_return();
            if p.start_col > 0 {
                term.move_forward(Column(p.start_col).0);
            }
        }
    }
    let mut template = template;
    template.extra = template.extra.take();
    term.grid_mut().cursor.template = template;
    match after {
        After::Right => {}
        After::Below => {
            term.linefeed();
            term.carriage_return();
        }
        After::Stay => {
            term.goto(saved.line.0, saved.column.0);
        }
    }
}

/// Cells needed to show `w`×`h` pixels.
fn cells_for(w: u32, h: u32, cell: (u32, u32)) -> (usize, usize) {
    (w.div_ceil(cell.0.max(1)).max(1) as usize, h.div_ceil(cell.1.max(1)).max(1) as usize)
}

fn decode(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(data).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    ((w as usize) * (h as usize) <= MAX_PIXELS).then(|| (w, h, img.into_raw()))
}

// ---- kitty ------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Params {
    action: char,
    format: u32,
    medium: char,
    width: u32,
    height: u32,
    compressed: bool,
    id: Option<u32>,
    number: Option<u32>,
    placement: Option<u32>,
    more: bool,
    quiet: u32,
    cols: u32,
    rows: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    no_move: bool,
    delete: char,
    size: usize,
    offset: usize,
}

fn parse_params(s: &str) -> Params {
    let mut p = Params { action: 't', format: 32, medium: 'd', delete: 'a', ..Default::default() };
    for kv in s.split(',') {
        let Some((k, v)) = kv.split_once('=') else { continue };
        let n = || v.parse::<u32>().unwrap_or(0);
        let ch = || v.chars().next().unwrap_or(' ');
        match k {
            "a" => p.action = ch(),
            "f" => p.format = n(),
            "t" => p.medium = ch(),
            "s" => p.width = n(),
            "v" => p.height = n(),
            "o" => p.compressed = v == "z",
            "i" => p.id = Some(n()),
            "I" => p.number = Some(n()),
            "p" => p.placement = Some(n()),
            "m" => p.more = v == "1",
            "q" => p.quiet = n(),
            "c" => p.cols = n(),
            "r" => p.rows = n(),
            "x" => p.x = n(),
            "y" => p.y = n(),
            "w" => p.w = n(),
            "h" => p.h = n(),
            "C" => p.no_move = v == "1",
            "d" => p.delete = ch(),
            "S" => p.size = n() as usize,
            "O" => p.offset = n() as usize,
            _ => {}
        }
    }
    p
}

fn kitty_reply(ctx: &mut Context, p: &Params, message: &str) {
    let ok = message == "OK";
    if (ok && p.quiet >= 1) || p.quiet >= 2 || (p.id.is_none() && p.number.is_none()) {
        return;
    }
    let mut keys = Vec::new();
    if let Some(i) = p.id {
        keys.push(format!("i={i}"));
    }
    if let Some(n) = p.number {
        keys.push(format!("I={n}"));
    }
    if let Some(pl) = p.placement {
        keys.push(format!("p={pl}"));
    }
    ctx.reply.extend_from_slice(format!("\x1b_G{};{message}\x1b\\", keys.join(",")).as_bytes());
}

/// Read the image bytes the transmission medium points at.
fn medium_bytes(p: &Params, payload: Vec<u8>) -> Result<Vec<u8>, &'static str> {
    match p.medium {
        'd' => Ok(payload),
        'f' | 't' | 's' => {
            let name = String::from_utf8(payload).map_err(|_| "EINVAL:bad file name")?;
            let path = if p.medium == 's' { format!("/dev/shm/{}", name.trim_start_matches('/')) } else { name.clone() };
            let mut f = std::fs::File::open(&path).map_err(|_| "EBADF:could not open file")?;
            let mut data = Vec::new();
            if p.offset > 0 {
                use std::io::Seek;
                f.seek(std::io::SeekFrom::Start(p.offset as u64)).map_err(|_| "EINVAL:bad offset")?;
            }
            if p.size > 0 {
                f.take(p.size as u64).read_to_end(&mut data)
            } else {
                f.read_to_end(&mut data)
            }
            .map_err(|_| "EBADF:could not read file")?;
            // Temporary files and shared memory are ours to remove.
            if p.medium == 't' && path.contains("tty-graphics-protocol") || p.medium == 's' {
                let _ = std::fs::remove_file(&path);
            }
            Ok(data)
        }
        _ => Err("EINVAL:unsupported transmission medium"),
    }
}

fn kitty_image(p: &Params, data: Vec<u8>) -> Result<(u32, u32, Vec<u8>), &'static str> {
    let data = if p.compressed {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(&data[..]).read_to_end(&mut out).map_err(|_| "EINVAL:bad zlib data")?;
        out
    } else {
        data
    };
    match p.format {
        100 => decode(&data).ok_or("EBADPNG:could not decode image"),
        24 | 32 => {
            let (w, h) = (p.width, p.height);
            let bpp = if p.format == 24 { 3 } else { 4 };
            let need = w as usize * h as usize * bpp;
            if w == 0 || h == 0 || data.len() < need || (w as usize * h as usize) > MAX_PIXELS {
                return Err("ENODATA:insufficient image data");
            }
            let rgba = if bpp == 4 {
                data[..need].to_vec()
            } else {
                data[..need].chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect()
            };
            Ok((w, h, rgba))
        }
        _ => Err("EINVAL:unknown format"),
    }
}

/// Handle `ESC _ G … ESC \` (the bytes between `G` and the terminator).
pub fn kitty<T: EventListener>(term: &mut Term<T>, ctx: &mut Context, body: &[u8]) {
    let text = String::from_utf8_lossy(body);
    let (keys, payload) = text.split_once(';').unwrap_or((&text, ""));
    let mut p = parse_params(keys);
    let mut data = base64::engine::general_purpose::STANDARD.decode(payload.trim()).unwrap_or_default();

    // Chunked transmission: gather until the last chunk.
    {
        let mut s = ctx.store.lock().unwrap();
        if let Some((first, mut acc)) = s.partial.take() {
            acc.extend_from_slice(&data);
            if p.more {
                s.partial = Some((first, acc));
                return;
            }
            let quiet = p.quiet.max(first.quiet);
            p = first;
            p.quiet = quiet;
            data = acc;
        } else if p.more && matches!(p.action, 't' | 'T' | 'q') {
            s.partial = Some((p, data));
            return;
        }
    }

    match p.action {
        'q' => {
            let result = medium_bytes(&p, data).and_then(|d| kitty_image(&p, d));
            kitty_reply(ctx, &p, if result.is_ok() { "OK" } else { result.err().unwrap_or("EINVAL") });
        }
        't' | 'T' => {
            let result = medium_bytes(&p, data).and_then(|d| kitty_image(&p, d));
            match result {
                Ok((w, h, rgba)) => {
                    let id = {
                        let mut s = ctx.store.lock().unwrap();
                        let ours = match p.id.and_then(|i| s.kitty_ids.get(&i).copied()) {
                            Some(existing) => existing,
                            None => s.new_image_id(),
                        };
                        let id = s.add_image(w, h, rgba, Some(ours));
                        if let Some(i) = p.id {
                            s.kitty_ids.insert(i, id);
                        }
                        if let Some(n) = p.number {
                            s.kitty_numbers.insert(n, id);
                        }
                        id
                    };
                    if p.action == 'T' {
                        put(term, ctx, &p, id, w, h);
                    }
                    kitty_reply(ctx, &p, "OK");
                }
                Err(e) => kitty_reply(ctx, &p, e),
            }
        }
        'p' => {
            let found = {
                let s = ctx.store.lock().unwrap();
                p.id
                    .and_then(|i| s.kitty_ids.get(&i))
                    .or_else(|| p.number.and_then(|n| s.kitty_numbers.get(&n)))
                    .and_then(|id| s.images.get(id).map(|img| (*id, img.width, img.height)))
            };
            match found {
                Some((id, w, h)) => {
                    put(term, ctx, &p, id, w, h);
                    kitty_reply(ctx, &p, "OK");
                }
                None => kitty_reply(ctx, &p, "ENOENT:no such image"),
            }
        }
        'd' => {
            let mut s = ctx.store.lock().unwrap();
            let free = p.delete.is_ascii_uppercase();
            match p.delete.to_ascii_lowercase() {
                'i' => {
                    if let Some(&id) = p.id.and_then(|i| s.kitty_ids.get(&i)) {
                        s.placements.retain(|_, pl| pl.image != id);
                        if free {
                            s.images.remove(&id);
                            s.kitty_ids.retain(|_, v| *v != id);
                        }
                    }
                }
                'n' => {
                    if let Some(&id) = p.number.and_then(|n| s.kitty_numbers.get(&n)) {
                        s.placements.retain(|_, pl| pl.image != id);
                        if free {
                            s.images.remove(&id);
                        }
                    }
                }
                _ => {
                    // All placements (and, upper-case, the images too).
                    s.placements.clear();
                    if free {
                        s.images.clear();
                        s.kitty_ids.clear();
                        s.kitty_numbers.clear();
                    }
                }
            }
        }
        _ => kitty_reply(ctx, &p, "EINVAL:unsupported action"),
    }
}

fn put<T: EventListener>(term: &mut Term<T>, ctx: &Context, p: &Params, id: u32, w: u32, h: u32) {
    let src = [
        p.x.min(w),
        p.y.min(h),
        if p.w > 0 { p.w.min(w - p.x.min(w)) } else { w - p.x.min(w) },
        if p.h > 0 { p.h.min(h - p.y.min(h)) } else { h - p.y.min(h) },
    ];
    let natural = cells_for(src[2], src[3], ctx.cell);
    let (cols, rows) = match (p.cols, p.rows) {
        (0, 0) => natural,
        (c, 0) => (c as usize, ((c as f32 * ctx.cell.0 as f32 / src[2].max(1) as f32 * src[3] as f32) / ctx.cell.1 as f32).ceil().max(1.0) as usize),
        (0, r) => (((r as f32 * ctx.cell.1 as f32 / src[3].max(1) as f32 * src[2] as f32) / ctx.cell.0 as f32).ceil().max(1.0) as usize, r as usize),
        (c, r) => (c as usize, r as usize),
    };
    let after = if p.no_move { After::Stay } else { After::Right };
    place(term, ctx.store, Placement { image: id, start_col: 0, cols, rows, src }, after);
}

// ---- iTerm2 -----------------------------------------------------------------

/// Handle `ESC ] 1337 ; File=… : <base64> BEL`.
pub fn iterm<T: EventListener>(term: &mut Term<T>, ctx: &mut Context, body: &[u8]) {
    let text = String::from_utf8_lossy(body);
    let Some(rest) = text.strip_prefix("File=") else { return };
    let Some((args, payload)) = rest.split_once(':') else { return };
    let mut inline = false;
    let (mut width, mut height) = (String::from("auto"), String::from("auto"));
    let mut preserve = true;
    for kv in args.split(';') {
        let Some((k, v)) = kv.split_once('=') else { continue };
        match k {
            "inline" => inline = v == "1",
            "width" => width = v.into(),
            "height" => height = v.into(),
            "preserveAspectRatio" => preserve = v != "0",
            _ => {}
        }
    }
    if !inline {
        return; // A download; nothing to show.
    }
    let Ok(data) = base64::engine::general_purpose::STANDARD.decode(payload.trim()) else { return };
    let Some((w, h, rgba)) = decode(&data) else { return };
    let (cols_avail, rows_avail) = (term.columns() as u32, term.screen_lines() as u32);
    let spec = |s: &str, cell: u32, avail: u32| -> Option<u32> {
        if s == "auto" {
            None
        } else if let Some(px) = s.strip_suffix("px") {
            px.parse::<u32>().ok().map(|v| v.div_ceil(cell).max(1))
        } else if let Some(pc) = s.strip_suffix('%') {
            pc.parse::<u32>().ok().map(|v| (avail * v / 100).max(1))
        } else {
            s.parse::<u32>().ok()
        }
    };
    let natural = cells_for(w, h, ctx.cell);
    let cw = spec(&width, ctx.cell.0, cols_avail);
    let ch = spec(&height, ctx.cell.1, rows_avail);
    let aspect = |cols: u32| ((cols * ctx.cell.0) as f32 * h as f32 / w.max(1) as f32 / ctx.cell.1 as f32).ceil().max(1.0) as u32;
    let (mut cols, rows) = match (cw, ch) {
        (Some(c), Some(r)) if !preserve => (c, r),
        (Some(c), _) => (c, aspect(c)),
        (None, Some(r)) => (((r * ctx.cell.1) as f32 * w as f32 / h.max(1) as f32 / ctx.cell.0 as f32).ceil() as u32, r),
        (None, None) => (natural.0 as u32, natural.1 as u32),
    };
    // Too wide: shrink to fit, keeping the aspect ratio.
    let mut rows = rows;
    if cols > cols_avail {
        cols = cols_avail;
        rows = aspect(cols);
    }
    let id = ctx.store.lock().unwrap().add_image(w, h, rgba, None);
    place(term, ctx.store, Placement { image: id, start_col: 0, cols: cols as usize, rows: rows as usize, src: [0, 0, w, h] }, After::Below);
}

// ---- Sixel ------------------------------------------------------------------

/// The VT340's default colour registers.
const SIXEL_PALETTE: [[u8; 3]; 16] = [
    [0, 0, 0], [51, 51, 204], [204, 36, 36], [51, 204, 51],
    [204, 51, 204], [51, 204, 204], [204, 204, 51], [120, 120, 120],
    [69, 69, 69], [87, 87, 153], [153, 69, 69], [87, 153, 87],
    [153, 87, 153], [87, 153, 153], [153, 153, 87], [204, 204, 204],
];

/// Decode Sixel data (after the `q`). `transparent` is P2 = 1.
pub fn decode_sixel(data: &[u8], transparent: bool, background: [u8; 3]) -> Option<(u32, u32, Vec<u8>)> {
    let mut palette: Vec<[u8; 3]> = SIXEL_PALETTE.to_vec();
    palette.resize(256, [0, 0, 0]);
    let mut color = 0usize;
    let (mut x, mut y) = (0usize, 0usize);
    let (mut width, mut height) = (0usize, 0usize);
    // Pixels as palette entries + 1 (0 = unset), grown as needed.
    let mut canvas = Canvas::default();
    let mut declared: Option<(usize, usize)> = None;

    let mut i = 0;
    let num = |data: &[u8], i: &mut usize| -> Vec<usize> {
        let mut out = vec![0usize];
        while *i < data.len() {
            match data[*i] {
                b'0'..=b'9' => {
                    let last = out.last_mut().unwrap();
                    *last = last.saturating_mul(10).saturating_add((data[*i] - b'0') as usize);
                }
                b';' => out.push(0),
                _ => break,
            }
            *i += 1;
        }
        out
    };
    while i < data.len() {
        let b = data[i];
        match b {
            b'"' => {
                i += 1;
                let a = num(data, &mut i);
                if a.len() >= 4 && a[2] > 0 && a[3] > 0 {
                    declared = Some((a[2].min(10_000), a[3].min(10_000)));
                    canvas.grow(a[2].min(10_000), a[3].min(10_000));
                }
                continue;
            }
            b'#' => {
                i += 1;
                let a = num(data, &mut i);
                color = a[0].min(255);
                if a.len() >= 5 {
                    let (u, p1, p2, p3) = (a[1], a[2], a[3], a[4]);
                    palette[color] = if u == 2 {
                        [(p1 * 255 / 100) as u8, (p2 * 255 / 100) as u8, (p3 * 255 / 100) as u8]
                    } else {
                        hls(p1 as f32, p2 as f32 / 100.0, p3 as f32 / 100.0)
                    };
                }
                continue;
            }
            b'!' => {
                i += 1;
                let a = num(data, &mut i);
                let n = a[0].max(1).min(10_000);
                if i < data.len() && (0x3f..=0x7e).contains(&data[i]) {
                    let bits = data[i] - 0x3f;
                    canvas.grow(x + n, y + 6);
                    for _ in 0..n {
                        canvas.put(x, y, bits, color);
                        x += 1;
                    }
                    width = width.max(x);
                    if bits != 0 {
                        height = height.max(y + 6 - bits.leading_zeros() as usize + 2);
                    }
                    i += 1;
                }
                continue;
            }
            b'$' => x = 0,
            b'-' => {
                x = 0;
                y += 6;
            }
            0x3f..=0x7e => {
                let bits = b - 0x3f;
                canvas.grow(x + 1, y + 6);
                canvas.put(x, y, bits, color);
                x += 1;
                width = width.max(x);
                if bits != 0 {
                    height = height.max(y + 8 - bits.leading_zeros() as usize);
                }
            }
            _ => {}
        }
        i += 1;
    }
    if let Some((dw, dh)) = declared {
        width = width.max(dw);
        height = height.max(dh);
    }
    if width == 0 || height == 0 || width * height > MAX_PIXELS {
        return None;
    }
    canvas.grow(width, height);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        for col in 0..width {
            let v = canvas.px[row * canvas.stride + col];
            if v == 0 {
                if transparent {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                } else {
                    rgba.extend_from_slice(&[background[0], background[1], background[2], 255]);
                }
            } else {
                let c = palette[(v - 1) as usize];
                rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
    }
    Some((width as u32, height as u32, rgba))
}

/// Sixel pixels, growing as the image does (capped at 10000×10000).
#[derive(Default)]
struct Canvas {
    px: Vec<u16>,
    stride: usize,
    rows: usize,
}

impl Canvas {
    fn grow(&mut self, w: usize, h: usize) {
        let (w, h) = (w.min(10_000), h.min(10_000));
        if w <= self.stride && h <= self.rows {
            return;
        }
        let stride = if w > self.stride { w.next_power_of_two().min(10_000) } else { self.stride };
        let rows = if h > self.rows { h.next_power_of_two().min(10_000) } else { self.rows };
        let mut px = vec![0u16; stride * rows];
        for r in 0..self.rows {
            px[r * stride..r * stride + self.stride].copy_from_slice(&self.px[r * self.stride..(r + 1) * self.stride]);
        }
        (self.px, self.stride, self.rows) = (px, stride, rows);
    }

    fn put(&mut self, x: usize, y: usize, bits: u8, color: usize) {
        if x >= self.stride {
            return;
        }
        for k in 0..6 {
            if bits & (1 << k) != 0 && y + k < self.rows {
                self.px[(y + k) * self.stride + x] = color as u16 + 1;
            }
        }
    }
}

/// Sixel's HLS (hue 0° = blue) to RGB.
fn hls(h: f32, l: f32, s: f32) -> [u8; 3] {
    let h = (h + 240.0) % 360.0 / 360.0;
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

/// Handle `ESC P <params> q <data> ESC \`.
pub fn sixel<T: EventListener>(term: &mut Term<T>, ctx: &mut Context, params: &[u8], data: &[u8], background: [u8; 3]) {
    let p2 = String::from_utf8_lossy(params).split(';').nth(1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let Some((w, h, rgba)) = decode_sixel(data, p2 == 1, background) else { return };
    let (cols, rows) = cells_for(w, h, ctx.cell);
    let id = ctx.store.lock().unwrap().add_image(w, h, rgba, None);
    place(term, ctx.store, Placement { image: id, start_col: 0, cols, rows, src: [0, 0, w, h] }, After::Below);
}

/// The placement and image row a tagged cell shows, from its hyperlink.
pub fn parse_uri(uri: &str) -> Option<(u32, usize)> {
    let rest = uri.strip_prefix(URI_PREFIX)?;
    let (uid, row) = rest.split_once(':')?;
    Some((uid.parse().ok()?, row.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixel_basic() {
        // Two columns, colour 1 set to pure red (RGB percent), full sixel.
        let (w, h, rgba) = decode_sixel(b"#1;2;100;0;0#1~~", false, [9, 9, 9]).unwrap();
        assert_eq!((w, h), (2, 6));
        assert_eq!(&rgba[0..4], &[255, 0, 0, 255]);
        // Repeat and new line.
        let (w, h, _) = decode_sixel(b"#2!10~-!10~", true, [0, 0, 0]).unwrap();
        assert_eq!((w, h), (10, 12));
    }

    #[test]
    fn sixel_declared_size_and_transparency() {
        let (w, h, rgba) = decode_sixel(b"\"1;1;4;3#1@", true, [0, 0, 0]).unwrap();
        assert_eq!((w, h), (4, 3));
        // Unset pixels are transparent.
        assert_eq!(rgba[4 * 4 + 3], 0);
    }

    #[test]
    fn kitty_params() {
        let p = parse_params("a=T,f=100,i=7,c=10,r=5,q=2,m=1");
        assert_eq!((p.action, p.format, p.id, p.cols, p.rows, p.quiet, p.more), ('T', 100, Some(7), 10, 5, 2, true));
    }

    #[test]
    fn uri_round_trip() {
        assert_eq!(parse_uri("tabula-img:42:3"), Some((42, 3)));
        assert_eq!(parse_uri("https://example.com"), None);
    }
}

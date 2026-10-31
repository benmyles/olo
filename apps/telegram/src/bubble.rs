//! A message, laid out and painted: a balloon (ours on the right in a
//! tint of the keyboard's blue, theirs on the left on the card colour),
//! with who sent it in a group, what it forwards or answers, its picture
//! or file, its text (bold, italic, code, links, spoilers; emoji in
//! colour), its time (edited, read ticks, views) and reactions under it.
//! Stickers and a few emoji alone stand on the background with no
//! balloon; service messages are a pill in the middle. Laying out is
//! done once per message and width (`Laid`, cached by the view); painting
//! reads it.

use std::sync::Arc;

use olo_kit::{
    egui::{self, Align2, Color32, FontFamily, FontId, Galley, Painter, Pos2, Rect, Stroke, Vec2, pos2, text::LayoutJob, vec2},
    emoji, style,
};
use olo_tg::{EntityKind, Media, Message, Pending};

/// Message text's size.
pub const TEXT: f32 = 13.0;
/// The time and other small print.
pub const META: f32 = 10.5;
/// Inside a balloon.
pub const PAD: Vec2 = vec2(10.0, 6.0);
pub const RADIUS: f32 = 12.0;
/// A corner where the balloon joins the next of the same sender's.
pub const JOINED: f32 = 4.0;
/// The widest a balloon gets.
pub const MAX_WIDTH: f32 = 460.0;
/// A sender's picture beside their balloons in a group.
pub const AVATAR: f32 = 32.0;
/// A sticker's box.
pub const STICKER: f32 = 168.0;
const REACTION_H: f32 = 24.0;

/// The colours of a conversation.
#[derive(Clone)]
pub struct Look {
    pub ground: Color32,
    pub theirs: Color32,
    pub theirs_edge: Color32,
    pub ours: Color32,
    pub ours_edge: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub link: Color32,
    pub accent: Color32,
    pub pill: Color32,
    pub pill_text: Color32,
    pub dark: bool,
}

impl Look {
    pub fn of(p: &olo_theme::Palette) -> Look {
        let ink = style::card_ink(p);
        let dark = p.mode == olo_theme::ThemeMode::Dark;
        let card = [ink.card.r(), ink.card.g(), ink.card.b(), 255];
        let ours = olo_theme::mix(card, p.keyboard_navigation, if dark { 0.30 } else { 0.15 });
        let link = olo_theme::mix(p.keyboard_navigation, p.document_text, if dark { 0.0 } else { 0.15 });
        Look {
            ground: ink.gutter,
            theirs: ink.card,
            theirs_edge: ink.edge,
            ours: style::c(ours),
            ours_edge: style::c(olo_theme::mix(ours, p.keyboard_navigation, 0.35)),
            text: ink.text,
            dim: ink.dim,
            link: style::c(link),
            accent: style::c(p.keyboard_navigation),
            pill: style::c(olo_theme::mix(p.document_text, p.document_background, if dark { 0.75 } else { 0.62 })).gamma_multiply(0.85),
            pill_text: if dark { Color32::from_gray(235) } else { Color32::WHITE },
            dark,
        }
    }
}

/// Telegram's seven colours for people and chats (names in groups,
/// pictures that are initials): bright for fills, and a readable ink.
pub fn peer_color(id: i64, dark: bool) -> (Color32, Color32) {
    const FILL: [[u8; 3]; 7] = [[0xFF, 0x84, 0x5E], [0xFE, 0xBB, 0x5B], [0xB6, 0x94, 0xF9], [0x9A, 0xD1, 0x64], [0x5B, 0xCB, 0xE3], [0x5C, 0xAF, 0xFA], [0xFF, 0x8A, 0xAC]];
    const INK_LIGHT: [[u8; 3]; 7] = [[0xCC, 0x50, 0x49], [0xD6, 0x77, 0x22], [0x95, 0x5C, 0xDB], [0x40, 0xA9, 0x20], [0x30, 0x9E, 0xBA], [0x36, 0x8A, 0xD1], [0xC7, 0x50, 0x8B]];
    const INK_DARK: [[u8; 3]; 7] = [[0xFF, 0x8E, 0x86], [0xFF, 0xA3, 0x57], [0xB1, 0x8F, 0xFF], [0x7C, 0xD6, 0x5F], [0x45, 0xD8, 0xE8], [0x7A, 0xC9, 0xFF], [0xFF, 0x7F, 0xD5]];
    let i = (id.unsigned_abs() % 7) as usize;
    let rgb = |c: [u8; 3]| Color32::from_rgb(c[0], c[1], c[2]);
    (rgb(FILL[i]), rgb(if dark { INK_DARK[i] } else { INK_LIGHT[i] }))
}

/// A person's or chat's picture: their photo when it's here, else their
/// initials on their colour, round.
pub fn avatar(p: &Painter, r: Rect, id: i64, name: &str, tex: Option<&egui::TextureHandle>, dark: bool) {
    let r = style::snap(p.ctx(), r);
    if let Some(t) = tex {
        olo_kit::icons::paint_round(p, r, t, Color32::from_black_alpha(30));
        return;
    }
    let (fill, _) = peer_color(id, dark);
    let c = r.center();
    let radius = r.width().min(r.height()) / 2.0;
    // Lit from above: a lighter top half of the same colour.
    p.circle_filled(c, radius, fill);
    let top = Color32::from_rgba_unmultiplied(255, 255, 255, 28);
    p.add(egui::Shape::convex_polygon(
        (0..=32).map(|i| c + egui::Vec2::angled(std::f32::consts::PI + i as f32 / 32.0 * std::f32::consts::PI) * radius).collect(),
        top,
        Stroke::NONE,
    ));
    let letters = initials(name);
    let size = (r.height() * if letters.chars().count() > 1 { 0.38 } else { 0.44 }).round();
    style::label(p, c.x, c.y, egui::Align::Center, letters, FontId::new(size, style::bold()), Color32::WHITE);
}

/// "MO" for "Mira Okafor", "D" for "Design".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().filter(|w| w.chars().next().is_some_and(char::is_alphanumeric)).collect();
    let first = |w: &str| w.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    match words.as_slice() {
        [] => name.chars().find(|c| !c.is_whitespace()).map(|c| c.to_string()).unwrap_or_default(),
        [one] => first(one),
        [a, .., b] => format!("{}{}", first(a), first(b)),
    }
}

/// How a message sits with its neighbours.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Place {
    /// The first of a run from one sender (shows their name).
    pub first: bool,
    /// The last of the run (shows their picture; full corners).
    pub last: bool,
    /// In a group or forum: names and pictures shown.
    pub many: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Bubble,
    /// A sticker or emoji alone: no balloon.
    Bare,
    Service,
}

/// What a part of a message does when clicked.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Reply(i32),
    Media,
    Reaction(String),
    Link(String),
    Mention(String),
    MentionId(i64),
    Hashtag(String),
    Spoiler,
    Sender(i64),
}

/// A message laid out at a width.
pub struct Laid {
    pub shape: Shape,
    /// Its whole height in the list (without the gap after it).
    pub height: f32,
    /// The balloon (or the bare thing), from the column's left.
    pub bubble: Rect,
    pub name: Option<(Arc<Galley>, Pos2)>,
    pub forward: Option<(Arc<Galley>, Pos2)>,
    /// The quote: its box, the name and the text.
    pub reply: Option<(Rect, Arc<Galley>, Arc<Galley>)>,
    pub media: Option<Rect>,
    /// File, voice and audio rows: their title and detail.
    pub media_text: Option<(Arc<Galley>, Arc<Galley>)>,
    /// A web page's preview: site, title, description.
    pub page: Option<(Rect, Vec<(Arc<Galley>, Pos2)>)>,
    pub poll: Option<Vec<(Arc<Galley>, Arc<Galley>, f32, Rect)>>,
    pub text: Option<(Arc<Galley>, Pos2, Vec<String>)>,
    pub meta: (Arc<Galley>, Pos2),
    /// The time sits on the picture (no text under it).
    pub meta_over: bool,
    pub reactions: Vec<(Rect, String, Arc<Galley>, bool)>,
    pub big_emoji: Option<(Arc<Galley>, Pos2, Vec<String>)>,
    pub avatar: bool,
}

fn galley(ctx: &egui::Context, job: LayoutJob) -> Arc<Galley> {
    ctx.fonts_mut(|f| f.layout_job(job))
}

fn plain(ctx: &egui::Context, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple(text.to_owned(), font, color, width);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let (job, _) = emoji::make_room(&job);
    galley(ctx, job)
}

/// The text with its formatting, as a layout job.
pub fn text_job(m: &Message, look: &Look, width: f32, revealed: bool) -> LayoutJob {
    let text = &m.text;
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for e in &m.entities {
        cuts.push(e.start.min(text.len()));
        cuts.push(e.end.min(text.len()));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut job = LayoutJob { wrap: egui::text::TextWrapping { max_width: width, ..Default::default() }, ..Default::default() };
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a >= b || !text.is_char_boundary(a) || !text.is_char_boundary(b) {
            continue;
        }
        let mut fmt = egui::TextFormat { font_id: FontId::proportional(TEXT), color: look.text, ..Default::default() };
        let mut bold = false;
        for e in m.entities.iter().filter(|e| e.start <= a && e.end >= b) {
            match &e.kind {
                EntityKind::Bold => bold = true,
                EntityKind::Italic | EntityKind::Quote => fmt.font_id.family = style::italic(),
                EntityKind::Underline => fmt.underline = Stroke::new(1.0, look.text),
                EntityKind::Strike => fmt.strikethrough = Stroke::new(1.0, look.text),
                EntityKind::Code => {
                    fmt.font_id = FontId::monospace(TEXT - 1.0);
                    fmt.background = look.text.gamma_multiply(0.07);
                }
                // A block: one box behind its rows (`pre_boxes`).
                EntityKind::Pre(_) => fmt.font_id = FontId::monospace(TEXT - 1.0),
                EntityKind::Spoiler if !revealed => {
                    fmt.color = Color32::TRANSPARENT;
                    fmt.background = look.dim;
                }
                k if k.is_link() => fmt.color = look.link,
                _ => {}
            }
        }
        if bold && fmt.font_id.family == FontFamily::Proportional {
            fmt.font_id.family = style::bold();
        }
        job.append(&text[a..b], 0.0, fmt);
    }
    job
}

/// The time, "edited", views: as it reads at the bottom of a balloon.
fn meta_text(m: &Message, time: &str) -> String {
    let mut s = String::new();
    if let Some(v) = m.views {
        s.push_str(&short_count(v));
        s.push_str("  ");
    }
    if m.edited.is_some() {
        s.push_str("edited ");
    }
    s.push_str(time);
    s
}

/// "9.3K", "1.2M".
pub fn short_count(n: u32) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..10_000 => format!("{:.1}K", n as f32 / 1000.0).replace(".0K", "K"),
        10_000..1_000_000 => format!("{}K", n / 1000),
        _ => format!("{:.1}M", n as f32 / 1e6).replace(".0M", "M"),
    }
}

/// Room after the time for the read ticks (ours).
const TICKS: f32 = 16.0;

/// A message alone of one to three emoji, shown large.
fn big_emoji(m: &Message) -> bool {
    let t = m.text.trim();
    if m.media.is_some() || t.is_empty() || t.chars().count() > 24 {
        return false;
    }
    let found = emoji::clusters(t);
    let covered: usize = found.iter().map(|r| r.len()).sum();
    !found.is_empty() && found.len() <= 3 && covered == t.chars().filter(|c| !c.is_whitespace()).map(char::len_utf8).sum::<usize>()
}

/// Lay out `m` for a column `col` wide.
#[allow(clippy::too_many_arguments)]
pub fn lay(ctx: &egui::Context, m: &Message, place: Place, look: &Look, col: f32, time: &str, sender_name: Option<&str>, revealed: bool) -> Laid {
    let small = FontId::proportional(META);
    let ours = m.out;
    let left_room = if place.many && !ours { AVATAR + 8.0 } else { 0.0 };
    let max_w = (col - left_room - 48.0).clamp(120.0, MAX_WIDTH);
    let meta_g = plain(ctx, &meta_text(m, time), small.clone(), look.dim, f32::INFINITY);
    let meta_w = meta_g.size().x + if ours { TICKS + 2.0 } else { 0.0 };

    // A service message: a pill in the middle.
    if let Some(s) = &m.service {
        let g = {
            let mut job = LayoutJob::simple(s.clone(), FontId::proportional(META + 0.5), look.pill_text, (col - 80.0).max(100.0));
            job.halign = egui::Align::Center;
            galley(ctx, job)
        };
        let size = g.size() + vec2(20.0, 8.0);
        let r = Rect::from_center_size(pos2(col / 2.0, size.y / 2.0), size);
        return Laid {
            shape: Shape::Service,
            height: size.y,
            bubble: r,
            name: None,
            forward: None,
            reply: None,
            media: None,
            media_text: None,
            page: None,
            poll: None,
            text: Some((g, pos2(r.center().x, r.top() + 4.0), Vec::new())),
            meta: (meta_g, Pos2::ZERO),
            meta_over: false,
            reactions: Vec::new(),
            big_emoji: None,
            avatar: false,
        };
    }

    let bare_sticker = matches!(m.media, Some(Media::Sticker { .. }) | Some(Media::Dice { .. })) && m.text.is_empty() && m.reply_to.is_none() && m.forwarded.is_none();
    if bare_sticker || big_emoji(m) {
        let (content, big) = if bare_sticker {
            let (w, h) = match m.media {
                Some(Media::Sticker { w, h, .. }) => (w.max(1) as f32, h.max(1) as f32),
                _ => (1.0, 1.0),
            };
            let k = (STICKER / w).min(STICKER / h);
            (vec2(w * k, h * k), None)
        } else {
            let n = emoji::clusters(m.text.trim()).len();
            let size = [48.0, 40.0, 32.0][n.clamp(1, 3) - 1];
            let job = LayoutJob::simple_singleline(m.text.trim().to_owned(), FontId::proportional(size), look.text);
            let (job, found) = emoji::make_room(&job);
            let g = galley(ctx, job);
            (g.size(), Some((g, found)))
        };
        let meta_size = meta_g.size() + vec2(12.0 + if ours { TICKS } else { 0.0 }, 4.0);
        let h = content.y + 4.0 + meta_size.y;
        let x = if ours { col - content.x.max(meta_size.x) } else { left_room };
        let bubble = Rect::from_min_size(pos2(x, 0.0), vec2(content.x.max(meta_size.x), h));
        let media = Rect::from_min_size(pos2(if ours { bubble.right() - content.x } else { x }, 0.0), content);
        let meta_pos = pos2(if ours { bubble.right() - meta_size.x } else { x }, content.y + 4.0);
        let reactions = lay_reactions(ctx, m, look, bubble.left(), h + 4.0, MAX_WIDTH);
        let height = reactions.last().map_or(h, |r| r.0.bottom());
        return Laid {
            shape: Shape::Bare,
            height,
            bubble,
            name: None,
            forward: None,
            reply: None,
            media: big.is_none().then_some(media),
            media_text: None,
            page: None,
            poll: None,
            text: None,
            meta: (meta_g, meta_pos),
            meta_over: true,
            reactions,
            big_emoji: big.map(|(g, f)| (g, media.min, f)),
            avatar: place.many && !ours && place.last,
        };
    }

    // A balloon: its parts top to bottom, at widths up to `inner`.
    let inner = max_w - 2.0 * PAD.x;
    let mut y = PAD.y;
    let mut width: f32 = 0.0;
    let name = (place.many && !ours && place.first)
        .then(|| sender_name.map(|n| {
            let ink = peer_color(m.from.as_ref().map_or(0, |f| f.id), look.dark).1;
            plain(ctx, n, FontId::new(META + 1.0, style::bold()), ink, inner)
        }))
        .flatten()
        .map(|g| {
            let p = pos2(PAD.x, y);
            y += g.size().y + 2.0;
            width = width.max(g.size().x);
            (g, p)
        });
    let forward = m.forwarded.as_ref().map(|f| {
        let g = plain(ctx, &format!("Forwarded from {f}"), FontId::new(META + 0.5, style::bold()), look.link, inner);
        let p = pos2(PAD.x, y);
        y += g.size().y + 3.0;
        width = width.max(g.size().x);
        (g, p)
    });
    let reply = m.reply_to.as_ref().map(|r| {
        let who = r.from.clone().unwrap_or_else(|| "Message".into());
        let ink = look.link;
        let ng = plain(ctx, &who, FontId::new(META + 0.5, style::bold()), ink, inner - 12.0);
        let tg = plain(ctx, r.text.as_deref().unwrap_or("…"), FontId::proportional(META + 1.0), look.text, inner - 12.0);
        let w = ng.size().x.max(tg.size().x) + 14.0;
        let h = ng.size().y + tg.size().y + 8.0;
        let rect = Rect::from_min_size(pos2(PAD.x, y), vec2(w, h));
        y += h + 5.0;
        width = width.max(w);
        (rect, ng, tg)
    });

    // Media.
    let mut media = None;
    let mut media_text = None;
    let mut page = None;
    let mut poll = None;
    let mut photo_top = false;
    match &m.media {
        Some(Media::Photo { w, h, .. }) | Some(Media::Video { w, h, .. }) => {
            let (w, h) = ((*w).max(1) as f32, (*h).max(1) as f32);
            let round = matches!(m.media, Some(Media::Video { round: true, .. }));
            let most = if round { vec2(220.0, 220.0) } else { vec2(max_w.min(400.0), 360.0) };
            let k = (most.x / w).min(most.y / h);
            let size = vec2((w * k).max(140.0).min(most.x), (h * k).max(80.0).min(most.y));
            // A picture opens the balloon edge to edge when it leads.
            if name.is_none() && forward.is_none() && reply.is_none() {
                photo_top = true;
                media = Some(Rect::from_min_size(pos2(0.0, 0.0), size));
                y = size.y + PAD.y;
            } else {
                media = Some(Rect::from_min_size(pos2(PAD.x, y), size));
                y += size.y + PAD.y;
            }
            width = width.max(size.x - if photo_top { 2.0 * PAD.x } else { 0.0 });
        }
        Some(Media::Voice { duration, .. }) => {
            let r = Rect::from_min_size(pos2(PAD.x, y), vec2(inner.min(240.0), 40.0));
            media_text = Some((plain(ctx, "", small.clone(), look.text, 10.0), plain(ctx, &olo_tg::clock(*duration), small.clone(), look.dim, 100.0)));
            media = Some(r);
            y += r.height() + 4.0;
            width = width.max(r.width());
        }
        Some(Media::Audio { size, .. }) | Some(Media::File { size, .. }) => {
            let label = m.media.as_ref().map(Media::label).unwrap_or_default();
            let detail = match &m.media {
                Some(Media::Audio { duration, .. }) => format!("{} · {}", olo_tg::clock(*duration), olo_tg::bytes(*size)),
                _ => olo_tg::bytes(*size),
            };
            let tg = plain(ctx, &label, FontId::new(TEXT - 0.5, style::bold()), look.text, inner - 52.0);
            let dg = plain(ctx, &detail, small.clone(), look.dim, inner - 52.0);
            let w = 52.0 + tg.size().x.max(dg.size().x);
            let r = Rect::from_min_size(pos2(PAD.x, y), vec2(w.min(inner), 42.0));
            media_text = Some((tg, dg));
            media = Some(r);
            y += r.height() + 4.0;
            width = width.max(r.width());
        }
        Some(Media::WebPage { site, title, description, .. }) => {
            // After the text: laid out below, once the text is.
            page = Some((site.clone(), title.clone(), description.clone()));
        }
        Some(Media::Poll { question, answers, voters, closed, quiz }) => {
            let qg = galley(ctx, LayoutJob::simple(question.clone(), FontId::new(TEXT, style::bold()), look.text, inner));
            let mut rows = Vec::new();
            let total: u32 = answers.iter().map(|a| a.1).sum::<u32>().max(1);
            let mut py = y + qg.size().y + 4.0;
            let kind = format!("{}{} · {} votes", if *closed { "Final results · " } else { "" }, if *quiz { "Quiz" } else { "Poll" }, voters);
            let kg = plain(ctx, &kind, small.clone(), look.dim, inner);
            py += kg.size().y + 6.0;
            for (text, votes) in answers {
                let share = *votes as f32 / total as f32;
                let tg = plain(ctx, text, FontId::proportional(TEXT - 0.5), look.text, inner - 44.0);
                let pg = plain(ctx, &format!("{:.0}%", share * 100.0), FontId::new(META + 0.5, style::bold()), look.text, 40.0);
                let row = Rect::from_min_size(pos2(PAD.x, py), vec2(inner.min(300.0), tg.size().y + 10.0));
                py = row.bottom() + 4.0;
                rows.push((tg, pg, share, row));
            }
            rows.insert(0, (qg, kg, -1.0, Rect::from_min_size(pos2(PAD.x, y), vec2(inner, 0.0))));
            y = py + 2.0;
            width = width.max(inner.min(300.0));
            poll = Some(rows);
        }
        Some(other) if !matches!(other, Media::Sticker { .. }) => {
            let (label, detail) = match other {
                Media::Location { title, address, lat, lon } => (format!("📍 {}", title.clone().unwrap_or_else(|| "Location".into())), address.clone().unwrap_or_else(|| format!("{lat:.5}, {lon:.5}"))),
                Media::Contact { name, phone } => (format!("👤 {name}"), phone.clone()),
                Media::Dice { emoji, value } => (emoji.clone(), format!("rolled {value}")),
                other => (other.label(), String::new()),
            };
            let tg = plain(ctx, &label, FontId::new(TEXT - 0.5, style::bold()), look.text, inner);
            let dg = plain(ctx, &detail, small.clone(), look.dim, inner);
            let r = Rect::from_min_size(pos2(PAD.x, y), vec2(tg.size().x.max(dg.size().x).min(inner), tg.size().y + dg.size().y + 2.0));
            media_text = Some((tg, dg));
            media = Some(r);
            y += r.height() + 4.0;
            width = width.max(r.width());
        }
        Some(Media::Sticker { w, h, .. }) => {
            let (w, h) = ((*w).max(1) as f32, (*h).max(1) as f32);
            let k = (120.0 / w).min(120.0 / h);
            let r = Rect::from_min_size(pos2(PAD.x, y), vec2(w * k, h * k));
            media = Some(r);
            y += r.height() + 4.0;
            width = width.max(r.width());
        }
        _ => {}
    }

    // The text, with the time after its last line if there's room.
    let mut text = None;
    let mut meta_pos;
    let mut meta_over = false;
    if !m.text.is_empty() {
        let job = text_job(m, look, inner, revealed);
        let (job, found) = emoji::make_room(&job);
        let g = galley(ctx, job);
        let last_w = g.rows.last().map_or(0.0, |r| r.rect().right());
        let fits_inline = last_w + 10.0 + meta_w <= inner;
        let tw = g.size().x;
        width = width.max(tw);
        let tp = pos2(PAD.x, y);
        y += g.size().y;
        if fits_inline {
            width = width.max(last_w + 10.0 + meta_w);
            // On the last line's baseline.
            meta_pos = pos2(f32::NAN, tp.y + last_baseline(&g) - style::baseline_offset(&meta_g));
        } else {
            meta_pos = pos2(f32::NAN, y + 2.0);
            y += meta_g.size().y + 2.0;
        }
        text = Some((g, tp, found));
    } else if photo_top && page.is_none() && poll.is_none() {
        // The time on the picture itself.
        meta_over = true;
        meta_pos = pos2(f32::NAN, y - PAD.y - meta_g.size().y - 8.0);
        y -= PAD.y;
    } else {
        width = width.max(meta_w);
        meta_pos = pos2(f32::NAN, y);
        y += meta_g.size().y;
    }

    // A web page's preview, under the text.
    let page = page.map(|(site, title, description)| {
        y += 6.0;
        let top = y;
        let mut parts = Vec::new();
        let x = PAD.x + 10.0;
        let w = inner - 10.0;
        for (t, font, ink, rows) in [
            (site, FontId::new(META + 0.5, style::bold()), look.link, 1),
            (title, FontId::new(TEXT - 0.5, style::bold()), look.text, 2),
            (description, FontId::proportional(TEXT - 1.0), look.text, 4),
        ] {
            let Some(t) = t else { continue };
            let mut job = LayoutJob::simple(t, font, ink, w);
            job.wrap.max_rows = rows;
            let g = galley(ctx, job);
            parts.push((g.clone(), pos2(x, y)));
            y += g.size().y + 1.0;
            width = width.max(g.size().x + 10.0);
        }
        let r = Rect::from_min_max(pos2(PAD.x, top), pos2(PAD.x + 3.0, y));
        y += 4.0;
        meta_pos = pos2(f32::NAN, y);
        y += meta_g.size().y;
        (r, parts)
    });

    let w = (width + 2.0 * PAD.x).max(meta_w + 2.0 * PAD.x + 8.0).min(max_w);
    let w = match media {
        Some(r) if photo_top => w.max(r.width()),
        _ => w,
    };
    let h = y + if meta_over { 0.0 } else { PAD.y };
    let x = if ours { col - w } else { left_room };
    let bubble = Rect::from_min_size(pos2(x, 0.0), vec2(w, h));
    meta_pos.x = bubble.right() - PAD.x - meta_w + if meta_over { -6.0 } else { 0.0 };
    let shift = vec2(x, 0.0);
    let reactions = lay_reactions(ctx, m, look, bubble.left(), bubble.bottom() + 4.0, max_w);
    let height = reactions.last().map_or(bubble.bottom(), |r| r.0.bottom());
    Laid {
        shape: Shape::Bubble,
        height,
        bubble,
        name: name.map(|(g, p)| (g, p + shift)),
        forward: forward.map(|(g, p)| (g, p + shift)),
        reply: reply.map(|(r, a, b)| (r.translate(shift), a, b)),
        media: media.map(|r| if photo_top { Rect::from_min_size(r.min + shift, vec2(w, r.height())) } else { r.translate(shift) }),
        media_text,
        page: page.map(|(r, parts)| (r.translate(shift), parts.into_iter().map(|(g, p)| (g, p + shift)).collect())),
        poll: poll.map(|rows| rows.into_iter().map(|(a, b, s, r)| (a, b, s, r.translate(shift))).collect()),
        text: text.map(|(g, p, f)| (g, p + shift, f)),
        meta: (meta_g, pos2(meta_pos.x, meta_pos.y)),
        meta_over,
        reactions,
        big_emoji: None,
        avatar: place.many && !ours && place.last,
    }
}

fn lay_reactions(ctx: &egui::Context, m: &Message, look: &Look, x: f32, y: f32, max_w: f32) -> Vec<(Rect, String, Arc<Galley>, bool)> {
    let mut out = Vec::new();
    let (mut cx, mut cy) = (x, y);
    for r in &m.reactions {
        let g = plain(ctx, &short_count(r.count), FontId::new(META + 0.5, style::bold()), if r.mine { Color32::WHITE } else { look.link }, 60.0);
        let w = 8.0 + 18.0 + 4.0 + g.size().x + 8.0;
        if cx + w > x + max_w && cx > x {
            cx = x;
            cy += REACTION_H + 4.0;
        }
        out.push((Rect::from_min_size(pos2(cx, cy), vec2(w, REACTION_H)), r.emoji.clone(), g, r.mine));
        cx += w + 4.0;
    }
    out
}

/// The balloon's corners: rounded fully, but a little where it joins
/// the next or previous of the same sender's.
fn corners(place: Place, ours: bool) -> egui::CornerRadius {
    let (r, j) = (RADIUS as u8, JOINED as u8);
    let (top, bottom) = (if place.first { r } else { j }, if place.last { r } else { j });
    if ours { egui::CornerRadius { nw: r, ne: top, sw: r, se: bottom } } else { egui::CornerRadius { nw: top, ne: r, sw: bottom, se: r } }
}

/// What the painter needs besides the layout.
pub struct Paint<'a> {
    pub look: &'a Look,
    pub place: Place,
    /// The message's picture, when it's here.
    pub picture: Option<&'a egui::TextureHandle>,
    /// The sender's picture (groups), when it's here.
    pub sender_pic: Option<&'a egui::TextureHandle>,
    /// How far a download or upload has got.
    pub progress: Option<f32>,
    /// Ours, read by them.
    pub read: bool,
    /// A voice message playing: how far.
    pub playing: Option<f32>,
    pub selected: bool,
}

/// Paint `laid` with its top-left at `o`; what's under the pointer.
pub fn paint(p: &Painter, o: Pos2, m: &Message, laid: &Laid, pp: &Paint, pointer: Option<Pos2>) -> Option<Hit> {
    let look = pp.look;
    let at = |r: Rect| r.translate(o.to_vec2());
    let mut areas: Vec<(Rect, Hit)> = Vec::new();
    let mut over = |r: Rect, h: Hit| areas.push((r, h));
    let mut link = None;
    let bubble = at(laid.bubble);
    match laid.shape {
        Shape::Service => {
            p.rect_filled(style::snap(p.ctx(), bubble), bubble.height() / 2.0, look.pill);
            if let Some((g, pos, _)) = &laid.text {
                p.galley(pos2(o.x + pos.x - g.size().x / 2.0, o.y + pos.y), g.clone(), look.pill_text);
            }
            return None;
        }
        Shape::Bare => {
            if let Some(r) = laid.media {
                let r = at(r);
                match pp.picture {
                    Some(t) => {
                        p.image(t.id(), style::snap(p.ctx(), r), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                    None => {
                        let label = match &m.media {
                            Some(Media::Sticker { emoji, .. }) | Some(Media::Dice { emoji, .. }) => emoji.clone(),
                            _ => String::new(),
                        };
                        if !emoji::paint(p, r.shrink(r.width() * 0.2), &label) {
                            p.rect_filled(r, 12.0, look.theirs.gamma_multiply(0.6));
                        }
                    }
                }
                over(r, Hit::Media);
            }
            if let Some((g, pos, found)) = &laid.big_emoji {
                let pos = o + pos.to_vec2();
                p.galley(pos, g.clone(), look.text);
                emoji::paint_in(p, pos, g, found, look.text);
            }
            let (g, mp) = &laid.meta;
            let mr = Rect::from_min_size(o + mp.to_vec2(), g.size() + vec2(12.0 + if m.out { TICKS } else { 0.0 }, 4.0));
            p.rect_filled(style::snap(p.ctx(), mr), mr.height() / 2.0, look.pill);
            p.galley(mr.min + vec2(6.0, 2.0), g.clone(), look.pill_text);
            if m.out {
                ticks(p, pos2(mr.right() - 6.0 - TICKS + 2.0, mr.center().y), pp.read, m.pending, look.pill_text);
            }
            paint_reactions(p, o, laid, look, &mut over);
            return under(&areas, pointer);
        }
        Shape::Bubble => {}
    }

    // The sender's picture beside the last of their run.
    if laid.avatar
        && let Some(f) = &m.from
    {
        let r = Rect::from_min_size(pos2(o.x, bubble.bottom() - AVATAR), vec2(AVATAR, AVATAR));
        avatar(p, r, f.id, &f.name, pp.sender_pic, look.dark);
        over(r, Hit::Sender(f.id));
    }
    let (fill, edge) = if m.out { (look.ours, look.ours_edge) } else { (look.theirs, look.theirs_edge) };
    let corners = corners(pp.place, m.out);
    let br = style::snap(p.ctx(), bubble);
    // A soft lift, as a card's: a whole device pixel or two down.
    let ppp = p.ctx().pixels_per_point();
    p.rect_filled(br.translate(vec2(0.0, ppp.round().max(1.0) / ppp)), corners, Color32::from_black_alpha(if look.dark { 50 } else { 14 }));
    p.rect(br, corners, if pp.selected { fill.lerp_to_gamma(look.accent, 0.25) } else { fill }, Stroke::new(style::hairline(p.ctx()), edge), egui::StrokeKind::Inside);

    if let Some((g, pos)) = &laid.name {
        let r = Rect::from_min_size(o + pos.to_vec2(), g.size());
        p.galley(r.min, g.clone(), look.text);
        if let Some(f) = &m.from {
            over(r, Hit::Sender(f.id));
        }
    }
    if let Some((g, pos)) = &laid.forward {
        p.galley(o + pos.to_vec2(), g.clone(), look.link);
    }
    if let Some((r, ng, tg)) = &laid.reply {
        let r = at(*r);
        p.rect_filled(style::snap(p.ctx(), r), 4.0, look.link.gamma_multiply(0.10));
        p.rect_filled(style::snap(p.ctx(), Rect::from_min_size(r.min, vec2(3.0, r.height()))), egui::CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 }, look.link);
        p.galley(r.min + vec2(9.0, 3.0), ng.clone(), look.link);
        p.galley(r.min + vec2(9.0, 4.0 + ng.size().y), tg.clone(), look.text);
        if let Some(rt) = &m.reply_to {
            over(r, Hit::Reply(rt.id));
        }
    }
    if let Some(r) = laid.media {
        let r = at(r);
        paint_media(p, r, m, laid, pp, corners);
        over(r, Hit::Media);
    }
    if let Some((r, parts)) = &laid.page {
        let r = at(*r);
        p.rect_filled(style::snap(p.ctx(), r), 1.5, look.link);
        for (g, pos) in parts {
            p.galley(o + pos.to_vec2(), g.clone(), look.text);
        }
        if let Some(Media::WebPage { url, .. }) = &m.media {
            let whole = parts.iter().fold(r, |acc, (g, pos)| acc.union(Rect::from_min_size(o + pos.to_vec2(), g.size())));
            over(whole, Hit::Link(url.clone()));
        }
    }
    if let Some(rows) = &laid.poll {
        for (i, (a, b, share, r)) in rows.iter().enumerate() {
            let r = at(*r);
            if i == 0 {
                p.galley(r.min, a.clone(), look.text);
                p.galley(r.min + vec2(0.0, a.size().y + 2.0), b.clone(), look.dim);
                continue;
            }
            p.galley(r.min + vec2(40.0, 0.0), a.clone(), look.text);
            p.galley(pos2(r.left() + 34.0 - b.size().x, r.top()), b.clone(), look.text);
            let bar = Rect::from_min_size(pos2(r.left() + 40.0, r.bottom() - 5.0), vec2((r.width() - 40.0) * share.max(0.02), 3.0));
            p.rect_filled(style::snap(p.ctx(), bar), 1.5, look.link);
        }
    }
    if let Some((g, pos, found)) = &laid.text {
        let pos = o + pos.to_vec2();
        for r in pre_boxes(m, g) {
            let r = Rect::from_min_max(pos2(pos.x - 4.0, pos.y + r.top() - 2.0), pos2(pos.x + g.size().x + 4.0, pos.y + r.bottom() + 2.0));
            p.rect_filled(style::snap(p.ctx(), r), 6.0, look.text.gamma_multiply(0.06));
        }
        p.galley(pos, g.clone(), look.text);
        emoji::paint_in(p, pos, g, found, look.text);
        if let Some(q) = pointer {
            let r = Rect::from_min_size(pos, g.size());
            if r.contains(q) {
                let c = g.cursor_from_pos(q - pos);
                let byte = m.text.char_indices().nth(c.index.0).map_or(m.text.len(), |(b, _)| b);
                if let Some(e) = m.entities.iter().find(|e| e.start <= byte && byte < e.end) {
                    let word = m.text.get(e.start..e.end).unwrap_or("").to_owned();
                    let h = match &e.kind {
                        EntityKind::Url => Some(Hit::Link(word)),
                        EntityKind::Email => Some(Hit::Link(format!("mailto:{word}"))),
                        EntityKind::Phone => Some(Hit::Link(format!("tel:{word}"))),
                        EntityKind::TextUrl(u) => Some(Hit::Link(u.clone())),
                        EntityKind::Mention => Some(Hit::Mention(word)),
                        EntityKind::MentionName(id) => Some(Hit::MentionId(*id)),
                        EntityKind::Hashtag | EntityKind::Cashtag | EntityKind::BotCommand => Some(Hit::Hashtag(word)),
                        EntityKind::Spoiler => Some(Hit::Spoiler),
                        _ => None,
                    };
                    if let Some(h) = h {
                        link = Some(h);
                    }
                }
            }
        }
    }
    let (g, mp) = &laid.meta;
    let mp = o + mp.to_vec2();
    let ppp = p.ctx().pixels_per_point();
    let mp = pos2((mp.x * ppp).round() / ppp, (mp.y * ppp).round() / ppp);
    let ink = if laid.meta_over { Color32::WHITE } else if m.pending == Some((0, Pending::Failed)) { Color32::RED } else { look.dim };
    if laid.meta_over {
        let mr = Rect::from_min_size(mp - vec2(6.0, 2.0), g.size() + vec2(12.0 + if m.out { TICKS } else { 0.0 }, 4.0));
        p.rect_filled(style::snap(p.ctx(), mr), mr.height() / 2.0, Color32::from_black_alpha(110));
    }
    if m.views.is_some() {
        eye(p, pos2(mp.x - 9.0, mp.y + g.size().y / 2.0), ink);
    }
    p.galley(mp, g.clone(), ink);
    if m.out {
        let tick_ink = if laid.meta_over { Color32::WHITE } else if pp.read { look.link } else { look.dim };
        ticks(p, pos2(mp.x + g.size().x + 3.0, mp.y + g.size().y / 2.0), pp.read, m.pending, tick_ink);
    }
    paint_reactions(p, o, laid, look, &mut over);
    link.or_else(|| under(&areas, pointer))
}

/// Where a galley's last line has its baseline, from its top (as
/// `style::baseline_offset` has the first's).
fn last_baseline(g: &Galley) -> f32 {
    g.rows
        .last()
        .and_then(|r| r.row.glyphs.first().map(|gl| r.pos.y + gl.pos.y + gl.font_ascent - gl.font_face_ascent - 0.5 * (gl.font_height - gl.font_face_height)))
        .unwrap_or(g.size().y * 0.8)
}

/// The rows each code block (a `Pre` entity) takes in `g`, as one
/// rect (in the galley's coordinates) per block.
fn pre_boxes(m: &Message, g: &Galley) -> Vec<Rect> {
    let mut out = Vec::new();
    for e in m.entities.iter().filter(|e| matches!(e.kind, EntityKind::Pre(_))) {
        let chars = |b: usize| m.text.get(..b.min(m.text.len())).map_or(0, |t| t.chars().count());
        let (from, to) = (chars(e.start), chars(e.end));
        let mut at = 0;
        let mut rect: Option<Rect> = None;
        for row in &g.rows {
            let n = row.char_count_including_newline().0;
            if at + n > from && at < to {
                let r = row.rect();
                rect = Some(rect.map_or(r, |x| x.union(r)));
            }
            at += n;
        }
        out.extend(rect);
    }
    out
}

/// The last of `areas` under the pointer (later ones are on top).
fn under(areas: &[(Rect, Hit)], pointer: Option<Pos2>) -> Option<Hit> {
    let q = pointer?;
    areas.iter().rev().find(|(r, _)| r.contains(q)).map(|(_, h)| h.clone())
}

fn paint_reactions(p: &Painter, o: Pos2, laid: &Laid, look: &Look, over: &mut impl FnMut(Rect, Hit)) {
    for (r, e, g, mine) in &laid.reactions {
        let r = r.translate(o.to_vec2());
        let fill = if *mine { look.accent } else { look.link.gamma_multiply(0.14) };
        p.rect_filled(style::snap(p.ctx(), r), r.height() / 2.0, fill);
        let er = Rect::from_min_size(pos2(r.left() + 7.0, r.center().y - 9.0), vec2(18.0, 18.0));
        if !emoji::paint(p, er, e) {
            p.text(er.center(), Align2::CENTER_CENTER, e, FontId::proportional(13.0), look.text);
        }
        p.galley(pos2(er.right() + 4.0, r.center().y - g.size().y / 2.0), g.clone(), if *mine { Color32::WHITE } else { look.link });
        over(r, Hit::Reaction(e.clone()));
    }
}

fn paint_media(p: &Painter, r: Rect, m: &Message, laid: &Laid, pp: &Paint, corners: egui::CornerRadius) {
    let look = pp.look;
    match &m.media {
        Some(Media::Photo { .. }) | Some(Media::Video { .. }) => {
            let round = matches!(m.media, Some(Media::Video { round: true, .. }));
            let leads = laid.name.is_none() && laid.forward.is_none() && laid.reply.is_none();
            let mut c = if leads { egui::CornerRadius { sw: 0, se: 0, ..corners } } else { egui::CornerRadius::same(6) };
            if leads && laid.text.is_none() && laid.page.is_none() {
                c = corners;
            }
            let rr = style::snap(p.ctx(), r);
            match pp.picture {
                Some(t) if round => olo_kit::icons::paint_round(p, rr, t, Color32::TRANSPARENT),
                Some(t) => {
                    let k = (rr.width() / rr.height().max(1.0)) / (t.size_vec2().x / t.size_vec2().y.max(1.0));
                    let uv = if k > 1.0 {
                        let h = 1.0 / k;
                        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
                    } else {
                        Rect::from_min_max(pos2((1.0 - k) / 2.0, 0.0), pos2((1.0 + k) / 2.0, 1.0))
                    };
                    p.add(egui::epaint::RectShape::filled(rr, c, Color32::WHITE).with_texture(t.id(), uv));
                }
                None => {
                    p.rect_filled(rr, c, look.text.gamma_multiply(0.08));
                }
            }
            if let Some(Media::Video { duration, .. }) = &m.media {
                // A play button and the length.
                let cc = rr.center();
                p.circle_filled(cc, 22.0, Color32::from_black_alpha(120));
                let tri = vec![cc + vec2(-6.0, -9.0), cc + vec2(-6.0, 9.0), cc + vec2(10.0, 0.0)];
                p.add(egui::Shape::convex_polygon(tri, Color32::WHITE, Stroke::NONE));
                let t = olo_tg::clock(*duration);
                let g = plain(p.ctx(), &t, FontId::proportional(META), Color32::WHITE, 80.0);
                let tr = Rect::from_min_size(rr.min + vec2(6.0, 6.0), g.size() + vec2(10.0, 4.0));
                p.rect_filled(tr, tr.height() / 2.0, Color32::from_black_alpha(120));
                p.galley(tr.min + vec2(5.0, 2.0), g, Color32::WHITE);
            }
            if let Some(f) = pp.progress {
                ring(p, rr.center(), 20.0, f, Color32::WHITE);
            }
        }
        Some(Media::Voice { waveform, duration, .. }) => {
            let c = pos2(r.left() + 20.0, r.center().y);
            p.circle_filled(c, 18.0, look.accent);
            if pp.playing.is_some() {
                for dx in [-4.0, 4.0] {
                    p.rect_filled(Rect::from_center_size(c + vec2(dx, 0.0), vec2(4.0, 13.0)), 1.0, Color32::WHITE);
                }
            } else {
                let tri = vec![c + vec2(-5.0, -8.0), c + vec2(-5.0, 8.0), c + vec2(9.0, 0.0)];
                p.add(egui::Shape::convex_polygon(tri, Color32::WHITE, Stroke::NONE));
            }
            if let Some(f) = pp.progress {
                ring(p, c, 18.0, f, Color32::WHITE);
            }
            // The waveform: bars of its samples, the part played in the accent.
            let wr = Rect::from_min_max(pos2(r.left() + 46.0, r.top() + 4.0), pos2(r.right(), r.top() + 24.0));
            let n = ((wr.width() / 3.0) as usize).max(1);
            let played = pp.playing.unwrap_or(0.0);
            let ppp = p.ctx().pixels_per_point();
            for i in 0..n {
                let s = if waveform.is_empty() { 4 } else { waveform[i * waveform.len() / n] } as f32;
                let h = (2.0 + s / 31.0 * wr.height()).min(wr.height());
                let x = ((wr.left() + i as f32 * 3.0) * ppp).round() / ppp;
                let bar = Rect::from_min_max(pos2(x, wr.bottom() - h), pos2(x + 2.0, wr.bottom()));
                let ink = if (i as f32 / n as f32) < played { look.accent } else { look.accent.gamma_multiply(0.35) };
                p.rect_filled(bar, 1.0, ink);
            }
            let t = match pp.playing {
                Some(f) => olo_tg::clock(f * duration),
                None => olo_tg::clock(*duration),
            };
            let g = plain(p.ctx(), &t, FontId::proportional(META), look.dim, 100.0);
            p.galley(pos2(wr.left(), wr.bottom() + 3.0), g, look.dim);
        }
        Some(Media::Audio { .. }) | Some(Media::File { .. }) => {
            let icon = Rect::from_min_size(r.min, vec2(42.0, 42.0));
            p.circle_filled(icon.center(), 20.0, look.accent);
            match pp.progress {
                Some(f) => ring(p, icon.center(), 15.0, f, Color32::WHITE),
                None if matches!(m.media, Some(Media::Audio { .. })) => {
                    let c = icon.center();
                    let tri = vec![c + vec2(-5.0, -8.0), c + vec2(-5.0, 8.0), c + vec2(9.0, 0.0)];
                    p.add(egui::Shape::convex_polygon(tri, Color32::WHITE, Stroke::NONE));
                }
                None => document(p, icon.center(), Color32::WHITE),
            }
            if let Some((tg, dg)) = &laid.media_text {
                p.galley(pos2(r.left() + 52.0, r.top() + 3.0), tg.clone(), look.text);
                p.galley(pos2(r.left() + 52.0, r.top() + 5.0 + tg.size().y), dg.clone(), look.dim);
            }
        }
        Some(Media::Sticker { .. }) => match pp.picture {
            Some(t) => {
                p.image(t.id(), style::snap(p.ctx(), r), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                p.rect_filled(r, 8.0, look.text.gamma_multiply(0.06));
            }
        },
        _ => {
            if let Some((tg, dg)) = &laid.media_text {
                p.galley(r.min, tg.clone(), look.text);
                p.galley(pos2(r.left(), r.top() + tg.size().y + 2.0), dg.clone(), look.dim);
            }
        }
    }
}

/// One tick (sent) or two (read); a clock while sending, "!" if it failed.
fn ticks(p: &Painter, left_mid: Pos2, read: bool, pending: Option<(u64, Pending)>, ink: Color32) {
    let s = Stroke::new(1.3, ink);
    match pending {
        Some((_, Pending::Sending)) => {
            let c = left_mid + vec2(6.0, 0.0);
            p.circle_stroke(c, 4.5, s);
            p.line_segment([c, c + vec2(0.0, -3.0)], s);
            p.line_segment([c, c + vec2(2.0, 0.0)], s);
        }
        Some((_, Pending::Failed)) => {
            let c = left_mid + vec2(6.0, 0.0);
            p.circle_filled(c, 5.5, Color32::from_rgb(0xE5, 0x3E, 0x3E));
            p.text(c, Align2::CENTER_CENTER, "!", FontId::new(9.0, style::bold()), Color32::WHITE);
        }
        None => {
            let one = |x: f32| [left_mid + vec2(x, 0.0), left_mid + vec2(x + 3.0, 3.0), left_mid + vec2(x + 9.0, -3.5)];
            p.line_segment([one(0.0)[0], one(0.0)[1]], s);
            p.line_segment([one(0.0)[1], one(0.0)[2]], s);
            if read {
                p.line_segment([one(4.0)[1], one(4.0)[2]], s);
            }
        }
    }
}

/// A small eye (a channel post's views), centred at `c`.
fn eye(p: &Painter, c: Pos2, ink: Color32) {
    let s = Stroke::new(1.0, ink);
    let pts: Vec<Pos2> = (0..=12).map(|i| {
        let t = i as f32 / 12.0 * std::f32::consts::PI;
        c + vec2(-4.5 * t.cos(), -2.6 * t.sin())
    }).collect();
    p.add(egui::Shape::line(pts.clone(), s));
    p.add(egui::Shape::line(pts.iter().map(|q| pos2(q.x, 2.0 * c.y - q.y)).collect(), s));
    p.circle_filled(c, 1.4, ink);
}

/// A page with a folded corner (a file), centred at `c`.
fn document(p: &Painter, c: Pos2, ink: Color32) {
    let s = Stroke::new(1.5, ink);
    let r = Rect::from_center_size(c, vec2(12.0, 15.0));
    let fold = 4.0;
    let pts = vec![r.left_top(), pos2(r.right() - fold, r.top()), pos2(r.right(), r.top() + fold), r.right_bottom(), r.left_bottom(), r.left_top()];
    p.add(egui::Shape::line(pts, s));
    p.line_segment([pos2(r.right() - fold, r.top()), pos2(r.right() - fold, r.top() + fold)], s);
    p.line_segment([pos2(r.right() - fold, r.top() + fold), pos2(r.right(), r.top() + fold)], s);
}

/// A ring of progress round `c`.
pub fn ring(p: &Painter, c: Pos2, radius: f32, fraction: f32, ink: Color32) {
    let n = 48;
    let end = (fraction.clamp(0.02, 1.0) * n as f32) as usize;
    let pts: Vec<Pos2> = (0..=end).map(|i| {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 / n as f32 * std::f32::consts::TAU;
        c + egui::Vec2::angled(a) * radius
    }).collect();
    p.add(egui::Shape::line(pts, Stroke::new(2.0, ink)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_to_initials() {
        assert_eq!(initials("Mira Okafor"), "MO");
        assert_eq!(initials("Design Crew Team"), "DT");
        assert_eq!(initials("jun"), "J");
        assert_eq!(initials("🎉 Party"), "P");
        assert_eq!(short_count(18_204), "18K");
        assert_eq!(short_count(9_311), "9.3K");
        assert_eq!(short_count(2_000), "2K");
    }

    #[test]
    fn emoji_alone_is_big() {
        let m = |t: &str| Message { text: t.into(), ..Message::default() };
        // Without the colour font loaded, clusters are still found.
        assert!(big_emoji(&m("🎉")));
        assert!(big_emoji(&m("👍🏽 ❤\u{FE0F}")));
        assert!(!big_emoji(&m("hi 🎉")));
        assert!(!big_emoji(&m("🎉🎉🎉🎉")));
    }
}

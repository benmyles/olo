//! A person's picture where there's no photograph of them: their
//! initials on a colour of their own, round (a chat's row, a contact's
//! card, a conversation's header), and their photograph when there is
//! one. The colour is by an id (the same person the same colour
//! everywhere): Telegram's seven. (Telegram's, moved here when Phone
//! wanted the same.)

use std::sync::Arc;

use crate::{
    Bounds, Corners, FontWeight, Hsla, Painter, Pixels, RenderImage, hsla, px, rgb,
    style::{self, Align},
    white,
};

/// An id for [`color`] and [`picture`] from words (a number, an
/// address): the same words, the same colour.
pub fn id_of(words: &str) -> i64 {
    // (FNV-1a: stable from run to run, which the standard hasher isn't
    // promised to be.)
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in words.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    (h >> 1) as i64
}

/// Seven colours for people and chats (names in groups, pictures that
/// are initials): bright for fills, and a readable ink.
pub fn color(id: i64, dark: bool) -> (Hsla, Hsla) {
    const FILL: [u32; 7] = [0xFF845E, 0xFEBB5B, 0xB694F9, 0x9AD164, 0x5BCBE3, 0x5CAFFA, 0xFF8AAC];
    const INK_LIGHT: [u32; 7] = [0xCC5049, 0xD67722, 0x955CDB, 0x40A920, 0x309EBA, 0x368AD1, 0xC7508B];
    const INK_DARK: [u32; 7] = [0xFF8E86, 0xFFA357, 0xB18FFF, 0x7CD65F, 0x45D8E8, 0x7AC9FF, 0xFF7FD5];
    let i = (id.unsigned_abs() % 7) as usize;
    (rgb(FILL[i]).into(), rgb(if dark { INK_DARK[i] } else { INK_LIGHT[i] }).into())
}

/// A person's or chat's picture: their photo when it's here, else their
/// initials on their colour, round.
pub fn picture(gr: &mut Painter, r: Bounds<Pixels>, id: i64, name: &str, img: Option<&Arc<RenderImage>>, dark: bool) {
    let r = style::snap(gr, r);
    let radius = r.size.width.min(r.size.height) / 2.0;
    let c = r.center();
    if let Some(img) = img {
        let _ = gr.paint_image(r, Corners::all(radius), img.clone(), 0, false);
        let hair = style::hairline(gr);
        gr.paint_sector(c, radius - hair, radius, 0.0, std::f32::consts::TAU, hsla(0.0, 0.0, 0.0, 30.0 / 255.0));
        return;
    }
    let (fill, _) = color(id, dark);
    gr.paint_sector(c, px(0.0), radius, 0.0, std::f32::consts::TAU, fill);
    // Lit from above: a lighter top half of the same colour.
    gr.paint_sector(c, px(0.0), radius, 1.5 * std::f32::consts::PI, std::f32::consts::PI, hsla(0.0, 0.0, 1.0, 28.0 / 255.0));
    let letters = initials(name);
    let h = f32::from(r.size.height);
    let size = (h * if letters.chars().count() > 1 { 0.38 } else { 0.44 }).round();
    style::label(gr, c.x, c.y, Align::Center, letters, size, style::ui_font(FontWeight::BOLD), white());
}

/// Somebody with no name known (a number, an address): a head and
/// shoulders on their colour.
pub fn nobody(gr: &mut Painter, r: Bounds<Pixels>, id: i64, dark: bool) {
    let r = style::snap(gr, r);
    let radius = r.size.width.min(r.size.height) / 2.0;
    let c = r.center();
    let (fill, _) = color(id, dark);
    gr.paint_sector(c, px(0.0), radius, 0.0, std::f32::consts::TAU, fill);
    gr.paint_sector(c, px(0.0), radius, 1.5 * std::f32::consts::PI, std::f32::consts::PI, hsla(0.0, 0.0, 1.0, 28.0 / 255.0));
    // The head, and the shoulders: the top half of a disc below it, its
    // flat foot inside the picture.
    let d = f32::from(radius) * 2.0;
    gr.paint_sector(crate::point(c.x, c.y - px(d * 0.12)), px(0.0), px(d * 0.16), 0.0, std::f32::consts::TAU, white());
    gr.paint_sector(crate::point(c.x, c.y + px(d * 0.36)), px(0.0), px(d * 0.27), 1.5 * std::f32::consts::PI, std::f32::consts::PI, white());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_to_initials() {
        assert_eq!(initials("Mira Okafor"), "MO");
        assert_eq!(initials("Design Crew Team"), "DT");
        assert_eq!(initials("jun"), "J");
        assert_eq!(initials("🎉 Party"), "P");
        assert_eq!(id_of("4155550134"), id_of("4155550134"));
        assert_ne!(id_of("4155550134"), id_of("4155550135"));
    }
}

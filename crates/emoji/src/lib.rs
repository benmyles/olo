//! Colour emoji. Fedora's Noto Color Emoji is a COLRv1 font: layers of
//! outlines filled with solid colours and linear, radial and sweep
//! gradients, composited in groups. This crate shapes a cluster with
//! harfrust (ZWJ sequences, skin tones, keycaps and flags are ligatures
//! in the font), walks the glyph's paint graph with skrifa and paints it
//! with tiny-skia into an RGBA image the caller uploads as a texture.
//! COLRv0 fonts take the same path.
//!
//! Which characters start an emoji (`starts_emoji`, `clusters`) needs
//! nothing else: GPUI's text system takes just that (no default
//! features), so every app shows the same characters in colour.

#[cfg(feature = "render")]
mod render;
#[cfg(feature = "render")]
pub use render::{ColorFont, Image};

/// A regional indicator (a flag's letters).
pub fn is_regional(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Where a cluster starting with `c` (followed by `next`) is an emoji
/// to show in colour.
pub fn starts_emoji(c: char, next: Option<char>) -> bool {
    let u = c as u32;
    // Below U+1F000, those shown as emoji by default (Emoji_Presentation).
    const BMP: &[(u32, u32)] = &[
        (0x231A, 0x231B),
        (0x23E9, 0x23EC),
        (0x23F0, 0x23F0),
        (0x23F3, 0x23F3),
        (0x25FD, 0x25FE),
        (0x2614, 0x2615),
        (0x2648, 0x2653),
        (0x267F, 0x267F),
        (0x2693, 0x2693),
        (0x26A1, 0x26A1),
        (0x26AA, 0x26AB),
        (0x26BD, 0x26BE),
        (0x26C4, 0x26C5),
        (0x26CE, 0x26CE),
        (0x26D4, 0x26D4),
        (0x26EA, 0x26EA),
        (0x26F2, 0x26F3),
        (0x26F5, 0x26F5),
        (0x26FA, 0x26FA),
        (0x26FD, 0x26FD),
        (0x2705, 0x2705),
        (0x270A, 0x270B),
        (0x2728, 0x2728),
        (0x274C, 0x274C),
        (0x274E, 0x274E),
        (0x2753, 0x2755),
        (0x2757, 0x2757),
        (0x2795, 0x2797),
        (0x27B0, 0x27B0),
        (0x27BF, 0x27BF),
        (0x2B1B, 0x2B1C),
        (0x2B50, 0x2B50),
        (0x2B55, 0x2B55),
    ];
    // Those below that take a skin tone (Emoji_Modifier_Base), with one
    // ("✌🏽").
    let toned = matches!(u, 0x261D | 0x26F9 | 0x270C | 0x270D) && next.is_some_and(|n| ('\u{1F3FB}'..='\u{1F3FF}').contains(&n));
    (0x1F000..=0x1FAFF).contains(&u) || BMP.iter().any(|&(a, b)| (a..=b).contains(&u)) || is_regional(c) || toned || next == Some('\u{FE0F}') || next == Some('\u{20E3}') && !c.is_alphabetic()
}

/// The emoji in `text`, as byte ranges: each a whole sequence (joined
/// with ZWJ, a skin tone, a flag's two letters, a keycap).
pub fn clusters(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut it = text.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let next = it.peek().map(|&(_, n)| n);
        if !starts_emoji(c, next) {
            continue;
        }
        let mut end = i + c.len_utf8();
        let mut flag = is_regional(c);
        while let Some(&(j, n)) = it.peek() {
            let joins = n == '\u{FE0F}' || n == '\u{20E3}' || ('\u{1F3FB}'..='\u{1F3FF}').contains(&n) || ('\u{E0020}'..='\u{E007F}').contains(&n);
            if joins || flag && is_regional(n) {
                flag = false;
                end = j + n.len_utf8();
                it.next();
            } else if n == '\u{200D}' {
                it.next();
                end = j + n.len_utf8();
                if let Some((k, m)) = it.next() {
                    end = k + m.len_utf8();
                }
            } else {
                break;
            }
        }
        out.push(i..end);
    }
    out
}

#[cfg(test)]
mod emoji_chars {
    use super::*;

    #[test]
    fn which_start_an_emoji() {
        // Emoji by default, below U+1F000.
        assert!(starts_emoji('✋', None) && starts_emoji('⚡', None) && starts_emoji('✅', None));
        // Text by default: emoji with VS16 or a skin tone.
        assert!(!starts_emoji('✌', None) && starts_emoji('✌', Some('\u{FE0F}')) && starts_emoji('✌', Some('🏽')));
        assert!(!starts_emoji('a', None) && !starts_emoji('a', Some('🏽')));
        assert_eq!(clusters("hi ✋🏽 ok 👍"), vec![3..10, 14..18]);
    }
}

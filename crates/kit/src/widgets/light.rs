//! A status light: a small round dot before a line of words that says
//! how a thing stands (a host's connection, a service, a pool on a
//! host), in the look's own colours: on, waiting, down (something went
//! wrong) and off (nothing wrong: not in use). And a ring in its place
//! for a thing being done now. Rex's windows had them first; the Token
//! Pool's Hosts window is the second app to (2026-11-23), so they are
//! the kit's.

use olo_interface::{Element, Hsla, IntoElement as _, Styled as _, canvas, px};

use crate::{style, theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Light {
    On,
    Waiting,
    Down,
    Off,
}

impl Light {
    pub fn color(self, p: &olo_theme::Palette) -> Hsla {
        match self {
            Light::On => style::c(p.success),
            // (The look's orange: `warning` is made to be read as
            // text, and as a dot it was a brown hard to tell from red
            // at 1×.)
            Light::Waiting => style::c(p.hue(olo_theme::Hue::Orange)),
            Light::Down => style::c(p.failure),
            Light::Off => style::disabled_text(p),
        }
    }
}

/// How far across a status light is.
pub const LIGHT: f32 = 8.0;

/// A status light before a line of text: 8 points across and no wider
/// than it looks, so that its edge is the edge of what it is set under
/// (in a box of 10 it stood a point to the right of the label above
/// it); to be centred on the line beside it.
pub fn light(light: Light) -> Element {
    light_of(light, LIGHT)
}

/// The same light, `across` points across: before words set smaller
/// than a label's (a status bar's), in their proportion.
pub fn light_of(light: Light, across: f32) -> Element {
    canvas(move |g, b| style::dot(g, b.center(), across, light.color(&theme::palette())))
        .paint_key(style::key((light, across.to_bits())))
        .w(px(across))
        .h(px(across))
        .flex_shrink_0()
        .into_any_element()
}

/// A thing being done now, where a light would stand: a ring in the
/// waiting light's colour, `across` points across. (Not that light
/// itself, which is also a thing at rest that wants something: Rex's
/// Host window's "Being installed…" beside its "nothing granted", to
/// its second design review.)
pub fn busy_of(across: f32) -> Element {
    canvas(move |g, b| {
        let s = g.scale_factor();
        let r = style::snap_centred(g, b.center(), across, across);
        // (A point and a half of line, in whole device pixels.)
        let line = (1.5 * s).round().max(1.0) / s;
        g.paint_quad(olo_interface::quad(r, r.size.width / 2.0, olo_interface::transparent_black(), px(line), Light::Waiting.color(&theme::palette()), olo_interface::BorderStyle::Solid));
    })
    .paint_key(style::key(("busy", across.to_bits())))
    .w(px(across))
    .h(px(across))
    .flex_shrink_0()
    .into_any_element()
}

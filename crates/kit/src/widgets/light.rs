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
    ring_of(Light::Waiting, across)
}

/// A ring in `light`'s colour, `across` points across: a thing not at
/// rest in that standing. In the waiting light's it is a thing being
/// done ([`busy_of`]); in the off light's, one that waits its turn
/// behind it (a row of steps of which one runs: the same shape as the
/// one running, in the grey of what hasn't begun).
pub fn ring_of(light: Light, across: f32) -> Element {
    canvas(move |g, b| style::ring(g, b.center(), across, light.color(&theme::palette())))
        .paint_key(style::key(("ring", light, across.to_bits())))
        .w(px(across))
        .h(px(across))
        .flex_shrink_0()
        .into_any_element()
}

//! Original typed CSS color values. Parsing, contextual resolution and
//! unclipped colorimetric conversion; not cascade, gamut mapping or painting.
mod compute;
pub use compute::Computed;
mod convert;
mod functional;
mod matrices;
mod named;
mod parse;
use crate::css::{Components, math};
pub use functional::Functional;
use functional::{Numeric, Unit};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
#[cfg(feature = "baseline-color-f64")]
type Channel = f64;
#[cfg(not(feature = "baseline-color-f64"))]
type Channel = f32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Space {
    Srgb,
    SrgbLinear,
    DisplayP3,
    DisplayP3Linear,
    A98Rgb,
    ProphotoRgb,
    Rec2020,
    XyzD50,
    XyzD65,
    Lab,
    Lch,
    Oklab,
    Oklch,
    Hsl,
    Hwb,
}
/// Four finite channels with a separate missing-component mask. Retain
/// space/missing information until interpolation or absolute painting.
/// Storage uses f32 (enough for CSS's minimum round-trip precision), with
/// f64 conversion arithmetic. Baseline builds retain f64 for comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    channels: [Channel; 4],
    space: Space,
    missing: u8,
}
impl Color {
    /// Raw color-space coordinates, not a parser: conversions may produce
    /// out-of-range values. Alpha clamps to [0,1]; missing coordinates use
    /// stored zero and remain distinct in the mask. Finite coordinate limit
    /// is +/-f32::MAX in both storage modes. NaN input becomes zero.
    pub fn new(space: Space, channels: [f64; 3], alpha: f64, missing: u8) -> Self {
        let mut values = [0.; 4];
        values[..3].copy_from_slice(&channels);
        values[3] = if alpha.is_nan() { 0. } else { alpha.clamp(0., 1.) };
        Self { channels: std::array::from_fn(|i| pack(if missing & (1 << i) != 0 { 0. } else { values[i] })), space, missing: missing & 15 }
    }
    pub fn space(self) -> Space {
        self.space
    }
    pub fn components(self) -> [f64; 3] {
        std::array::from_fn(|i| unpack(self.channels[i]))
    }
    pub fn alpha(self) -> f64 {
        unpack(self.channels[3])
    }
    pub fn missing(self) -> u8 {
        self.missing
    }
    pub fn is_missing(self, channel: usize) -> bool {
        channel < 4 && self.missing & (1 << channel) != 0
    }
    /// Resolve legacy hue-model colors to clipped sRGB when no components
    /// are missing. Keep hue-model identity for values containing `none`.
    /// This does not perform general destination gamut mapping.
    pub fn computed(self) -> Self {
        if matches!(self.space, Space::Hsl | Space::Hwb) && self.missing == 0 {
            let c = self.convert_absolute(Space::Srgb);
            return Color::new(Space::Srgb, c.components().map(|x| x.clamp(0., 1.)), c.alpha(), 0);
        }
        self
    }
    /// Explicit absolute-color conversion: input `none` becomes zero.
    /// No gamut mapping/clipping; out-of-gamut values remain represented.
    /// Powerless hues produced by a space conversion are marked missing.
    /// Same-space calls only materialize the input missing components.
    pub fn convert_absolute(self, target: Space) -> Self {
        convert::convert(&self, target)
    }
    /// Clipping is explicit. A future renderer must choose its destination
    /// profile/gamut-mapping policy rather than mistake this for CSS mapping.
    pub fn srgb8_clipped(self) -> [u8; 4] {
        let c = self.convert_absolute(Space::Srgb);
        let v = c.components();
        [quantize(v[0]), quantize(v[1]), quantize(v[2]), quantize(c.alpha())]
    }
}
/// Allocation-free conversion worker. Each step converts at most 4096
/// colors and checks cancellation at least every four. The caller can
/// stream progress, yield and throttle UI messages between steps. Work
/// already completed remains converted after cancellation.
pub struct Conversion<'a> {
    colors: &'a mut [Color],
    target: Space,
    converted: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub converted: usize,
    pub total: usize,
}
impl Progress {
    pub fn finished(self) -> bool {
        self.converted == self.total
    }
}
impl<'a> Conversion<'a> {
    pub fn new(colors: &'a mut [Color], target: Space) -> Self {
        Self { colors, target, converted: 0 }
    }
    pub fn progress(&self) -> Progress {
        Progress { converted: self.converted, total: self.colors.len() }
    }
    pub fn step(&mut self, limit: usize, cancelled: Option<&AtomicBool>) -> Result<Progress> {
        check(cancelled)?;
        if limit == 0 && !self.progress().finished() {
            return Err(Error::Limit);
        }
        let end = self.converted.saturating_add(limit.min(4096)).min(self.colors.len());
        // Dispatch once per bounded step. Trying the vector helper for
        // every four nonlinear colors adds calls/register spills to work
        // that cannot use it (caught by the release fallback benchmark).
        let use_batch = convert::use_batch(self.colors.get(self.converted).map(|c| c.space), self.target);
        if !use_batch {
            convert::scalar_step(&mut self.colors[self.converted..end], self.target, cancelled, &mut self.converted)?;
            return Ok(self.progress());
        }
        while self.converted < end {
            check(cancelled)?;
            let next = (self.converted + 4).min(end);
            let colors = &mut self.colors[self.converted..next];
            if !convert::batch(colors, self.target) {
                for color in colors {
                    *color = convert::convert(color, self.target);
                }
            }
            self.converted = next;
        }
        Ok(self.progress())
    }
}
#[cfg(feature = "baseline-color-f64")]
fn unpack(value: Channel) -> f64 {
    value
}
#[cfg(not(feature = "baseline-color-f64"))]
fn unpack(value: Channel) -> f64 {
    value as f64
}
fn pack(value: f64) -> Channel {
    let value = if value.is_nan() { 0. } else { value.clamp(-(f32::MAX as f64), f32::MAX as f64) };
    let value = if value == 0. { 0. } else { value };
    #[cfg(feature = "baseline-color-f64")]
    {
        value
    }
    #[cfg(not(feature = "baseline-color-f64"))]
    {
        value as f32
    }
}
fn quantize(value: f64) -> u8 {
    // Keep the scale in retained precision so a stored 70% rounds from
    // 178.5, rather than exposing its f32 representation error via f64.
    #[cfg(not(feature = "baseline-color-f64"))]
    {
        ((value.clamp(0., 1.) as f32) * 255. + 0.5).floor() as u8
    }
    #[cfg(feature = "baseline-color-f64")]
    {
        (value.clamp(0., 1.) * 255. + 0.5).floor() as u8
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Light,
    Dark,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemColor {
    AccentColor,
    AccentColorText,
    ActiveText,
    ButtonBorder,
    ButtonFace,
    ButtonText,
    Canvas,
    CanvasText,
    Field,
    FieldText,
    GrayText,
    Highlight,
    HighlightText,
    LinkText,
    Mark,
    MarkText,
    SelectedItem,
    SelectedItemText,
    VisitedText,
    ActiveBorder,
    ActiveCaption,
    AppWorkspace,
    Background,
    ButtonHighlight,
    ButtonShadow,
    CaptionText,
    InactiveBorder,
    InactiveCaption,
    InactiveCaptionText,
    InfoBackground,
    InfoText,
    Menu,
    MenuText,
    Scrollbar,
    ThreeDDarkShadow,
    ThreeDFace,
    ThreeDHighlight,
    ThreeDLightShadow,
    ThreeDShadow,
    Window,
    WindowFrame,
    WindowText,
}
/// Supplies already-resolved inherited/current color and the host's actual
/// palette for the element's used scheme. Night mode can choose Dark while
/// providing its own system palette; no desktop theme is guessed here.
pub trait Context {
    fn scheme(&self) -> Scheme;
    fn current_color(&self) -> Result<Color> {
        Err(Error::ContextUnavailable)
    }
    fn system_color(&self, _color: SystemColor, _scheme: Scheme) -> Result<Color> {
        Err(Error::ContextUnavailable)
    }
    /// Explicit stable numeric snapshot; no font or layout defaults.
    fn math_factor(&self, _unit: math::RelativeUnit) -> math::Result<f64> {
        Err(math::Error::ContextUnavailable)
    }
    /// The host enforces the math Context's stylesheet/tree-scope rules.
    fn math_tree_count(&self, _kind: math::TreeCount) -> math::Result<u32> {
        Err(math::Error::ContextUnavailable)
    }
}
/// Conservative dependencies of the complete value, including unselected
/// conditional branches. Cached once while compiling; no tree scan to query.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dependencies {
    pub current_color: bool,
    pub system_colors: bool,
    pub scheme: bool,
    pub math: math::Dependencies,
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Conditional nodes plus retained numeric instructions while compiling.
    /// Also bounds temporary math emission and resolution stack (max 4096).
    pub nodes: usize,
    /// Shared source/token/emitter work while compiling; conditional visits
    /// and all selected-channel instructions while resolving.
    pub work: usize,
    /// Shared color/math nesting, with an unconditional hard maximum of 128.
    pub depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self { nodes: 65536, work: 1_000_000, depth: 64 }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Unsupported(&'static str),
    RequiresSubstitution,
    Limit,
    DepthLimit,
    Cancelled,
    ContextUnavailable,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CSS color {self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Clone, Copy)]
enum Atom {
    Absolute(Color),
    Current,
    System(SystemColor),
    Functional(u32),
}
#[derive(Clone, Copy)]
enum Node {
    Absolute(Color),
    Current,
    System(SystemColor),
    LightDark { light: u32, dark: u32 },
    Functional(u32),
}
/// Arena-owned conditional expression; no recursive tree ownership/drop.
pub struct Expression {
    nodes: Vec<Node>,
    root: u32,
    functions: Box<[Functional]>,
    // Keep the booleans beside root so they use its alignment padding.
    math: math::Dependencies,
    current_color: bool,
    system_colors: bool,
    scheme: bool,
}
#[derive(Clone)]
pub enum Value {
    Absolute(Color),
    CurrentColor,
    System(SystemColor),
    Expression(Arc<Expression>),
    Functional(Arc<Functional>),
}
impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColorValue").field("nodes", &self.expression_nodes()).finish_non_exhaustive()
    }
}
impl Value {
    /// Worker-only complete value grammar over source-borrowed components.
    /// Simple colors use no heap allocation beyond the existing Syntax.
    pub fn parse(values: Components<'_>, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Self> {
        parse::parse(values, limits, cancelled)
    }
    /// Conditional-color arena nodes; numeric programs are accounted in bytes.
    pub fn expression_nodes(&self) -> usize {
        match self {
            Self::Expression(e) => e.nodes.len(),
            _ => 0,
        }
    }
    /// Retained arena capacities, boxed channels and program/Arc headers,
    /// excluding this Value, allocator metadata and transient worker scratch.
    /// A clone shares these allocations; do not add them twice for a pool.
    pub fn expression_reserved_bytes(&self) -> usize {
        match self {
            Self::Expression(e) => {
                let functions = e.functions.len() * size_of::<Functional>() + e.functions.iter().map(Functional::reserved_bytes).sum::<usize>();
                e.nodes.capacity() * size_of::<Node>() + size_of::<Expression>() + 2 * size_of::<usize>() + functions
            }
            Self::Functional(f) => size_of::<Functional>() + 2 * size_of::<usize>() + f.reserved_bytes(),
            _ => 0,
        }
    }
    pub fn dependencies(&self) -> Dependencies {
        match self {
            Self::Absolute(_) => Dependencies::default(),
            Self::CurrentColor => Dependencies { current_color: true, ..Default::default() },
            Self::System(_) => Dependencies { system_colors: true, scheme: true, ..Default::default() },
            Self::Expression(e) => Dependencies { current_color: e.current_color, system_colors: e.system_colors, scheme: e.scheme, math: e.math },
            Self::Functional(f) => Dependencies { math: f.dependencies(), ..Default::default() },
        }
    }
    /// Resolve only the chosen conditional branch. Both branches were
    /// grammar-checked at parse time. This remains color data, not painting.
    pub fn resolve(&self, context: &impl Context, limits: Limits, cancelled: Option<&AtomicBool>) -> Result<Color> {
        check(cancelled)?;
        let atom = match self {
            Self::Absolute(c) => Atom::Absolute(*c),
            Self::CurrentColor => Atom::Current,
            Self::System(s) => Atom::System(*s),
            Self::Functional(f) => return Ok(f.resolve(context, limits, cancelled)?.computed()),
            Self::Expression(e) => {
                let mut at = e.root;
                let mut work = 0;
                loop {
                    check(cancelled)?;
                    if work >= limits.work {
                        return Err(Error::Limit);
                    }
                    work += 1;
                    match e.nodes[at as usize] {
                        Node::Absolute(c) => break Atom::Absolute(c),
                        Node::Current => break Atom::Current,
                        Node::System(s) => break Atom::System(s),
                        Node::Functional(i) => {
                            let mut remaining = limits;
                            remaining.work -= work;
                            return Ok(e.functions[i as usize].resolve(context, remaining, cancelled)?.computed());
                        }
                        Node::LightDark { light, dark } => {
                            at = match context.scheme() {
                                Scheme::Light => light,
                                Scheme::Dark => dark,
                            };
                        }
                    }
                }
            }
        };
        let color = match atom {
            Atom::Absolute(c) => Ok(c),
            Atom::Current => context.current_color(),
            Atom::System(s) => context.system_color(s, context.scheme()),
            Atom::Functional(_) => unreachable!(),
        }?;
        Ok(color.computed())
    }
}
fn check(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|c| c.load(Ordering::Relaxed)) { Err(Error::Cancelled) } else { Ok(()) }
}
fn hue(value: f64) -> f64 {
    if value.is_finite() { value.rem_euclid(360.) } else { 0. }
}

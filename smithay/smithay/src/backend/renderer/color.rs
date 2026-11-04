use std::ops::Mul;

/// A four-component color representing pre-multiplied RGBA color values
#[derive(Debug, Copy, Clone, Default, PartialEq)]
pub struct Color32F([f32; 4]);

impl Color32F {
    /// Initialize a new [`Color32F`]
    #[inline]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self([r, g, b, a])
    }
}

impl Color32F {
    /// Transparent color
    pub const TRANSPARENT: Color32F = Color32F::new(0.0, 0.0, 0.0, 0.0);

    /// Solid black color
    pub const BLACK: Color32F = Color32F::new(0f32, 0f32, 0f32, 1f32);
}

impl Color32F {
    /// Red color component
    #[inline]
    pub fn r(&self) -> f32 {
        self.0[0]
    }

    /// Green color component
    #[inline]
    pub fn g(&self) -> f32 {
        self.0[1]
    }

    /// Blue color component
    #[inline]
    pub fn b(&self) -> f32 {
        self.0[2]
    }

    /// Alpha color component
    #[inline]
    pub fn a(&self) -> f32 {
        self.0[3]
    }

    /// Color components
    #[inline]
    pub fn components(self) -> [f32; 4] {
        self.0
    }
}

impl Color32F {
    /// Test if the color represents a opaque color
    #[inline]
    pub fn is_opaque(&self) -> bool {
        self.a() == 1f32
    }
}

impl From<[f32; 4]> for Color32F {
    #[inline]
    fn from(value: [f32; 4]) -> Self {
        Self(value)
    }
}

impl Mul<f32> for Color32F {
    type Output = Color32F;

    #[inline]
    fn mul(self, rhs: f32) -> Self::Output {
        Self::new(self.r() * rhs, self.g() * rhs, self.b() * rhs, self.a() * rhs)
    }
}

/// (Olo) What a texture's colours are in: its primaries, and how it
/// encodes light. sRGB unless said (`gles::color`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSource {
    /// Its primaries and white.
    pub primaries: olo_color::Primaries,
    /// How it encodes light. sRGB content is taken as gamma 2.2, as a
    /// display shows it.
    pub transfer: olo_color::Transfer,
}

impl ColorSource {
    /// What nearly everything is drawn in.
    pub const SRGB: ColorSource = ColorSource {
        primaries: olo_color::Primaries::SRGB,
        transfer: olo_color::Transfer::Gamma22,
    };
}

/// (Olo) A surface's colours: what the client said its buffers hold (an
/// image description, wp_color_management_v1), drawn converted from it.
/// Cached state, so it applies with the surface's next commit (a
/// synchronized subsurface's with its parent's). None: sRGB.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct SurfaceColor(pub Option<ColorSource>);

#[cfg(feature = "wayland_frontend")]
impl crate::wayland::compositor::Cacheable for SurfaceColor {
    fn commit(&mut self, _dh: &wayland_server::DisplayHandle) -> Self {
        *self
    }

    fn merge_into(self, into: &mut Self, _dh: &wayland_server::DisplayHandle) {
        *into = self;
    }
}

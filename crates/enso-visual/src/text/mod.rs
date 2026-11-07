//! Text: strings drawn in the scene as one quad per glyph, shaded from a
//! multi-channel signed distance field (MSDF) atlas.
//!
//! An MSDF atlas stores, per texel, the distance to the glyph's outline
//! (in three channels, which keeps corners sharp). The shader turns that
//! distance into coverage, so text stays crisp at any size and angle, and
//! outlines, glows and weights are shader maths.
//!
//! * [`FontDesc`] → [`Renderer::create_font`](crate::Renderer::create_font)
//!   builds the atlas (once, on the CPU).
//! * Text shaders are written against [`PRELUDE`]; materials and named
//!   parameters work as for meshes.
//! * Each tick, [`RenderWorld::draw_text`](crate::RenderWorld::draw_text)
//!   draws a [`TextDraw`] with its string.
//!
//! Layout is simple: lines split on `\n`, aligned per [`TextAlign`], no
//! kerning or complex-script shaping (yet).
//!
//! **Font caveat:** the distance-field generator (`fdsm`) does not handle
//! glyphs whose outlines *overlap* (common in fonts exported straight from a
//! variable-font source, e.g. Inter). They show small spikes or specks where
//! strokes join. Prefer fonts with overlaps removed (most "static" builds,
//! e.g. Noto Sans), or remove overlaps in a font editor.

mod font;
mod renderer;

use glam::Vec4;

pub(crate) use font::FontAtlas;
pub(crate) use renderer::TextRenderer;

use crate::{FontId, MaterialId, Transform, world::Span};

/// The WGSL contract prepended to every text shader (after the globals and
/// material declarations).
pub const PRELUDE: &str = include_str!("prelude.wgsl");

/// Printable ASCII plus a few symbols handy for music. Characters a font
/// lacks are skipped.
pub const DEFAULT_CHARS: &str = concat!(
    " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~",
    "♯♭♮°·•–—…‘’“”"
);

/// A font to build an atlas from.
#[derive(Debug, Clone)]
pub struct FontDesc<'a> {
    pub label: &'a str,
    /// The contents of a `.ttf` / `.otf` file.
    pub data: &'a [u8],
    /// Characters to put in the atlas.
    pub chars: &'a str,
    /// Atlas pixels per em. Higher = sharper corners up close, bigger atlas.
    pub glyph_px: f64,
    /// Distance range around each outline, in atlas pixels. Limits how far
    /// outlines and glows can reach.
    pub range_px: f64,
}

impl<'a> FontDesc<'a> {
    pub fn new(label: &'a str, data: &'a [u8]) -> Self {
        Self { label, data, chars: DEFAULT_CHARS, glyph_px: 48.0, range_px: 6.0 }
    }
    pub fn with_chars(mut self, chars: &'a str) -> Self {
        self.chars = chars;
        self
    }
    pub fn with_glyph_px(mut self, glyph_px: f64) -> Self {
        self.glyph_px = glyph_px;
        self
    }
    pub fn with_range_px(mut self, range_px: f64) -> Self {
        self.range_px = range_px;
        self
    }
}

/// Horizontal alignment of each line around the text's origin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// A string in a [`RenderWorld`](crate::RenderWorld). Its glyphs reach the
/// shader as `GlyphIn` (see [`PRELUDE`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextDraw {
    /// Stable identity across ticks; matched draws interpolate transform,
    /// colour, size and parameters.
    pub key: Option<u64>,
    pub font: FontId,
    /// Must use a text shader ([`TextShaderId`](crate::TextShaderId)).
    pub material: MaterialId,
    /// Places the text: its origin is the first line's baseline, at the
    /// alignment point; text runs along +X with +Y up.
    pub transform: Transform,
    pub color: Vec4,
    /// Height of one em, in world units.
    pub size: f32,
    pub align: TextAlign,
    pub(crate) text: Span,
    pub(crate) params: Span,
}

impl TextDraw {
    pub fn new(font: FontId, material: MaterialId, transform: impl Into<Transform>) -> Self {
        Self {
            key: None,
            font,
            material,
            transform: transform.into(),
            color: Vec4::ONE,
            size: 1.0,
            align: TextAlign::Left,
            text: Span::default(),
            params: Span::default(),
        }
    }
    pub fn with_key(mut self, key: u64) -> Self {
        self.key = Some(key);
        self
    }
    pub fn with_color(mut self, color: Vec4) -> Self {
        self.color = color;
        self
    }
    pub fn with_size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    pub fn with_align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }
}

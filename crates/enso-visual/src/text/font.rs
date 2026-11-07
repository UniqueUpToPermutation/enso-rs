//! Fonts on the CPU: building an MSDF atlas from a TrueType/OpenType font,
//! and laying out a string as positioned glyph quads.
//!
//! Units: glyph metrics are kept in **ems** (1.0 = the font's em size).
//! `TextDraw::size` scales ems to world units when the text is drawn.

use std::collections::HashMap;

use fdsm::{
    bezier::{Point, Segment, scanline::FillRule},
    correct_error::{ErrorCorrectionConfig, correct_error_msdf},
    generate::generate_msdf,
    render::correct_sign_msdf,
    shape::{Contour, Shape},
};
use image::{DynamicImage, Rgb32FImage, RgbImage};

use super::{FontDesc, TextAlign};
use crate::RenderError;

/// Width of the atlas texture; its height grows to fit.
const ATLAS_WIDTH: u32 = 1024;
/// Space between glyphs in the atlas, so linear filtering never mixes neighbours.
const PADDING: u32 = 1;

/// One character's metrics and atlas location.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Glyph {
    /// Horizontal distance to the next glyph, in ems.
    pub advance: f32,
    /// The quad to draw, in ems relative to the pen position on the
    /// baseline: `[x0, y0, x1, y1]` (y up). `None` for blank glyphs (space).
    pub plane: Option<[f32; 4]>,
    /// The quad's texture coordinates in the atlas: `[u0, v0, u1, v1]`
    /// with v0 at the *top* edge.
    pub uv: [f32; 4],
}

/// A font's MSDF atlas plus everything needed to lay text out.
#[derive(Debug)]
pub(crate) struct FontAtlas {
    pub width: u32,
    pub height: u32,
    /// RGBA8, linear (distances, not colours).
    pub pixels: Vec<u8>,
    /// Distance range encoded in the atlas, in atlas pixels.
    pub px_range: f32,
    glyphs: HashMap<char, Glyph>,
    /// Baseline-to-baseline distance, in ems.
    line_height: f32,
}

/// A glyph placed by [`FontAtlas::layout`], in ems (y up, first baseline at 0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlacedGlyph {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    /// Index of the character in the string (counting every `char`).
    pub index: u32,
}

impl FontAtlas {
    pub(crate) fn build(desc: &FontDesc<'_>) -> Result<Self, RenderError> {
        let face = ttf_parser::Face::parse(desc.data, 0)
            .map_err(|e| RenderError::InvalidFont(format!("{}: {e}", desc.label)))?;
        let units_per_em = f64::from(face.units_per_em());
        let scale = desc.glyph_px / units_per_em; // atlas pixels per font unit
        let range = desc.range_px;

        // Generate every glyph's bitmap first, then pack them.
        let mut bitmaps: Vec<(char, RgbImage, [f32; 4])> = Vec::new();
        let mut glyphs = HashMap::new();
        for c in desc.chars.chars() {
            if glyphs.contains_key(&c) {
                continue;
            }
            let Some(id) = face.glyph_index(c) else { continue };
            let advance = face.glyph_hor_advance(id).map_or(0.0, |a| f64::from(a) / units_per_em) as f32;
            let blank = Glyph { advance, plane: None, uv: [0.0; 4] };

            let Some(bbox) = face.glyph_bounding_box(id) else {
                glyphs.insert(c, blank); // e.g. space: advance only
                continue;
            };
            let (x_min, y_min, y_max) = (f64::from(bbox.x_min), f64::from(bbox.y_min), f64::from(bbox.y_max));
            let width = ((f64::from(bbox.x_max) - x_min) * scale + 2.0 * range).ceil() as u32;
            let height = ((y_max - y_min) * scale + 2.0 * range).ceil() as u32;

            // Outline -> fdsm shape in bitmap pixels, keeping the font's y-up
            // orientation: mirroring the outline here would reverse its
            // winding and break the distance field at corners. The finished
            // bitmap is flipped instead.
            let mut builder = ShapeBuilder::new(move |x, y| {
                Point::new((f64::from(x) - x_min) * scale + range, (f64::from(y) - y_min) * scale + range)
            });
            face.outline_glyph(id, &mut builder);
            let shape = builder.finish();
            if shape.contours.is_empty() {
                glyphs.insert(c, blank);
                continue;
            }

            let colored = Shape::edge_coloring_simple(shape, 0.03, 0x5eed);
            let prepared = colored.prepare();
            // Generated in f32 so the error-correction pass can run: it
            // removes the stray spikes plain MSDF can leave at sharp corners.
            let mut field = Rgb32FImage::new(width, height);
            generate_msdf(&prepared, range, &mut field);
            correct_sign_msdf(&mut field, &prepared, FillRule::Nonzero);
            correct_error_msdf(&mut field, &colored, &prepared, range, &ErrorCorrectionConfig::default());
            // Rows were generated bottom-up (y up); the atlas is top-down.
            let bitmap: RgbImage = image::imageops::flip_vertical(&DynamicImage::ImageRgb32F(field).to_rgb8());

            // The bitmap covers the glyph's box plus `range` pixels of margin.
            let em = |px: f64| (px / desc.glyph_px) as f32;
            let x0 = (x_min / units_per_em) as f32 - em(range);
            let top = (y_max / units_per_em) as f32 + em(range);
            let plane = [x0, top - em(f64::from(height)), x0 + em(f64::from(width)), top];

            glyphs.insert(c, Glyph { advance, plane: Some(plane), uv: [0.0; 4] });
            bitmaps.push((c, bitmap, plane));
        }

        // Shelf packing: left to right in rows, tallest first so rows are tight.
        bitmaps.sort_by_key(|(_, b, _)| std::cmp::Reverse(b.height()));
        let mut placements = Vec::with_capacity(bitmaps.len());
        let (mut x, mut y, mut row_height) = (PADDING, PADDING, 0);
        for (_, bitmap, _) in &bitmaps {
            if x + bitmap.width() + PADDING > ATLAS_WIDTH {
                x = PADDING;
                y += row_height + PADDING;
                row_height = 0;
            }
            placements.push((x, y));
            x += bitmap.width() + PADDING;
            row_height = row_height.max(bitmap.height());
        }
        let height = (y + row_height + PADDING).next_power_of_two().max(4);

        let mut pixels = vec![0u8; (ATLAS_WIDTH * height * 4) as usize];
        for ((c, bitmap, _), (px, py)) in bitmaps.iter().zip(&placements) {
            for (bx, by, rgb) in bitmap.enumerate_pixels() {
                let i = (((py + by) * ATLAS_WIDTH + px + bx) * 4) as usize;
                pixels[i..i + 3].copy_from_slice(&rgb.0);
                pixels[i + 3] = 255;
            }
            let (w, h) = (ATLAS_WIDTH as f32, height as f32);
            glyphs.get_mut(c).unwrap().uv = [
                *px as f32 / w,
                *py as f32 / h,
                (px + bitmap.width()) as f32 / w,
                (py + bitmap.height()) as f32 / h,
            ];
        }

        let line_height =
            f64::from(face.ascender() - face.descender() + face.line_gap()) / units_per_em;
        Ok(Self {
            width: ATLAS_WIDTH,
            height,
            pixels,
            px_range: range as f32,
            glyphs,
            line_height: line_height as f32,
        })
    }

    /// Lay `text` out in ems: lines split on `\n`, each aligned around x = 0
    /// per `align`; the first baseline is at y = 0, later lines below.
    /// Characters missing from the atlas use `?` if present, else are skipped.
    pub(crate) fn layout(&self, text: &str, align: TextAlign, out: &mut Vec<PlacedGlyph>) {
        out.clear();
        let fallback = self.glyphs.get(&'?');
        let glyph = |c: char| self.glyphs.get(&c).or(fallback);

        let mut index = 0u32;
        for (line_number, line) in text.split('\n').enumerate() {
            let width: f32 = line.chars().filter_map(glyph).map(|g| g.advance).sum();
            let mut pen = match align {
                TextAlign::Left => 0.0,
                TextAlign::Center => -width / 2.0,
                TextAlign::Right => -width,
            };
            let baseline = -(line_number as f32) * self.line_height;
            for c in line.chars() {
                if let Some(g) = glyph(c) {
                    if let Some([x0, y0, x1, y1]) = g.plane {
                        out.push(PlacedGlyph {
                            rect: [pen + x0, baseline + y0, pen + x1, baseline + y1],
                            uv: g.uv,
                            index,
                        });
                    }
                    pen += g.advance;
                }
                index += 1;
            }
            index += 1; // the '\n'
        }
    }
}

/// Collects ttf-parser outline commands into an fdsm shape, mapping each
/// point from font units to bitmap pixels.
struct ShapeBuilder<F> {
    map: F,
    contours: Vec<Contour>,
    current: Contour,
    start: Point,
    last: Point,
}

impl<F: Fn(f32, f32) -> Point> ShapeBuilder<F> {
    fn new(map: F) -> Self {
        let origin = Point::new(0.0, 0.0);
        Self { map, contours: Vec::new(), current: Contour::default(), start: origin, last: origin }
    }

    fn end_contour(&mut self) {
        if self.last != self.start {
            self.current.segments.push(Segment::line(self.last, self.start));
        }
        let contour = std::mem::take(&mut self.current);
        if !contour.segments.is_empty() {
            self.contours.push(contour);
        }
        self.last = self.start;
    }

    fn finish(mut self) -> Shape<Contour> {
        self.end_contour();
        Shape { contours: self.contours }
    }
}

impl<F: Fn(f32, f32) -> Point> ttf_parser::OutlineBuilder for ShapeBuilder<F> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.end_contour();
        self.start = (self.map)(x, y);
        self.last = self.start;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = (self.map)(x, y);
        self.current.segments.push(Segment::line(self.last, p));
        self.last = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let p = (self.map)(x, y);
        self.current.segments.push(Segment::quad(self.last, (self.map)(x1, y1), p));
        self.last = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let p = (self.map)(x, y);
        self.current.segments.push(Segment::cubic(self.last, (self.map)(x1, y1), (self.map)(x2, y2), p));
        self.last = p;
    }
    fn close(&mut self) {
        self.end_contour();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTO_SANS: &[u8] = include_bytes!("../../examples/fonts/NotoSans-Regular.ttf");

    fn atlas() -> FontAtlas {
        FontAtlas::build(&FontDesc::new("Noto Sans", NOTO_SANS).with_chars("AVa ?")).unwrap()
    }

    #[test]
    fn builds_glyphs_and_blank_space() {
        let atlas = atlas();
        let a = atlas.glyphs[&'A'];
        let space = atlas.glyphs[&' '];
        assert!(a.plane.is_some() && a.advance > 0.4 && a.advance < 1.0, "{a:?}");
        assert!(space.plane.is_none() && space.advance > 0.1, "{space:?}");
        // Inside the glyph the median distance is > 0.5 (inside); at the
        // padded corner it is < 0.5 (outside).
        let [u0, v0, u1, v1] = a.uv;
        let texel = |u: f32, v: f32| {
            let (x, y) = ((u * atlas.width as f32) as u32, (v * atlas.height as f32) as u32);
            let i = ((y * atlas.width + x) * 4) as usize;
            let mut rgb = [atlas.pixels[i], atlas.pixels[i + 1], atlas.pixels[i + 2]];
            rgb.sort();
            rgb[1]
        };
        assert!(texel(u0 + 0.001, v0 + 0.001) < 128);
        // The A's crossbar: a little below the middle of its box.
        assert!(texel((u0 + u1) / 2.0, v0 + (v1 - v0) * 0.6) > 128);
    }

    #[test]
    fn lays_out_lines_and_alignment() {
        let atlas = atlas();
        let mut out = Vec::new();

        atlas.layout("A A", TextAlign::Left, &mut out);
        assert_eq!(out.len(), 2, "the space draws nothing");
        assert_eq!(out[1].index, 2, "indices count every character");
        assert!(out[1].rect[0] > out[0].rect[0]);

        atlas.layout("A", TextAlign::Center, &mut out);
        let centre = (out[0].rect[0] + out[0].rect[2]) / 2.0;
        assert!(centre.abs() < 0.05, "centred around x = 0: {centre}");

        atlas.layout("A\nA", TextAlign::Left, &mut out);
        assert!(out[1].rect[1] < out[0].rect[1] - 0.5, "second line is below the first");

        atlas.layout("Ω", TextAlign::Left, &mut out);
        assert_eq!(out.len(), 1, "missing characters fall back to '?'");
    }
}

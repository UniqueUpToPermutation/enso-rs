// ---- enso text contract ----
//
// Prepended to every text shader (after the globals and the material
// declarations). Each glyph is one instance; its quad is built from the
// vertex index (6 vertices, no vertex buffer). Your shader must define:
//
//   @vertex   fn vs_main(@builtin(vertex_index) vertex: u32, g: GlyphIn) -> <your output>
//   @fragment fn fs_main(<your output>) -> @location(0) vec4<f32>
//
// and may declare `struct Material { .. }` / `struct Instance { .. }` as for
// meshes (`instance_data(g)` gives the draw's Instance values; every glyph of
// one TextDraw shares them).
//
// Typical shader:
//
//   let corner = glyph_corner(vertex, g);
//   out.clip = globals.view_proj * instance_model(g) * vec4<f32>(corner.position, 0.0, 1.0);
//   out.uv = corner.uv;
//   ...
//   return vec4<f32>(color.rgb, color.a * text_coverage(in.uv));   // fragment
//
// Text is drawn double-sided; use a blended BlendMode (Alpha or Additive).

// Per-glyph data.
struct GlyphIn {
    @location(0) model_0: vec4<f32>,
    @location(1) model_1: vec4<f32>,
    @location(2) model_2: vec4<f32>,
    @location(3) model_3: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) rect: vec4<f32>,     // quad in text space: min.xy, max.xy (y up, world units)
    @location(6) uv_rect: vec4<f32>,  // atlas uv: min.xy at the top-left, max.xy bottom-right
    @location(7) index: u32,          // index of this character in the string
    @location(8) param_slot: u32,     // index of this draw's `Instance` block
};

fn instance_model(g: GlyphIn) -> mat4x4<f32> {
    return mat4x4<f32>(g.model_0, g.model_1, g.model_2, g.model_3);
}

struct GlyphCorner {
    position: vec2<f32>,  // in text space (multiply by instance_model)
    uv: vec2<f32>,        // atlas uv for text_coverage / text_screen_distance
    corner: vec2<f32>,    // 0..1 across the quad, (0, 0) = bottom-left
};

// The corner of this glyph's quad for `vertex` (0..5: two triangles).
fn glyph_corner(vertex: u32, g: GlyphIn) -> GlyphCorner {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[vertex % 6u];
    var out: GlyphCorner;
    out.position = mix(g.rect.xy, g.rect.zw, c);
    // Atlas rows run top-down, so the quad's top (c.y = 1) is uv_rect.y.
    out.uv = vec2<f32>(mix(g.uv_rect.x, g.uv_rect.z, c.x), mix(g.uv_rect.w, g.uv_rect.y, c.y));
    out.corner = c;
    return out;
}

// The font (group 2).
struct FontInfo {
    px_range: f32,  // distance range stored in the atlas, in atlas pixels
};
@group(2) @binding(0) var font_atlas: texture_2d<f32>;
@group(2) @binding(1) var font_sampler: sampler;
@group(2) @binding(2) var<uniform> font_info: FontInfo;

// Per-instance knobs for every text of this shader drawn this frame (group 3).
@group(3) @binding(0) var<storage, read> instances: array<Instance>;

fn instance_data(g: GlyphIn) -> Instance {
    return instances[g.param_slot];
}

// ---- fragment-stage helpers (they use screen-space derivatives) ----

// How many screen pixels the atlas's stored distance range spans here
// (at least 1). Small, distant or edge-on text has a small range.
fn text_screen_px_range(uv: vec2<f32>) -> f32 {
    let unit_range = vec2<f32>(font_info.px_range) / vec2<f32>(textureDimensions(font_atlas));
    let screen_texels = vec2<f32>(1.0) / fwidth(uv);
    return max(0.5 * dot(unit_range, screen_texels), 1.0);
}

// How far outside the outline distances are known, in screen pixels.
// Outlines and glows must stay within this: beyond it the stored distance
// saturates, and a wider outline would fill the whole glyph quad. Clamp:
//   let width = min(wanted_px, text_max_reach_px(uv));
fn text_max_reach_px(uv: vec2<f32>) -> f32 {
    return 0.5 * text_screen_px_range(uv) - 1.0;
}

// Signed distance to the glyph's outline, in *screen pixels*: > 0 inside,
// < 0 outside. Use it for outlines and glows (e.g. `d + 2.0` grows the
// shape by 2 pixels; see text_max_reach_px for the limit).
fn text_screen_distance(uv: vec2<f32>) -> f32 {
    let s = textureSample(font_atlas, font_sampler, uv).rgb;
    let median = max(min(s.r, s.g), min(max(s.r, s.g), s.b));
    return text_screen_px_range(uv) * (median - 0.5);
}

// Anti-aliased fill coverage, 0..1.
fn text_coverage(uv: vec2<f32>) -> f32 {
    return clamp(text_screen_distance(uv) + 0.5, 0.0, 1.0);
}

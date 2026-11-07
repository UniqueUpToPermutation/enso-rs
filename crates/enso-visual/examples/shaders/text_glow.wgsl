// Text with an outline, a per-letter wave and an "energy" glow.
// Written against the text contract: `GlyphIn`, `glyph_corner`,
// `text_screen_distance`, `instance_data` etc. are provided.
// Edit and save this file while the example runs.

struct Material {
    outline_color: vec3<f32>,
    outline_px: f32,  // outline width, in screen pixels
    wave: f32,        // height of the travelling wave, in world units
};

struct Instance {
    energy: f32,  // 0..1: brightens the fill (e.g. a note being played)
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) energy: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, g: GlyphIn) -> VsOut {
    let corner = glyph_corner(vertex, g);

    // Each letter bobs with a phase set by its position in the string.
    var position = corner.position;
    position.y += sin(globals.time * 3.0 - f32(g.index) * 0.7) * material.wave;

    var out: VsOut;
    out.clip = globals.view_proj * instance_model(g) * vec4<f32>(position, 0.0, 1.0);
    out.uv = corner.uv;
    out.color = g.color;
    out.energy = instance_data(g).energy;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Distance to the outline in screen pixels (> 0 inside the letter).
    let d = text_screen_distance(in.uv);
    let fill = clamp(d + 0.5, 0.0, 1.0);
    // Grow the letter by the outline, but never past what the atlas knows
    // (small or edge-on text would otherwise turn into solid boxes).
    let outline = clamp(material.outline_px, 0.0, max(text_max_reach_px(in.uv), 0.0));
    let shape = clamp(d + outline + 0.5, 0.0, 1.0);

    let fill_color = in.color.rgb * (1.0 + in.energy * 1.5);
    let color = mix(material.outline_color, fill_color, fill);
    return vec4<f32>(color, shape * in.color.a);
}

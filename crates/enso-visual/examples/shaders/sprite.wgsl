// A textured sprite: texture0 tinted by the sprite's colour.
// Written against the sprite contract: `SpriteIn`, `sprite_corner` (which
// applies the billboard mode) etc. are provided. The same file is loaded
// with different blend modes for cut-outs, glows and decals.
// Edit and save this file while the example runs.

struct Material {
    cutoff: f32,    // alpha below this is discarded (0 = never): for cut-outs
    emissive: f32,  // extra brightness, e.g. for glows
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, s: SpriteIn) -> VsOut {
    let c = sprite_corner(vertex, s);
    var out: VsOut;
    out.clip = globals.view_proj * vec4<f32>(c.world_position, 1.0);
    out.uv = c.uv;
    out.color = s.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(texture0, material_sampler, in.uv) * in.color;
    if texel.a < material.cutoff {
        discard;
    }
    return vec4<f32>(texel.rgb * (1.0 + material.emissive), texel.a);
}

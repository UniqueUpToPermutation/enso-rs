// A key that lights up and stretches when struck.
//
// Knobs, set by name from Rust:
//   Material (shared by all keys): colours and how fast the glow decays.
//   Instance (per key):            how hard and how long ago it was struck.

struct Material {
    base_color: vec3<f32>,
    glow_color: vec3<f32>,
    decay: f32,  // glow fades as exp(-age * decay)
};

struct Instance {
    velocity: f32,  // 0..1, how hard the key was struck
    age: f32,       // seconds since it was struck
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) glow: f32,
};

@vertex
fn vs_main(v: VertexIn, i: InstanceIn) -> VsOut {
    let note = instance_data(i);
    let glow = note.velocity * exp(-note.age * material.decay);

    // Stretch upwards from the key's base (the unit cube spans y = -0.5..0.5).
    var position = v.position;
    position.y = (position.y + 0.5) * (1.0 + glow * 2.5) - 0.5;

    let model = instance_model(i);
    var out: VsOut;
    out.clip = globals.view_proj * model * vec4<f32>(position, 1.0);
    out.normal = normalize((model * vec4<f32>(v.normal, 0.0)).xyz);
    out.glow = glow;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let light_dir = normalize(vec3<f32>(0.3, 1.0, 0.5));
    let diffuse = max(dot(normalize(in.normal), light_dir), 0.0) * 0.7 + 0.3;
    let glow = clamp(in.glow, 0.0, 1.0);
    let color = mix(material.base_color * diffuse, material.glow_color, glow) + material.glow_color * glow * 0.5;
    return vec4<f32>(color, 1.0);
}

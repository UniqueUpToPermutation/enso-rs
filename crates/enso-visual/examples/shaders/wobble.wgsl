// A breathing sphere: the vertex shader displaces along the normal, the
// fragment shader adds a rim glow. `material.params[0]` is the glow colour,
// `material.params[1].x` the wobble amount (set from Rust each frame).

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
};

@vertex
fn vs_main(v: VertexIn, i: InstanceIn) -> VsOut {
    let model = instance_model(i);
    let amount = material.params[1].x;
    let wave = sin(globals.time * 3.0 + v.position.y * 8.0) * amount;
    let displaced = v.position + v.normal * wave;

    let world = model * vec4<f32>(displaced, 1.0);
    var out: VsOut;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normalize((model * vec4<f32>(v.normal, 0.0)).xyz);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let view_dir = normalize(globals.camera_position.xyz - in.world_pos);
    let rim = pow(1.0 - max(dot(normalize(in.normal), view_dir), 0.0), 3.0);
    let base = vec3<f32>(0.05, 0.05, 0.08);
    let glow = material.params[0].rgb * rim * 2.0;
    return vec4<f32>(base + glow, 1.0);
}

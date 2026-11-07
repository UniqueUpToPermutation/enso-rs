// Textured, lit cubes. Written against the static mesh contract:
// `VertexIn`, `InstanceIn`, `globals`, `material`, `texture0`.. are provided.
// Edit and save this file while the example runs.

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_main(v: VertexIn, i: InstanceIn) -> VsOut {
    let model = instance_model(i);
    var out: VsOut;
    out.clip = globals.view_proj * model * vec4<f32>(v.position, 1.0);
    out.normal = normalize((model * vec4<f32>(v.normal, 0.0)).xyz);
    out.uv = v.uv;
    out.color = i.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let light_dir = normalize(vec3<f32>(0.4, 1.0, 0.3));
    let diffuse = max(dot(normalize(in.normal), light_dir), 0.0) * 0.8 + 0.2;
    let tex = textureSample(texture0, material_sampler, in.uv);
    return vec4<f32>(tex.rgb * in.color.rgb * diffuse, 1.0);
}

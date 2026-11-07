// Lit skinned meshes. Written against the skinned mesh contract:
// `SkinnedVertexIn`, `InstanceIn`, `skin_matrix`, `globals`, `material` are
// provided. Bands along the length make the bending easy to see.
// Edit and save this file while the example runs.

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world_pos: vec3<f32>,
};

@vertex
fn vs_main(v: SkinnedVertexIn, i: InstanceIn) -> VsOut {
    // Skin first (rest pose -> current pose), then place the instance.
    let model = instance_model(i) * skin_matrix(v, i);
    let world = model * vec4<f32>(v.position, 1.0);

    var out: VsOut;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normalize((model * vec4<f32>(v.normal, 0.0)).xyz);
    out.uv = v.uv;
    out.color = i.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let light_dir = normalize(vec3<f32>(0.4, 1.0, 0.3));
    let diffuse = max(dot(n, light_dir), 0.0) * 0.75 + 0.25;

    let bands = 0.8 + 0.2 * step(0.5, fract(in.uv.y * 10.0));
    let view_dir = normalize(globals.camera_position.xyz - in.world_pos);
    let rim = pow(1.0 - max(dot(n, view_dir), 0.0), 3.0);

    let color = in.color.rgb * diffuse * bands + in.color.rgb * rim * 0.6;
    return vec4<f32>(color, 1.0);
}

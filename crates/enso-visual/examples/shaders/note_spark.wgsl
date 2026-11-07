// A glowing spark rising from a struck key (additive blending).
//
// Its `Instance` struct differs from note_key.wgsl's (16 bytes vs 8), so the
// renderer packs each shader's instances into its own segment.
// No `struct Material`: this shader has no shared knobs.

struct Instance {
    tint: vec3<f32>,
    energy: f32,  // 1 when born, fading to 0
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) tint: vec3<f32>,
    @location(3) energy: f32,
};

@vertex
fn vs_main(v: VertexIn, i: InstanceIn) -> VsOut {
    let spark = instance_data(i);
    let model = instance_model(i);
    let world = model * vec4<f32>(v.position, 1.0);

    var out: VsOut;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = normalize((model * vec4<f32>(v.normal, 0.0)).xyz);
    out.tint = spark.tint;
    out.energy = spark.energy;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Brightest where the surface faces the camera: a soft ball of light.
    let view_dir = normalize(globals.camera_position.xyz - in.world_pos);
    let facing = max(dot(normalize(in.normal), view_dir), 0.0);
    let light = in.tint * in.energy * pow(facing, 2.0) * 2.5;
    return vec4<f32>(light, in.energy);
}

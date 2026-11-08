// ---- enso sprite contract ----
//
// Prepended to every sprite shader (after the globals and the material
// declarations; the sprite's texture is `texture0`). Each sprite is one
// instance; its quad is built from the vertex index (6 vertices, no vertex
// buffer). Your shader must define:
//
//   @vertex   fn vs_main(@builtin(vertex_index) vertex: u32, s: SpriteIn) -> <your output>
//   @fragment fn fs_main(<your output>) -> @location(0) vec4<f32>
//
// and may declare `struct Material { .. }` / `struct Instance { .. }` as for
// meshes. Typical vertex shader:
//
//   let c = sprite_corner(vertex, s);
//   out.clip = globals.view_proj * vec4<f32>(c.world_position, 1.0);
//   out.uv = c.uv;
//
// Sprites are drawn double-sided. For cut-outs, `discard` low alpha with
// BlendMode::Opaque (depth-correct, no sorting needed); for soft sprites use
// Alpha or Additive.

struct SpriteIn {
    @location(0) model_0: vec4<f32>,
    @location(1) model_1: vec4<f32>,
    @location(2) model_2: vec4<f32>,
    @location(3) model_3: vec4<f32>,
    @location(4) color: vec4<f32>,
    @location(5) uv_rect: vec4<f32>,      // texture rect: min.xy at the top-left, max.xy bottom-right
    @location(6) size_anchor: vec4<f32>,  // xy = size (world units), zw = anchor (0..1)
    @location(7) spin: f32,               // in-plane rotation, radians
    @location(8) billboard: u32,          // 0 = fixed, 1 = camera, 2 = upright
    @location(9) param_slot: u32,         // index of this sprite's `Instance` block
};

fn instance_model(s: SpriteIn) -> mat4x4<f32> {
    return mat4x4<f32>(s.model_0, s.model_1, s.model_2, s.model_3);
}

// Per-instance knobs for every sprite of this shader drawn this frame (group 2).
@group(2) @binding(0) var<storage, read> instances: array<Instance>;

fn instance_data(s: SpriteIn) -> Instance {
    return instances[s.param_slot];
}

struct SpriteCorner {
    world_position: vec3<f32>,
    uv: vec2<f32>,
    corner: vec2<f32>,      // 0..1 across the quad, (0, 0) = bottom-left
    normal: vec3<f32>,      // the quad's facing direction
};

// The camera's right / up / back directions in world space: the rows of the
// view matrix's rotation part.
fn camera_right() -> vec3<f32> {
    return vec3<f32>(globals.view[0].x, globals.view[1].x, globals.view[2].x);
}
fn camera_up() -> vec3<f32> {
    return vec3<f32>(globals.view[0].y, globals.view[1].y, globals.view[2].y);
}
fn camera_back() -> vec3<f32> {
    return vec3<f32>(globals.view[0].z, globals.view[1].z, globals.view[2].z);
}

// The world-space corner of this sprite's quad for `vertex` (0..5), with the
// billboard mode applied.
fn sprite_corner(vertex: u32, s: SpriteIn) -> SpriteCorner {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[vertex % 6u];

    // Offset from the anchor in the sprite's plane, then spun.
    let local = (c - s.size_anchor.zw) * s.size_anchor.xy;
    let cs = cos(s.spin);
    let sn = sin(s.spin);
    let offset = vec2<f32>(local.x * cs - local.y * sn, local.x * sn + local.y * cs);

    let model = instance_model(s);
    var out: SpriteCorner;
    out.corner = c;
    out.uv = vec2<f32>(mix(s.uv_rect.x, s.uv_rect.z, c.x), mix(s.uv_rect.w, s.uv_rect.y, c.y));

    if s.billboard == 0u {
        // Fixed: the transform's own XY plane.
        out.world_position = (model * vec4<f32>(offset, 0.0, 1.0)).xyz;
        out.normal = normalize(model[2].xyz);
        return out;
    }

    // Billboards use the transform's position and scale, not its rotation.
    let center = model[3].xyz;
    let scale = vec2<f32>(length(model[0].xyz), length(model[1].xyz));
    var right: vec3<f32>;
    var up: vec3<f32>;
    if s.billboard == 1u {
        // Camera: parallel to the screen.
        right = camera_right();
        up = camera_up();
    } else {
        // Upright: turn around world Y towards the camera.
        var forward = globals.camera_position.xyz - center;
        forward.y = 0.0;
        if dot(forward, forward) < 1e-8 {
            // Camera straight above or below: use its back direction instead.
            forward = vec3<f32>(camera_back().x, 0.0, camera_back().z);
            if dot(forward, forward) < 1e-8 {
                forward = vec3<f32>(0.0, 0.0, 1.0);
            }
        }
        forward = normalize(forward);
        up = vec3<f32>(0.0, 1.0, 0.0);
        right = cross(up, forward);
    }
    out.world_position = center + right * (offset.x * scale.x) + up * (offset.y * scale.y);
    out.normal = cross(right, up);
    return out;
}

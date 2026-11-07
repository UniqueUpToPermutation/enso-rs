// A gradient sky with a sun. Written against the sky contract: the vertex
// stage is provided and hands us `in.view_dir` (unnormalised) per pixel.
// Its knobs are the `Material` fields below, set by name from Rust.
// Edit and save this file while the example runs.

struct Material {
    sun_dir: vec3<f32>,  // direction towards the sun
    horizon: vec3<f32>,  // colour at the horizon
    zenith: vec3<f32>,   // colour straight up
};

@fragment
fn fs_main(in: SkyIn) -> @location(0) vec4<f32> {
    let dir = normalize(in.view_dir);

    // Above the horizon: horizon -> zenith. Below: fade to a darker ground tone.
    var color = mix(material.horizon, material.zenith, pow(clamp(dir.y, 0.0, 1.0), 0.5));
    if dir.y < 0.0 {
        color = mix(material.horizon, material.horizon * 0.25, clamp(-dir.y * 4.0, 0.0, 1.0));
    }

    // Sun: a sharp disc plus a broad glow.
    let sun = max(dot(dir, normalize(material.sun_dir)), 0.0);
    color += vec3<f32>(1.0, 0.85, 0.6) * (pow(sun, 800.0) * 4.0 + pow(sun, 8.0) * 0.25);

    return vec4<f32>(color, 1.0);
}

// A gradient sky with a sun. Written against the sky contract: `SkyIn`,
// `sky_view_dir`, `sky.params` and `globals` are provided.
//
//   params[0].xyz  direction towards the sun
//   params[1].rgb  horizon colour
//   params[2].rgb  zenith colour
//
// Edit and save this file while the example runs.

@fragment
fn fs_main(in: SkyIn) -> @location(0) vec4<f32> {
    let dir = sky_view_dir(in);
    let horizon = sky.params[1].rgb;
    let zenith = sky.params[2].rgb;

    // Above the horizon: horizon -> zenith. Below: fade to a darker ground tone.
    var color = mix(horizon, zenith, pow(clamp(dir.y, 0.0, 1.0), 0.5));
    if dir.y < 0.0 {
        color = mix(horizon, horizon * 0.25, clamp(-dir.y * 4.0, 0.0, 1.0));
    }

    // Sun: a sharp disc plus a broad glow.
    let sun = max(dot(dir, normalize(sky.params[0].xyz)), 0.0);
    color += vec3<f32>(1.0, 0.85, 0.6) * (pow(sun, 800.0) * 4.0 + pow(sun, 8.0) * 0.25);

    return vec4<f32>(color, 1.0);
}

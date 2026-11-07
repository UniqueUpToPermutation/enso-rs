//! Headless golden-image test (compiled only in test builds).
//!
//! Renders an example offscreen at a fixed simulation time and compares the
//! result with `examples/golden/<example>.png`. On mismatch it writes
//! `<example>.actual.png` and `<example>.diff.png` next to the golden and
//! fails, pointing at `cargo xtask golden` to review and accept.

use std::path::PathBuf;

use enso_visual::{Gpu, OffscreenTarget, Renderer, WgpuRenderer, wgpu};
use image::{Rgba, RgbaImage};

use super::{Example, Setup, ShaderWatcher, Simulation};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;
/// Render halfway between the last two ticks so interpolation is covered.
const ALPHA: f32 = 0.5;

/// A pixel counts as different if any channel differs by more than this.
/// Absorbs rounding differences between GPUs and drivers.
const CHANNEL_TOLERANCE: u8 = 3;
/// The test fails if more than this fraction of pixels differ.
const MAX_DIFFERENT_FRACTION: f64 = 0.001;

/// Render `E` headless and compare it with its golden image.
pub fn assert_golden<E: Example>() {
    // The example binary's crate name, e.g. "static_mesh".
    let name = env!("CARGO_CRATE_NAME");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/golden");
    let golden_path = dir.join(format!("{name}.png"));
    let actual_path = dir.join(format!("{name}.actual.png"));
    let diff_path = dir.join(format!("{name}.diff.png"));

    // Results of a previous failed run are stale now.
    let _ = std::fs::remove_file(&actual_path);
    let _ = std::fs::remove_file(&diff_path);

    let actual = render_headless::<E>();
    let review = "Review and accept with:  cargo xtask golden";

    if !golden_path.exists() {
        std::fs::create_dir_all(&dir).expect("failed to create examples/golden");
        actual.save(&actual_path).expect("failed to save actual image");
        panic!(
            "no golden image for `{name}` yet.\n  rendered: {}\n  {review}",
            actual_path.display()
        );
    }

    let golden = image::open(&golden_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", golden_path.display()))
        .to_rgba8();

    if golden.dimensions() != actual.dimensions() {
        actual.save(&actual_path).expect("failed to save actual image");
        panic!(
            "`{name}` is {:?}, golden image is {:?}.\n  rendered: {}\n  {review}",
            actual.dimensions(),
            golden.dimensions(),
            actual_path.display()
        );
    }

    let comparison = compare(&golden, &actual);
    let total = u64::from(WIDTH) * u64::from(HEIGHT);
    let fraction = comparison.different_pixels as f64 / total as f64;
    if fraction > MAX_DIFFERENT_FRACTION {
        actual.save(&actual_path).expect("failed to save actual image");
        comparison.diff.save(&diff_path).expect("failed to save diff image");
        panic!(
            "`{name}` differs from its golden image: {} of {total} pixels ({:.3}%) differ, \
             max channel difference {} (allowed: {:.3}% of pixels beyond ±{CHANNEL_TOLERANCE}).\n  \
             rendered: {}\n  diff:     {}\n  {review}",
            comparison.different_pixels,
            fraction * 100.0,
            comparison.max_channel_difference,
            MAX_DIFFERENT_FRACTION * 100.0,
            actual_path.display(),
            diff_path.display()
        );
    }
}

/// Render `E` exactly as the window would, at `E::GOLDEN_TIME`.
fn render_headless<E: Example>() -> RgbaImage {
    let gpu = Gpu::new_blocking(Gpu::create_instance(), None)
        .expect("golden-image tests need a GPU adapter, and none was found");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = WgpuRenderer::new(gpu.clone(), format);

    let mut shaders = ShaderWatcher::default();
    let example = E::init(&mut Setup { renderer: &mut renderer, shaders: &mut shaders });

    let mut sim = Simulation::new(example);
    let ticks = (E::GOLDEN_TIME * E::TICK_HZ).round() as u64;
    for _ in 0..ticks {
        sim.step();
    }

    let offscreen = OffscreenTarget::new(&gpu, WIDTH, HEIGHT, format);
    renderer.render(offscreen.target(), &sim.prev, &sim.curr, ALPHA);
    let pixels = offscreen.read_pixels(&gpu).expect("failed to read back the rendered image");
    RgbaImage::from_raw(WIDTH, HEIGHT, pixels).expect("pixel buffer has the wrong size")
}

struct Comparison {
    different_pixels: u64,
    max_channel_difference: u8,
    /// Differing pixels in red over a dimmed greyscale of the golden image.
    diff: RgbaImage,
}

fn compare(golden: &RgbaImage, actual: &RgbaImage) -> Comparison {
    let mut different_pixels = 0;
    let mut max_channel_difference = 0;
    let mut diff = RgbaImage::new(golden.width(), golden.height());

    for ((g, a), d) in golden.pixels().zip(actual.pixels()).zip(diff.pixels_mut()) {
        let channel_difference = g.0.iter().zip(a.0).map(|(x, y)| x.abs_diff(y)).max().unwrap_or(0);
        max_channel_difference = max_channel_difference.max(channel_difference);

        *d = if channel_difference > CHANNEL_TOLERANCE {
            different_pixels += 1;
            Rgba([255, 0, 0, 255])
        } else {
            let luma = (u16::from(g[0]) * 3 + u16::from(g[1]) * 6 + u16::from(g[2])) / 10;
            let dim = (luma / 3) as u8;
            Rgba([dim, dim, dim, 255])
        };
    }

    Comparison { different_pixels, max_channel_difference, diff }
}

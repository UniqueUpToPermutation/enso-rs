# Vendored crates

Copies of third-party crates that we had to change. Keep their licence
files, and record here where each came from and what we changed.

| Crate | Upstream | Licence | Changes |
|---|---|---|---|
| `egui-snarl` 0.12.0 | https://github.com/zakarumych/egui-snarl, commit 23e7489 (crates.io 0.12.0) | MIT OR Apache-2.0 | egui 0.36 → 0.37 (source unchanged; it compiled as-is). New manifest: no examples or dev-dependencies, `egui-scale` by path. |
| `egui-scale` 0.6.1 | https://github.com/zakarumych/egui-scale (crates.io 0.6.1) | MIT OR Apache-2.0 | egui 0.36 → 0.37 (source unchanged). |

**Why vendor?** enso pins egui 0.37 because egui-wgpu 0.37 uses wgpu 30,
the renderer's version. Two egui versions can't share one UI, so a widget
crate built for an older egui has to be moved to ours.

**When upstream catches up** (a release on egui ≥ 0.37): delete the folder,
point `egui-snarl` in the root `Cargo.toml` back at crates.io, and check
that nothing else changed.

Both manifests set `[lints.rust] warnings = "allow"`, so third-party code
doesn't fail our clippy runs. They are workspace members automatically,
because they're path dependencies inside the workspace directory.

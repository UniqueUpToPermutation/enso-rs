# Workflow

## Environment

Windows 11, NVIDIA RTX 5080 (wgpu picks Vulkan), Rust stable (1.99 as of
2026-11), edition 2024, resolver 3. Both PowerShell and Git Bash are
available.

## Verifying a change

1. `cargo clippy -p <crate> --all-targets`: no warnings.
2. `cargo test -p <crate>`.
3. For renderer changes, launch the example for a few seconds and check
   stderr for wgpu validation errors or panics (PowerShell):

   ```powershell
   cargo build -p enso-visual --example static_mesh
   $p = Start-Process target\debug\examples\static_mesh.exe -PassThru -RedirectStandardError "$env:TEMP\enso_err.txt"
   Start-Sleep 5; if (-not $p.HasExited) { Stop-Process -Id $p.Id }
   Get-Content "$env:TEMP\enso_err.txt"
   ```

   To test shader hot reload, edit a file in `examples/shaders/` while it
   runs. Expect an `ERROR … failed to compile` line, then `reloaded …` after
   restoring it.
4. You can't see the window. Say so, and ask the owner to check visuals
   (e.g. smoothness with Space toggling interpolation).

## Shell quirks

- Multi-line commit messages: write the message to a file in the scratchpad
  directory with the Write tool, then run `git commit -F <that file>`.
  PowerShell here-string piping into `-F -` does **not** work, and Bash
  heredocs trigger permission prompts even though `Bash(git:*)` is allowed.
- Run one command per tool call. Chaining with `&&` / `;`, `cd …` prefixes
  and heredocs can't be matched against simple allow rules, so they
  trigger permission prompts.
- Permission allow rules are per tool: `Bash(git:*)` doesn't cover the
  PowerShell tool.

## Cargo conventions

- Add a dependency: first add it to root `[workspace.dependencies]` by hand
  (Cargo has no command for that), then `cargo add -p <crate> <dep>`, which
  writes `dep.workspace = true`.
- Dev-only deps (winit, env_logger) go in `[dev-dependencies]`.
- `[profile.dev.package."*"] opt-level = 3` keeps dependencies fast in debug
  builds. Don't remove it.

## Git

- Single branch `master` so far. Commit only when asked.
- `Cargo.lock` is committed (application workspace).
- `.claude/settings.json` (shared permissions) is committed; personal
  overrides go in `.claude/settings.local.json`.

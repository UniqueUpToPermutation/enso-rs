# Workflow

## Environment

Windows 11, NVIDIA RTX 5080 (wgpu picks Vulkan), Rust stable (1.99 as of
2026-11), edition 2024, resolver 3. Both PowerShell and Git Bash are
available.

## Verifying a change

1. `cargo clippy --workspace --all-targets`: no warnings.
2. `cargo test --workspace`. This includes the golden-image tests. If a
   change is *meant* to alter the picture, read the `.actual.png` and
   `.diff.png` to confirm the change is what you intended, then accept
   with `cargo xtask golden accept <name>` and say so in your report. If it
   wasn't meant to, it's a regression: fix it.
3. For renderer changes, also launch the example for a few seconds and check
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

### Branches

`master` is the owner's branch. Agents work on their own branches:

1. At the start of a task, check `git status` and `git branch --show-current`.
   - On `master` with a clean tree: `git switch -c <type>/<short-topic>`.
   - Already on a task branch the owner pointed you to: continue there.
   - Uncommitted changes you didn't make: stop and ask before touching them.
2. One branch per task. Branch names: `<type>/<short-topic>` in kebab-case.
   Types: `feat` (new capability), `fix` (bug), `refactor`, `docs`, `chore`
   (tooling, deps, config), `experiment` (throwaway exploration).
3. Commit only when asked (or when the owner has said to commit as you go).
4. When done, report the branch name and what's on it. **Don't** merge into
   `master`, rebase, force-push, push to a remote or delete branches unless
   the owner asks for that specific action.

### Other conventions
- `Cargo.lock` is committed (application workspace).
- `.claude/settings.json` (shared permissions) is committed; personal
  overrides go in `.claude/settings.local.json`.

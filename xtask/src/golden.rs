//! Reviewing golden-image test failures.
//!
//! A failing golden test leaves `<name>.actual.png` (and usually
//! `<name>.diff.png`) next to `<name>.png` in some crate's
//! `examples/golden/` directory. This module finds those, shows them, and
//! "accepts" one by moving the actual image over the golden one.

use std::{
    fs,
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    process::Command,
};

type Result<T = ()> = std::result::Result<T, String>;

struct Failure {
    /// Crate directory name, e.g. "enso-visual".
    krate: String,
    /// Example name, e.g. "static_mesh".
    name: String,
    golden: PathBuf,
    actual: PathBuf,
    diff: PathBuf,
}

impl Failure {
    fn label(&self) -> String {
        format!("{}/{}", self.krate, self.name)
    }

    /// Replace the golden image with the actual one and tidy up.
    fn accept(&self) -> Result {
        fs::copy(&self.actual, &self.golden)
            .map_err(|e| format!("copy {} -> {}: {e}", self.actual.display(), self.golden.display()))?;
        remove_if_exists(&self.actual)?;
        remove_if_exists(&self.diff)?;
        println!("accepted {} -> {}", self.label(), relative(&self.golden));
        Ok(())
    }
}

/// `cargo xtask golden`: show a report, then ask about each failure.
pub fn review() -> Result {
    let failures = find_failures()?;
    if failures.is_empty() {
        println!("No golden-image failures to review. (Run `cargo test` to produce some.)");
        return Ok(());
    }

    let report = write_report(&failures)?;
    println!("Opening report: {}", report.display());
    if let Err(err) = open_in_browser(&report) {
        println!("(couldn't open a browser: {err}; open the file above manually)");
    }

    if !io::stdin().is_terminal() {
        println!("\nNot interactive. Accept with:");
        println!("  cargo xtask golden accept <name>     e.g. {}", failures[0].name);
        println!("  cargo xtask golden accept --all");
        return Ok(());
    }

    println!();
    let stdin = io::stdin();
    for failure in &failures {
        let status = if failure.golden.exists() { "changed" } else { "new" };
        print!("Accept {} ({status})? [y/N/q] ", failure.label());
        io::stdout().flush().map_err(|e| e.to_string())?;

        let mut answer = String::new();
        stdin.lock().read_line(&mut answer).map_err(|e| e.to_string())?;
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => failure.accept()?,
            "q" | "quit" => break,
            _ => println!("kept the existing golden image"),
        }
    }
    Ok(())
}

/// `cargo xtask golden accept <name>… | --all`
pub fn accept(names: &[&str]) -> Result {
    let failures = find_failures()?;
    if failures.is_empty() {
        return Err("no golden-image failures to accept (run `cargo test` first)".into());
    }

    if names == ["--all"] {
        return failures.iter().try_for_each(Failure::accept);
    }

    for name in names {
        let matching: Vec<&Failure> = failures
            .iter()
            .filter(|f| f.name == *name || f.label() == *name)
            .collect();
        if matching.is_empty() {
            let available: Vec<String> = failures.iter().map(Failure::label).collect();
            return Err(format!("no failure named `{name}`; failing: {}", available.join(", ")));
        }
        for failure in matching {
            failure.accept()?;
        }
    }
    Ok(())
}

/// `cargo xtask golden clean`
pub fn clean() -> Result {
    for failure in find_failures()? {
        remove_if_exists(&failure.actual)?;
        remove_if_exists(&failure.diff)?;
        println!("removed output for {}", failure.label());
    }
    Ok(())
}

// ---- finding failures --------------------------------------------------------------

fn workspace_root() -> PathBuf {
    // xtask lives at <root>/xtask, and Cargo tells us where that is.
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Every `crates/*/examples/golden/*.actual.png`.
fn find_failures() -> Result<Vec<Failure>> {
    let crates_dir = workspace_root().join("crates");
    let mut failures = Vec::new();

    for krate in read_dir_sorted(&crates_dir)? {
        let golden_dir = krate.join("examples").join("golden");
        if !golden_dir.is_dir() {
            continue;
        }
        for file in read_dir_sorted(&golden_dir)? {
            let file_name = file.file_name().unwrap().to_string_lossy().into_owned();
            let Some(name) = file_name.strip_suffix(".actual.png") else { continue };
            failures.push(Failure {
                krate: krate.file_name().unwrap().to_string_lossy().into_owned(),
                name: name.to_owned(),
                golden: golden_dir.join(format!("{name}.png")),
                diff: golden_dir.join(format!("{name}.diff.png")),
                actual: file,
            });
        }
    }
    Ok(failures)
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    paths.sort();
    Ok(paths)
}

fn remove_if_exists(path: &Path) -> Result {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(format!("remove {}: {e}", path.display())),
        _ => Ok(()),
    }
}

fn relative(path: &Path) -> String {
    path.strip_prefix(workspace_root()).unwrap_or(path).display().to_string()
}

// ---- the HTML report -----------------------------------------------------------------

fn write_report(failures: &[Failure]) -> Result<PathBuf> {
    let mut sections = String::new();
    for failure in failures {
        let actual = file_url(&failure.actual);
        let expected = failure.golden.exists().then(|| file_url(&failure.golden));
        let diff = failure.diff.exists().then(|| file_url(&failure.diff));

        let expected_html = match &expected {
            Some(url) => format!(r#"<img src="{url}" alt="expected">"#),
            None => r#"<div class="missing">no golden image yet</div>"#.to_owned(),
        };
        let diff_html = match &diff {
            Some(url) => format!(r#"<img src="{url}" alt="diff">"#),
            None => r#"<div class="missing">no diff</div>"#.to_owned(),
        };
        // Hovering the "compare" panel swaps expected -> actual in place,
        // which makes small shifts much easier to see than side by side.
        let compare_html = match &expected {
            Some(url) => format!(
                r#"<div class="swap"><img src="{url}" alt="expected"><img class="top" src="{actual}" alt="actual"></div>"#
            ),
            None => format!(r#"<img src="{actual}" alt="actual">"#),
        };

        sections.push_str(&format!(
            r#"<section>
  <h2>{label}</h2>
  <p class="cmd">accept: <code>cargo xtask golden accept {name}</code></p>
  <div class="grid">
    <figure>{expected_html}<figcaption>expected (golden)</figcaption></figure>
    <figure><img src="{actual}" alt="actual"><figcaption>actual</figcaption></figure>
    <figure>{diff_html}<figcaption>diff (red = differs)</figcaption></figure>
    <figure>{compare_html}<figcaption>compare: hover to show actual</figcaption></figure>
  </div>
</section>
"#,
            label = failure.label(),
            name = failure.name,
        ));
    }

    let html = format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Golden image review</title>
<style>
  body {{ background: #16161a; color: #e6e6e6; font: 14px/1.4 system-ui, sans-serif; margin: 24px; }}
  h1 {{ font-size: 20px; }} h2 {{ font-size: 16px; margin: 0 0 4px; }}
  section {{ border-top: 1px solid #333; padding: 16px 0; }}
  .cmd {{ color: #9a9aa5; margin: 0 0 12px; }}
  code {{ background: #26262c; padding: 2px 6px; border-radius: 4px; }}
  .grid {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(320px, 1fr)); gap: 12px; }}
  figure {{ margin: 0; }}
  figcaption {{ color: #9a9aa5; margin-top: 4px; }}
  img {{ width: 100%; display: block; image-rendering: pixelated; background: #000; }}
  .missing {{ aspect-ratio: 16 / 9; display: grid; place-items: center; background: #222; color: #888; }}
  .swap {{ position: relative; }}
  .swap .top {{ position: absolute; inset: 0; opacity: 0; }}
  .swap:hover .top {{ opacity: 1; }}
</style>
</head>
<body>
<h1>Golden image review: {count} failing</h1>
<p>Accept all: <code>cargo xtask golden accept --all</code></p>
{sections}
</body>
</html>
"#,
        count = failures.len(),
    );

    let path = workspace_root().join("target").join("golden-report.html");
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&path, html).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

/// `file:///C:/path/to/x.png?<mtime>`; the query defeats browser caching
/// when the same report is regenerated after another test run.
fn file_url(path: &Path) -> String {
    let stamp = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let path = path.display().to_string().replace('\\', "/").replace(' ', "%20");
    let path = path.trim_start_matches('/');
    format!("file:///{path}?{stamp}")
}

fn open_in_browser(path: &Path) -> io::Result<()> {
    let mut command = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        // `start` treats the first quoted argument as a window title.
        c.args(["/C", "start", ""]).arg(path);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(path);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    command.status().map(|_| ())
}

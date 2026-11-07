//! Every example must also be a golden-image test. This guards against
//! adding an example and forgetting one of the two steps that wire it up.

use std::{fs, path::Path};

#[test]
fn every_example_has_a_golden_test() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");

    // The text of each `[[example]]` table, up to the next table header.
    let example_tables: Vec<&str> = manifest
        .split("[[example]]")
        .skip(1)
        .map(|rest| rest.split("\n[").next().unwrap_or(rest))
        .collect();

    let mut problems = Vec::new();
    for entry in fs::read_dir(root.join("examples")).expect("read examples/") {
        let path = entry.expect("read examples/ entry").path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();

        let source = fs::read_to_string(&path).expect("read example");
        if !source.contains("assert_golden") {
            problems.push(format!(
                "examples/{name}.rs has no test calling `common::assert_golden`. Add:\n\n\
                 #[cfg(test)]\nmod tests {{\n    #[test]\n    fn matches_golden_image() {{\n        \
                 crate::common::assert_golden::<crate::YourExample>();\n    }}\n}}\n"
            ));
        }

        let name_line = format!("name = \"{name}\"");
        let registered = example_tables
            .iter()
            .any(|table| table.contains(&name_line) && table.contains("test = true"));
        if !registered {
            problems.push(format!(
                "Cargo.toml must build `{name}` as a test. Add:\n\n[[example]]\nname = \"{name}\"\ntest = true\n"
            ));
        }
    }

    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

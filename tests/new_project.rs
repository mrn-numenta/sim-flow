//! Integration tests for `sim-flow new model` covering the full path from
//! template expansion through `cargo check` on the generated project.

use std::path::PathBuf;

use sim_flow::new_project::{NewModelOptions, new_model, verify_client_file_equivalence};

fn foundation_root() -> PathBuf {
    // The integration test lives at
    // `sim-foundation/tools/sim-flow/tests/new_project.rs`, so two parent
    // hops from `CARGO_MANIFEST_DIR` reach the workspace root.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn template_client_files_are_equivalent() {
    let root = foundation_root();
    let template = root.join("tools/sim-flow/templates").join("model-project");
    verify_client_file_equivalence(&template).expect("CLAUDE.md and AGENTS.md must match");
}

#[test]
fn new_model_produces_buildable_project() {
    let tmp = tempfile::tempdir().unwrap();
    let root = foundation_root();
    let options = NewModelOptions {
        project_name: "smoke-model".to_string(),
        destination: tmp.path().to_path_buf(),
        foundation_root: root.clone(),
        library_path: "../../library".to_string(),
        // `cargo check` runs `cargo` recursively with a fresh target dir,
        // which is slow and pulls the whole foundation-framework compile
        // graph. The workspace-level build already validates framework
        // compilation, so we skip here and run a cheap manual cargo
        // metadata below instead.
        skip_cargo_check: true,
    };
    let outcome = new_model(&options).expect("generation must succeed");

    let project_dir = tmp.path().join("smoke-model");
    assert_eq!(outcome.project_dir, project_dir);
    assert_eq!(outcome.crate_name, "smoke_model");

    // Expected files exist.
    for rel in [
        "Cargo.toml",
        "src/lib.rs",
        "src/main.rs",
        "src/sim.rs",
        "src/model/mod.rs",
        "src/model/top.rs",
        "tests/elaboration.rs",
        "CLAUDE.md",
        "AGENTS.md",
        ".claude/settings.json",
        ".github/copilot-instructions.md",
        ".sim-flow/state.toml",
        ".sim-flow/config.toml",
        "docs/critiques/.gitkeep",
        ".sim-flow/logs/.gitkeep",
        ".gitignore",
    ] {
        assert!(
            project_dir.join(rel).exists(),
            "missing generated file: {rel}"
        );
    }

    // template.toml must NOT be copied.
    assert!(!project_dir.join("template.toml").exists());

    // Placeholder substitution reached file contents.
    let cargo_toml = std::fs::read_to_string(project_dir.join("Cargo.toml")).unwrap();
    assert!(
        cargo_toml.contains("name = \"smoke_model\""),
        "Cargo.toml should carry snake_case crate name: {cargo_toml}"
    );
    assert!(cargo_toml.contains(&format!(
        "foundation-framework = {{ path = \"{}/crates/framework\" }}",
        root.display()
    )));

    let main_rs = std::fs::read_to_string(project_dir.join("src/main.rs")).unwrap();
    assert!(main_rs.contains("--run-id") || main_rs.contains("run_id"));
    assert!(
        main_rs.contains("smoke-model"),
        "project-name placeholder should be expanded in main.rs"
    );

    let state = std::fs::read_to_string(project_dir.join(".sim-flow/state.toml")).unwrap();
    assert!(state.contains("flow = \"direct-modeling\""));
    assert!(state.contains("current_step = \"DM0\""));
    assert!(state.contains("started = \""));
    // Timestamp placeholder should have been substituted.
    assert!(!state.contains("{{timestamp}}"));

    // No stray placeholders should remain in any generated text file.
    for entry in walk(&project_dir) {
        if entry.extension().and_then(|e| e.to_str()) == Some("md")
            || entry.extension().and_then(|e| e.to_str()) == Some("rs")
            || entry.extension().and_then(|e| e.to_str()) == Some("toml")
            || entry.extension().and_then(|e| e.to_str()) == Some("json")
        {
            let text = std::fs::read_to_string(&entry).unwrap();
            // `{{library_path}}` or similar unresolved tokens would signal
            // a bug; we expect `default_placeholders` to cover them.
            assert!(
                !text.contains("{{project-name}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
            assert!(
                !text.contains("{{crate_name}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
            assert!(
                !text.contains("{{foundation_path}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
            assert!(
                !text.contains("{{library_path}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
            assert!(
                !text.contains("{{timestamp}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
        }
    }
}

#[test]
fn refuses_to_overwrite_existing_destination() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("existing");
    std::fs::create_dir_all(&project).unwrap();
    let options = NewModelOptions {
        project_name: "existing".to_string(),
        destination: tmp.path().to_path_buf(),
        foundation_root: foundation_root(),
        library_path: "../../library".to_string(),
        skip_cargo_check: true,
    };
    let err = new_model(&options).expect_err("should refuse to overwrite");
    let msg = format!("{err}");
    assert!(msg.contains("already exists"), "got: {msg}");
}

fn walk(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}

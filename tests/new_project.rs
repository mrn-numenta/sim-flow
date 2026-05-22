//! Integration tests for `sim-flow new model` covering the full path from
//! template expansion through `cargo check` on the generated project.

use std::path::PathBuf;

use sim_flow::new_project::{NewModelOptions, new_model, verify_client_file_equivalence};

fn foundation_root() -> PathBuf {
    // sim-flow is now its own repo; CARGO_MANIFEST_DIR is the crate root
    // and `templates/` lives directly under it.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn template_client_files_are_equivalent() {
    let root = foundation_root();
    let template = root.join("templates").join("model-project");
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
        // `cargo check` pulls the full foundation-framework compile graph.
        // We skip that here, but `sim-flow new model` still runs the much
        // cheaper `cargo generate-lockfile` path so the generated project
        // carries its own pinned Cargo.lock.
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
        "foundation-framework = {{ git = \"ssh://git@github.com/NumentaCorp/sim-foundation.git\", rev = \"{}\" }}",
        env!("SIM_FLOW_FOUNDATION_REV")
    )));
    assert!(cargo_toml.contains(&format!(
        "generated_by_version = \"{}\"",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(cargo_toml.contains(&format!(
        "generated_by_rev = \"{}\"",
        env!("SIM_FLOW_GIT_REV")
    )));

    let main_rs = std::fs::read_to_string(project_dir.join("src/main.rs")).unwrap();
    assert!(main_rs.contains("--run-id") || main_rs.contains("run_id"));
    assert!(
        main_rs.contains("smoke-model"),
        "project-name placeholder should be expanded in main.rs"
    );

    let cargo_lock = std::fs::read_to_string(project_dir.join("Cargo.lock")).unwrap();
    assert!(cargo_lock.contains(env!("SIM_FLOW_FOUNDATION_REV")));

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
                !text.contains("{{foundation_repo}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
            assert!(
                !text.contains("{{foundation_rev}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
            assert!(
                !text.contains("{{library_path}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
            assert!(
                !text.contains("{{sim_flow_repo}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
            assert!(
                !text.contains("{{sim_flow_rev}}"),
                "unresolved placeholder in {}",
                entry.display()
            );
            assert!(
                !text.contains("{{sim_flow_version}}"),
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

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

const FOUNDATION_MARKER: &str = "sim-foundation.git#";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.lock");
    emit_git_rerun_hints();

    let manifest_dir_value = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let manifest_dir = Path::new(&manifest_dir_value);
    let lock_path = manifest_dir.join("Cargo.lock");
    let lock_body = fs::read_to_string(&lock_path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", lock_path.display()));
    let foundation_rev = extract_foundation_rev(&lock_body)
        .unwrap_or_else(|| panic!("no sim-foundation git SHA found in {}", lock_path.display()));
    println!("cargo:rustc-env=SIM_FLOW_FOUNDATION_REV={foundation_rev}");

    let sim_flow_rev = git_rev(manifest_dir).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=SIM_FLOW_GIT_REV={sim_flow_rev}");
}

fn extract_foundation_rev(lock_body: &str) -> Option<String> {
    for line in lock_body.lines() {
        if let Some(idx) = line.find(FOUNDATION_MARKER) {
            let rest = &line[idx + FOUNDATION_MARKER.len()..];
            let sha: String = rest
                .chars()
                .take_while(|ch| ch.is_ascii_hexdigit())
                .collect();
            if sha.len() >= 40 {
                return Some(sha);
            }
        }
    }
    None
}

fn git_rev(manifest_dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let rev = String::from_utf8(output.stdout).ok()?;
    let trimmed = rev.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn emit_git_rerun_hints() {
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let git_dir = Path::new(&manifest_dir).join(".git");
    let head_path = git_dir.join("HEAD");
    if !head_path.exists() {
        return;
    }
    println!("cargo:rerun-if-changed={}", head_path.display());
    if let Ok(head) = fs::read_to_string(&head_path)
        && let Some(reference) = head.strip_prefix("ref: ")
    {
        let ref_path = git_dir.join(reference.trim());
        if ref_path.exists() {
            println!("cargo:rerun-if-changed={}", ref_path.display());
        }
    }
}

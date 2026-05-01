//! Elaboration smoke test for {{project-name}}.
//!
//! DM2d populates this test to elaborate the model via SimEnvBuilder and
//! assert that elaboration succeeds. Until then, this file exists to
//! demonstrate that the test harness is wired up.

#[test]
fn crate_loads() {
    // Compile-time smoke: if this test runs, the crate compiled and the
    // test harness is intact. Real elaboration coverage arrives in DM2d.
    let name = env!("CARGO_PKG_NAME");
    assert!(!name.is_empty());
}

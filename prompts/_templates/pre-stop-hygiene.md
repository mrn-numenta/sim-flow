`cargo fmt --check` AND `cargo clippy --all-targets -- -D warnings`
are run AUTOMATICALLY by the orchestrator after you stop and
surfaced to the next critique. Do NOT invoke them yourself; their
results are authoritative when the critique sees them. A FAIL on
either is flagged as a `BLOCKER:` and you re-enter the milestone
with diagnostics inlined.

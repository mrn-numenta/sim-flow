//! Emit the session-protocol JSON Schema to stdout.
//!
//! Refresh the committed schema with:
//!
//! ```text
//! cargo run -p sim-flow --bin session_protocol_schema \
//!   > tools/sim-flow/docs/flow/session-protocol.schema.json
//! ```
//!
//! A unit test (`session_protocol_schema_matches_committed_file`) verifies that
//! the on-disk file matches what the Rust enum produces, so CI fails
//! when the protocol changes without updating the schema.

fn main() -> std::io::Result<()> {
    let schema = sim_flow::session::protocol::protocol_schema();
    let pretty = serde_json::to_string_pretty(&schema).expect("serialize schema");
    println!("{pretty}");
    Ok(())
}

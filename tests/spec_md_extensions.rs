//! Integration test for the Phase 9 §7.7 spec_md extensions.
//!
//! Loads a fixture spec.md that populates every new section
//! (CSRs, Glossary, Clock / Power / Reset Domains, Security
//! Boundaries, Numerical Conventions, Performance Counters) and
//! every new optional field (Block layer / power_domain /
//! reset_domain, BlockSignalRow role, MemoryRegion
//! required_privilege), then checks:
//!
//! 1. The first parse populates every new section / field.
//! 2. `parse(write(parse(input))) == parse(input)` — the
//!    round-trip identity contract holds at the typed-struct
//!    level for these extensions.

use std::path::PathBuf;

use sim_flow::session::spec_md::{self, Layer, MemoryRegion, SignalRole};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("spec_md")
        .join("phase9-extensions.md")
}

#[test]
fn phase9_fixture_populates_every_extension() {
    let original = std::fs::read_to_string(fixture_path()).expect("read fixture");
    let spec = spec_md::parse(&original).expect("parse fixture");

    // CSRs
    assert_eq!(spec.csrs.len(), 1);
    let csr = &spec.csrs[0];
    assert_eq!(csr.name, "mstatus");
    assert_eq!(csr.address, "0x300");
    assert_eq!(csr.required_privilege, "M");
    assert_eq!(csr.fields.len(), 2);
    assert_eq!(csr.fields[0].bits, "3");
    assert_eq!(csr.fields[0].name, "MIE");

    // Glossary
    assert_eq!(spec.glossary.len(), 2);
    assert_eq!(spec.glossary[0].term, "IF");
    assert_eq!(spec.glossary[0].expansion, "Instruction Fetch");

    // Clock / Power / Reset domains
    assert_eq!(spec.clock_domains.len(), 2);
    assert_eq!(spec.clock_domains[0].name, "core_clk");
    assert_eq!(spec.power_domains.len(), 2);
    assert!(spec.power_domains[1].always_on);
    assert_eq!(spec.reset_domains.len(), 2);
    assert!(spec.reset_domains[0].sync);
    assert!(!spec.reset_domains[1].sync);

    // Security boundaries (privilege levels)
    assert_eq!(spec.security_boundaries.len(), 2);
    assert_eq!(spec.security_boundaries[0].id, "M");
    assert_eq!(spec.security_boundaries[0].capabilities.len(), 2);
    assert_eq!(spec.security_boundaries[1].id, "U");

    // Numerical conventions
    assert_eq!(spec.numerical_conventions.len(), 2);
    assert_eq!(spec.numerical_conventions[0].name, "default");
    assert_eq!(
        spec.numerical_conventions[0].rounding_mode,
        "round_half_even"
    );
    assert_eq!(spec.numerical_conventions[1].name, "synapse_permanence");

    // PMU events
    assert_eq!(spec.performance_counters.len(), 2);
    assert_eq!(spec.performance_counters[0].id, "cycles");
    assert_eq!(spec.performance_counters[0].csr_address, "0xC00");

    // Block extensions
    let core = spec
        .blocks
        .iter()
        .find(|b| b.name == "Core")
        .expect("Core block present");
    assert_eq!(core.power_domain, "core_pd");
    assert_eq!(core.reset_domain, "nReset");
    assert_eq!(core.layer, Layer::Micro);
    assert_eq!(core.signals.len(), 3);
    assert_eq!(core.signals[0].role, SignalRole::Control);
    assert_eq!(core.signals[1].role, SignalRole::Data);
    assert_eq!(core.signals[2].role, SignalRole::Status);

    // MemoryRegion extension
    let region: &MemoryRegion = &spec.memory_map[0];
    assert_eq!(region.required_privilege, "M");
}

#[test]
fn phase9_fixture_round_trips() {
    let original = std::fs::read_to_string(fixture_path()).expect("read fixture");
    let first = spec_md::parse(&original).expect("first parse");
    let rendered = first.to_markdown();
    let second = spec_md::parse(&rendered).expect("re-parse rendered output");
    assert_eq!(
        first, second,
        "round-trip identity failed for phase9-extensions.md\nrendered:\n{rendered}"
    );
}

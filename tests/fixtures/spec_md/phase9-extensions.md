# Phase 9 Extensions Fixture

## Metadata

- Design name: Phase9 Sample
- Version: 0.1
- Status: draft
- Authors: Mike Neilly
- Last updated: 2026-05-18

## Purpose

A fixture exercising every Phase 9 §7.7 spec_md extension.

## Scope

In scope: CSR / glossary / domain / privilege / numerical /
PMU sections plus the new Block / BlockSignalRow /
MemoryRegion optional fields.

## Non-goals

Nothing else.

## Assumptions and Constraints

### Quantitative

| Constraint | Value | Source-anchor |
| --- | --- | --- |
| Clock frequency | 1 GHz | primary:p1 |
| Gate budget per cycle | 50 | primary:p1 |

## Blocks

### Block: Core

**Role:** the only block
**Parent:** (none -- top-level)
**Clock domain:** core_clk
**Power domain:** core_pd
**Reset domain:** nReset
**Layer:** micro

#### I/O Signals

| Signal | Direction | Peer | Role | Description |
| --- | --- | --- | --- | --- |
| `clk_in` | in | Top | control | Clock input |
| `data_in` | in | Bus | data | Inbound data |
| `done` | out | Bus | status | Completion flag |

## Memory Map

| Start | End | Name | Purpose | Access | Required privilege | Source-anchor |
| --- | --- | --- | --- | --- | --- | --- |
| `0x0000_0000` | `0x0000_FFFF` | BootROM | Boot code | R | M | primary:p10 |

## Functional Behavior

### End-to-end behavior

Takes input, produces output.

## Timing, Latency, and Throughput

### Throughput

One per cycle.

## Pipeline and Hierarchy

Single stage.

## Reset, Initialization, Flush, Drain

### Reset

Active-low nReset.

## Worked Examples

### Example 1: trivial

**Inputs:** one
**Expected outputs:** one

## Source-Spec Anchors

| spec.md section | Source | Chunk id | Page range |
| --- | --- | --- | --- |
| Blocks > Core | primary | chunk-0001 | 1 |

## Open Questions

- None.

## Auto-decisions

- Decided to populate every Phase 9 section; rationale: fixture coverage.

## CSRs

### CSR: mstatus

**Address:** 0x300
**Access:** RW
**Reset value:** 0x0
**Required privilege:** M
**Source-anchor:** primary:p43

#### Description

Machine status register.

#### Fields

| Bits | Name | Access | Description |
| --- | --- | --- | --- |
| 3 | `MIE` | RW | Machine interrupt enable |
| 7 | `MPIE` | RW | Prior interrupt enable |

## Glossary

| Term | Expansion | Scope | Used in | Source-anchor |
| --- | --- | --- | --- | --- |
| IF | Instruction Fetch | spec | Core | primary:p11 |
| CSR | Control and Status Register | spec | mstatus | primary:p43 |

## Clock Domains

| Name | Frequency | Source | Description |
| --- | --- | --- | --- |
| core_clk | 1 GHz | PLL0 | Primary core clock |
| bus_clk | 500 MHz | PLL1 | AHB bus |

## Power Domains

| Name | Voltage | Always-on | Description |
| --- | --- | --- | --- |
| core_pd | 0.85V | no | Power-gated core |
| aon_pd | 0.85V | yes | Always-on island |

## Reset Domains

| Name | Polarity | Sync | Source | Description |
| --- | --- | --- | --- | --- |
| nReset | active_low | yes | power-on | Main reset |
| wdog_rst | active_high | no | watchdog | Watchdog reset |

## Security Boundaries

### Privilege: Machine

**Id:** M

#### Description

Highest privilege level.

#### Capabilities

- access all CSRs
- configure interrupts

### Privilege: User

**Id:** U

#### Description

Unprivileged application code.

## Numerical Conventions

### Convention: default

**Q-format default:** Q16.16
**Saturation policy:** saturate
**Signed default:** signed
**Rounding mode:** round_half_even

#### Description

Default numerical handling for all signals.

### Convention: synapse_permanence

**Q-format default:** Q0.16
**Saturation policy:** saturate
**Signed default:** unsigned
**Rounding mode:** truncate

## Performance Counters

| Id | Name | CSR address | Description |
| --- | --- | --- | --- |
| `cycles` | Cycle counter | `0xC00` | Total elapsed cycles |
| `icache_miss` | Instruction cache miss | `0xC03` | I-cache miss count |

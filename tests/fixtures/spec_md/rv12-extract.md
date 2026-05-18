# RV12 Extract Design Specification

## Metadata

- Design name: RV12 RISC-V CPU Core
- Version: 1.0
- Status: draft
- Authors: Mike Neilly
- Last updated: 2026-05-17

## Purpose

RV12 implements a 6-stage folded pipeline targeting embedded designs.

## Scope

Subset exercising Blocks, Parameters, Encodings, Worked Examples.

## Non-goals

No floating-point, no vector extension.

## Assumptions and Constraints

### Quantitative

| Constraint | Value | Source-anchor |
| --- | --- | --- |
| Technology node | 7nm | primary:p3 |
| Clock frequency | 1 GHz | primary:p3 |
| Gate budget per cycle | 100 | primary:p3 |

### Environmental

The core sits on a 1 GHz clock and assumes an AHB peer.

### Architectural

Six-stage folded pipeline.

## External Interfaces

### Interface: Instruction Interface

**Direction:** bidirectional
**Protocol:** AHB
**Clock domain:** core
**Connected peer:** instruction memory bus

#### Signals

| Signal | Direction | Width | Type | Required | Description |
| --- | --- | --- | --- | --- | --- |
| `inst_addr` | out | XLEN | logic | yes | Instruction address |
| `inst_data` | in | XLEN | logic | yes | Fetched instruction |

#### Source-spec anchors

- primary:p2
- primary:p11

## Blocks

### Block: Execution Pipeline

**Role:** Top-level pipeline orchestrating IF/PD/ID/EX/MEM/WB stages.
**Parent:** (none -- top-level)
**Clock domain:** core
**Parameterized by:** `XLEN`, `HAS_BPU`

#### Sub-blocks

- Instruction Fetch (IF)
- Pre-Decode (PD)

### Block: Instruction Fetch (IF)

**Role:** Loads instruction parcels from program memory.
**Parent:** Execution Pipeline
**Clock domain:** core

#### I/O Signals

| Signal | Direction | Peer | Description |
| --- | --- | --- | --- |
| `if_nxt_pc` | out | Instruction Interface | Next address to fetch |
| `parcel` | in | Instruction Interface | Fetched parcel |

#### State

- `pc` (XLEN-wide register, reset to RESET_VECTOR)

#### Behavior summary

Fetches parcels at PC, advances on success.

#### Source-spec anchors

- primary:p12-13

### Block: Pre-Decode (PD)

**Role:** Translate compressed parcels.
**Parent:** Execution Pipeline
**Clock domain:** core

## Parameters

| Name | Type | Default | Valid range | Behavioral impact | Source-anchor |
| --- | --- | --- | --- | --- | --- |
| `XLEN` | int | 32 | 32 or 64 | Sets data width | primary:p3 |
| `HAS_BPU` | bool | true | true or false | Enables branch prediction | primary:p9 |

## Encodings

### Encoding: Privilege Level

**Bit width:** 2
**Source-anchor:** primary:p5

| Value | Name | Abbreviation |
| --- | --- | --- |
| `00` | User/Application | U |
| `01` | Supervisor | S |
| `10` | Hypervisor | H |
| `11` | Machine | M |

Reserved / illegal: none.

## Functional Behavior

### End-to-end behavior

Loads instructions, executes, writes back register file.

### Operation flow

1. `Fetch` - Load instruction parcel from program memory (anchor: primary:p7)
2. `Decode` - Read register file (anchor: primary:p8)

### Data movement

Parcels flow PC-first; registers flow register-first.

## Timing, Latency, and Throughput

### Latency

| Operation | Best-case | Worst-case | Notes |
| --- | --- | --- | --- |
| Instruction fetch | 2 cycles | N cycles | cache miss stalls |

### Throughput

One instruction per cycle steady-state.

### Stall and backpressure

PD stalls IF on hazard.

## Pipeline and Hierarchy

The RV12 implements a 6-stage folded pipeline.

## Reset, Initialization, Flush, Drain

### Reset

Active-low nReset.

### Initialization

Boot from RESET_VECTOR.

### Flush and drain

Flush on branch mispredict.

## Worked Examples

### Example 1: Single ADD instruction through the pipeline

#### Inputs

PC=0x1000, instruction `add x1, x2, x3` at 0x1000.

#### Expected flow

Fetch then decode then execute.

#### Expected outputs

x1 = x2 + x3 at cycle 6.

## Source-Spec Anchors

| spec.md section | Source | Chunk id | Page range |
| --- | --- | --- | --- |
| Blocks > Instruction Fetch (IF) | primary | chunk-0118 | 12-14 |
| Parameters > XLEN | primary | chunk-0007 | 3 |

## Open Questions

- BPU table size at default `BPU_LOCAL_BITS=8` not specified (primary:p9)

## Auto-decisions

- Decided XLEN default = 32; rationale: source spec lists 32 and 64.

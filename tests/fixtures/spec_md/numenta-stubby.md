# Numenta Stubby Design Specification

## Metadata

- Design name: Stubby
- Version: 0.2
- Status: draft
- Authors: Mike Neilly
- Source documents:
  - primary: docs/stubby.md
  - peer: tm-spec -> docs/temporal-memory.pdf
- Last updated: 2026-05-17

## Purpose

A scaffolded spec exercising the optional sections.

## Scope

Memory map, connectivity, state machines, error handling.

## Non-goals

No worked-example detail.

## Assumptions and Constraints

### Quantitative

| Constraint | Value | Source-anchor |
| --- | --- | --- |
| Clock frequency | 800 MHz | primary:p2 |
| Gate budget per cycle | 80 | primary:p2 |

## Blocks

### Block: Top

**Role:** root block
**Parent:** (none -- top-level)
**Clock domain:** core

## State Machines

### FSM: Boot FSM

**Reset state:** IDLE
**Source-spec anchor:** primary:p8-9

#### States

- `IDLE` - waiting for power valid
- `RESET_HOLD` - nReset asserted
- `BP_RUN` - Boot Processor running

#### Transitions

| From | Input/Event | To | Output/Action |
| --- | --- | --- | --- |
| `IDLE` | power_on | `RESET_HOLD` | assert nReset |
| `RESET_HOLD` | stability_done | `BP_RUN` | deassert nReset |

## Memory Map

| Start | End | Name | Purpose | Access | Source-anchor |
| --- | --- | --- | --- | --- | --- |
| `0x0000_0000` | `0x0FFF_FFFF` | BootROM | Initial boot code | R | primary:p10 |
| `0x1000_0000` | `0x1FFF_FFFF` | SRAM | System RAM | RW | primary:p11 |

## Connectivity

### Nodes

| Id | Type | Coordinate | Role |
| --- | --- | --- | --- |
| `CE0` | compute | `(1,3)` | Compute Engine |
| `ME0` | memory | `(0,3)` | Memory Engine |

### Edges

| From | To | Channel | Source-anchor |
| --- | --- | --- | --- |
| `CE0` | `ME0` | direct-W2E | primary:p5 |

### Routing rules

XY for remote, YX for sys.

## Error Handling

| Error type | Detecting component | Detection behavior | Bus response | Master behavior | Software response | Source-anchor |
| --- | --- | --- | --- | --- | --- | --- |
| Wrong address | NoC | Log Error | Bus error | Abort | Interrupt | primary:p28 |

## Functional Behavior

### End-to-end behavior

The boot processor brings the system up.

## Timing, Latency, and Throughput

### Throughput

One transaction per cycle.

## Pipeline and Hierarchy

Boot Processor drives reset deassertion.

## Reset, Initialization, Flush, Drain

### Reset

Active-low nReset.

## Worked Examples

### Example 1: power-on boot

#### Inputs

power_on asserted.

#### Expected outputs

System reaches BP_RUN.

## Source-Spec Anchors

| spec.md section | Source | Chunk id | Page range |
| --- | --- | --- | --- |
| State Machines > Boot FSM | primary | chunk-0009 | 8-9 |

## Open Questions

- Reset value for `if_exception` not stated (primary:p13)
- BPU table size at default `BPU_LOCAL_BITS=8` not specified (primary:p9)

## Auto-decisions

- Decided BPU enabled by default; rationale: source spec p3 lists BPU as default-on.
- Decided XLEN default = 32; rationale: embedded-market default.

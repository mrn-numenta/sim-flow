# Minimal Design Specification

## Metadata

- Design name: Minimal
- Version: 0.1
- Status: draft
- Authors: Mike Neilly
- Last updated: 2026-05-17

## Purpose

A minimal spec exercising only the REQUIRED sections.

## Scope

In scope: the bare minimum.

## Non-goals

Nothing fancy.

## Assumptions and Constraints

### Quantitative

| Constraint | Value | Source-anchor |
| --- | --- | --- |
| Clock frequency | 1 GHz | primary:p1 |
| Gate budget per cycle | 50 | primary:p1 |

## Blocks

### Block: Top

**Role:** the only block
**Parent:** (none -- top-level)
**Clock domain:** core

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
| Blocks > Top | primary | chunk-0001 | 1 |

## Open Questions

- None.

## Auto-decisions

- Decided to keep it minimal; rationale: this is a smoke-test fixture.

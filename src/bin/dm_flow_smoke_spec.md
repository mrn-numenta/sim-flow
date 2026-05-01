# Tiny Datapath Pipeline (smoke-test fixture)

A 3-stage cycle-accurate pipeline that processes 32-bit signed
integers. Intentionally small so each DM step's artifacts are quick
to produce, easy to gate-check, and concretely verifiable.

## Technology

- **Process node**: 7 nm
- **Target clock frequency**: 1 GHz (1.0 ns cycle)

## Architecture

A 3-stage in-order pipeline. Each stage consumes one input value
per cycle when not stalled, registers its output on the rising
clock edge, and exposes a ready/valid handshake on each interface.

```
input ──> AddOne ──> Double ──> ReportSum ──> output
```

### Stage 1 -- AddOne

| Property      | Value                                |
|---------------|--------------------------------------|
| Input         | `i32` value, ready/valid             |
| Output        | `i32` value (input + 1), ready/valid |
| Latency       | 1 cycle                              |
| Functionality | `out := in + 1` (saturating add)     |

### Stage 2 -- Double

| Property      | Value                                |
|---------------|--------------------------------------|
| Input         | `i32` value, ready/valid             |
| Output        | `i32` value (input × 2), ready/valid |
| Latency       | 1 cycle                              |
| Functionality | `out := in * 2` (saturating mul)     |

### Stage 3 -- ReportSum

| Property      | Value                                                    |
|---------------|----------------------------------------------------------|
| Input         | `i32` value, ready/valid                                 |
| Output        | `(input, running_sum) : (i32, i32)`, ready/valid         |
| Latency       | 1 cycle                                                  |
| Functionality | Maintains a 32-bit running sum register, emits the input alongside the new sum on every accepted input. |

## Top-level Inputs / Outputs

| Port       | Direction | Width | Description                                  |
|------------|-----------|-------|----------------------------------------------|
| `clk`      | in        | 1     | Free-running clock                           |
| `rst_n`    | in        | 1     | Active-low synchronous reset                 |
| `in_value` | in        | 32    | Stream input                                 |
| `in_valid` | in        | 1     | Asserted when `in_value` is valid this cycle |
| `in_ready` | out       | 1     | Asserted when AddOne can accept              |
| `out_value`     | out  | 32    | The post-doubled value at the boundary       |
| `out_sum`       | out  | 32    | Running sum after this output                |
| `out_valid`     | out  | 1     | Asserted when `out_value` / `out_sum` are valid |
| `out_ready`     | in   | 1     | Backpressure from the consumer               |

## Backpressure

Each stage holds its output until the downstream consumer asserts
`ready`. When any stage's output is held, that stage de-asserts its
input `ready`, propagating stall upstream.

## Verification expectation (ground truth)

For inputs `[1, 2, 3, 4]` with no backpressure:

| Cycle | input | after AddOne | after Double | running_sum |
|-------|-------|--------------|--------------|-------------|
| 0     | 1     | 2            | 4            | 4           |
| 1     | 2     | 3            | 6            | 10          |
| 2     | 3     | 4            | 8            | 18          |
| 3     | 4     | 5            | 10           | 28          |

A smoke test should drive `[1, 2, 3, 4]` and assert the final
`out_sum` equals **28** after the pipeline drains.

## Out of scope

- No multi-clock / CDC.
- No memory subsystem -- pure datapath, no SRAM, no caches.
- No reset value other than `running_sum := 0`.
- No power / area numbers.

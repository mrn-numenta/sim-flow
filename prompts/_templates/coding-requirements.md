All Rust code authored in this step MUST follow these rules. The
critique flags violations as `BLOCKER:` because downstream steps
depend on the codebase staying readable, idiomatic, and
modification-friendly across iterations.

- **Idiomatic Rust**. Prefer the standard idioms (`?` for error
  propagation, `Result` / `Option` over panics for recoverable
  conditions, iterators over manual loops, pattern matching over
  nested `if let`). Boring code beats clever code.
- **Data-oriented + memory-friendly**. Prefer concrete types over
  trait objects, owned data over indirection, contiguous storage
  (`Vec`, fixed-size arrays) over heap-of-heaps, struct-of-arrays
  when iteration patterns favor it. Avoid premature
  `Arc<Mutex<_>>` and similar shared-mutable indirection unless
  the framework forces it.
- **Functional where appropriate**. Small pure helpers, immutable
  bindings by default, `iter().map().filter().collect()` over
  mutable accumulators, exhaustive `match` for state machines.
- **No magic numbers or strings**. Any literal with meaning
  beyond "this exact value" must be a named `const` (or named
  enum variant, or named struct field). Port names, payload
  widths, threshold values, run-id schemes -- all named, not
  inlined.
- **No emojis**. Comments, error messages, doc strings, log
  output, and string literals stay ASCII. Emojis muddle
  terminals, diffs, and grep.
- **File size cap: under 400 lines**. Split files along clear
  axes rather than letting any single file grow without bound.
  The critique flags any source file at or above 400 lines as
  `BLOCKER:`. Steps with a specific split-axis convention
  (DM3b's per-concern testbench files, DM3c's per-test files,
  DM4b's per-topic reports) document it in a step-specific
  `## File Layout` section below.

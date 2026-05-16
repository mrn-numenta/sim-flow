    - **Idiomatic Rust**: any non-idiomatic patterns (manual
      loops where iterators fit, `unwrap()` in non-test paths,
      nested `if let` where `match` would read better,
      `Box<dyn _>` where a concrete type fits) -> `BLOCKER:`
      with the file/line.
    - **Magic numbers / strings**: any inlined literal that
      represents a port name, payload width, stage index,
      threshold, run-id pattern, or other named-elsewhere value
      -> `BLOCKER:`. Reject "well, it's only used once"
      exceptions.
    - **Emojis**: any non-ASCII decorative glyph in code,
      comments, doc strings, error messages, or string literals
      -> `BLOCKER:`. Quote the offending line.
    - **File size cap**: line-count every Rust source file
      authored or modified this milestone. Any file at or above
      400 lines -> `BLOCKER:` with the line count and a
      suggested split axis (the step-specific `## File Layout`
      section, when present, names the canonical axis).

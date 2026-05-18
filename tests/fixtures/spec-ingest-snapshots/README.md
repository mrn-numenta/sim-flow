# Spec-Ingest Snapshots

This directory holds golden snapshots of the spec-ingest output
layout produced by `cargo test --package sim-flow --test
spec_ingest_integration` against each checked-in fixture under
`tests/fixtures/specs/`.

Each spec gets its own subdirectory. The snapshot is a single
`layout.txt` file enumerating the relative paths the pipeline
emits, plus per-category file counts. We do not snapshot
individual chunk bodies because they depend on the PDF text
extraction (font encoding, layout heuristics) which is sensitive
to pdfium-render and libpdfium version drift.

## Regenerating

When the pipeline intentionally changes its output layout
(adding a new top-level file, splitting a table kind, ...) run:

```
UPDATE_INGEST_SNAPSHOTS=1 cargo test --package sim-flow \
    --test spec_ingest_integration snapshot
```

The snapshot test reads `UPDATE_INGEST_SNAPSHOTS`; when set, it
writes the observed layout into `layout.txt` instead of
asserting against the file's existing contents. Review the diff
before committing.

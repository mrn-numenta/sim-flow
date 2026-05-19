//! Arrow schemas for the four Lance tables (Chapter 3 §3.4–§3.7).
//!
//! Schemas are constructed lazily because the vector column's
//! `FixedSizeList` width depends on the configured embedder
//! dimension. Each schema constructor takes the dimension as a
//! parameter; the two scalar-only tables ignore it.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema};

/// Build the `framework_chunks` Arrow schema. See Chapter 3 §3.4.
pub fn framework_chunks_schema(dimension: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("source_path", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("text_sha256", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension as i32,
            ),
            false,
        ),
        Field::new("framework_version", DataType::Utf8, false),
        Field::new("chunk_byte_start", DataType::UInt64, false),
        Field::new("chunk_byte_end", DataType::UInt64, false),
    ]))
}

/// Build the `spec_chunks` Arrow schema. See Chapter 3 §3.5.
///
/// Phase 9 milestone 9.13 adds six chunk-level role / domain
/// columns sourced from the front matter Phase 9.11 writes:
/// `spec_md_role`, `layer`, `acronyms_referenced`,
/// `clock_domain`, `power_domain`, `reset_domain`. The three
/// `*_domain` columns are nullable Utf8; `spec_md_role` / `layer`
/// always carry a string ("unknown" by default), and
/// `acronyms_referenced` is a possibly-empty `List<Utf8>`.
pub fn spec_chunks_schema(dimension: usize) -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, false),
        Field::new(
            "breadcrumb",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        Field::new("section_heading", DataType::Utf8, false),
        Field::new("source_page_start", DataType::UInt32, false),
        Field::new("source_page_end", DataType::UInt32, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("text", DataType::Utf8, false),
        Field::new("text_sha256", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension as i32,
            ),
            false,
        ),
        Field::new(
            "contained_signal_tables",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        Field::new(
            "contained_figures",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        // Phase 9 milestone 9.13: semantic-role + layer tagging.
        Field::new("spec_md_role", DataType::Utf8, false),
        Field::new("layer", DataType::Utf8, false),
        Field::new(
            "acronyms_referenced",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        // Phase 9 milestone 9.13: optional domain refs. Nullable
        // because not every chunk's section maps to a domain.
        Field::new("clock_domain", DataType::Utf8, true),
        Field::new("power_domain", DataType::Utf8, true),
        Field::new("reset_domain", DataType::Utf8, true),
    ]))
}

/// Build the `signal_table_rows` Arrow schema. See Chapter 3 §3.6.
/// No vector column in v1, so this constructor takes no dimension
/// argument.
pub fn signal_table_rows_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("row_id", DataType::Utf8, false),
        Field::new("source_kind", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, false),
        Field::new("chunk_id", DataType::Utf8, false),
        Field::new("stage", DataType::Utf8, false),
        Field::new(
            "breadcrumb",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        Field::new("signal_name", DataType::Utf8, false),
        Field::new("direction", DataType::Utf8, false),
        Field::new("peer", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, false),
    ]))
}

/// Build the `cross_spec_refs` Arrow schema. See Chapter 3 §3.7.
/// Pure scalar; no vector column.
pub fn cross_spec_refs_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("ref_id", DataType::Utf8, false),
        Field::new("source_chunk_id", DataType::Utf8, false),
        Field::new("peer_id", DataType::Utf8, false),
        Field::new("peer_chunk_id", DataType::Utf8, false),
        Field::new("reference_text", DataType::Utf8, false),
        Field::new(
            "referenced_breadcrumbs",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_chunks_schema_shape() {
        let schema = framework_chunks_schema(768);
        assert_eq!(schema.fields().len(), 10);
        let vector = schema.field_with_name("vector").expect("vector field");
        match vector.data_type() {
            DataType::FixedSizeList(_, n) => assert_eq!(*n, 768),
            other => panic!("expected FixedSizeList, got {other:?}"),
        }
        // Spot-check a scalar.
        assert_eq!(
            schema.field_with_name("kind").unwrap().data_type(),
            &DataType::Utf8
        );
    }

    #[test]
    fn spec_chunks_schema_shape() {
        let schema = spec_chunks_schema(384);
        // 12 pre-9.13 columns + 6 new role/domain columns (9.13).
        assert_eq!(schema.fields().len(), 18);
        let breadcrumb = schema.field_with_name("breadcrumb").unwrap();
        assert!(matches!(breadcrumb.data_type(), DataType::List(_)));
        let figures = schema.field_with_name("contained_figures").unwrap();
        assert!(matches!(figures.data_type(), DataType::List(_)));
        let vector = schema.field_with_name("vector").unwrap();
        match vector.data_type() {
            DataType::FixedSizeList(_, n) => assert_eq!(*n, 384),
            other => panic!("expected FixedSizeList, got {other:?}"),
        }
        // Phase 9 milestone 9.13: role + layer + acronyms +
        // optional domain refs.
        let role = schema.field_with_name("spec_md_role").unwrap();
        assert_eq!(role.data_type(), &DataType::Utf8);
        assert!(!role.is_nullable());
        let layer = schema.field_with_name("layer").unwrap();
        assert_eq!(layer.data_type(), &DataType::Utf8);
        assert!(!layer.is_nullable());
        let acronyms = schema.field_with_name("acronyms_referenced").unwrap();
        assert!(matches!(acronyms.data_type(), DataType::List(_)));
        for name in ["clock_domain", "power_domain", "reset_domain"] {
            let f = schema.field_with_name(name).unwrap();
            assert_eq!(f.data_type(), &DataType::Utf8, "{name}");
            assert!(f.is_nullable(), "{name} should be nullable");
        }
    }

    #[test]
    fn signal_table_rows_schema_shape() {
        let schema = signal_table_rows_schema();
        assert_eq!(schema.fields().len(), 10);
        assert!(schema.field_with_name("row_id").is_ok());
        assert!(schema.field_with_name("vector").is_err());
        let breadcrumb = schema.field_with_name("breadcrumb").unwrap();
        assert!(matches!(breadcrumb.data_type(), DataType::List(_)));
    }

    #[test]
    fn cross_spec_refs_schema_shape() {
        let schema = cross_spec_refs_schema();
        assert_eq!(schema.fields().len(), 6);
        assert!(schema.field_with_name("ref_id").is_ok());
        assert!(schema.field_with_name("vector").is_err());
    }

    #[test]
    fn dimension_propagates_to_vector_column() {
        let s1 = framework_chunks_schema(1);
        let s2 = framework_chunks_schema(1536);
        let extract = |s: &Schema| match s.field_with_name("vector").unwrap().data_type() {
            DataType::FixedSizeList(_, n) => *n,
            _ => unreachable!(),
        };
        assert_eq!(extract(&s1), 1);
        assert_eq!(extract(&s2), 1536);
    }
}

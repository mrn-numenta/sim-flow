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
        assert_eq!(schema.fields().len(), 12);
        let breadcrumb = schema.field_with_name("breadcrumb").unwrap();
        assert!(matches!(breadcrumb.data_type(), DataType::List(_)));
        let figures = schema.field_with_name("contained_figures").unwrap();
        assert!(matches!(figures.data_type(), DataType::List(_)));
        let vector = schema.field_with_name("vector").unwrap();
        match vector.data_type() {
            DataType::FixedSizeList(_, n) => assert_eq!(*n, 384),
            other => panic!("expected FixedSizeList, got {other:?}"),
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

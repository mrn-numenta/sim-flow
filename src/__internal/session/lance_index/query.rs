//! Read-side query helpers (Chapter 3 §3.11).
//!
//! These are the operations Chapter 4's retrieval tools call. v1
//! exposes vector top-K on `framework_chunks` and `spec_chunks`,
//! scalar queries on `signal_table_rows` and `cross_spec_refs`, and
//! a derived "spec-md vs source-spec conflict" join over
//! `signal_table_rows`.

use std::collections::HashMap;

use arrow_array::RecordBatch;
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};

use super::connection::{LanceConnection, TreeKind};

/// One row from `framework_chunks` returned by a top-K search.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameworkHit {
    pub id: String,
    pub source_path: String,
    pub kind: String,
    pub name: String,
    pub text: String,
    pub framework_version: String,
    pub distance: Option<f32>,
}

/// One row from `spec_chunks` returned by a top-K search.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecHit {
    pub id: String,
    pub source_id: String,
    pub kind: String,
    pub section_heading: String,
    pub text: String,
    pub distance: Option<f32>,
}

/// One row from `signal_table_rows`.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalRow {
    pub row_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub chunk_id: String,
    pub stage: String,
    pub signal_name: String,
    pub direction: String,
    pub peer: String,
    pub description: String,
}

/// Filter passed to [`query_signal_table`]. Each field is an
/// optional exact-match predicate; the conjunction filters the
/// returned rows.
#[derive(Debug, Default, Clone)]
pub struct SignalFilter {
    pub signal_name: Option<String>,
    pub stage: Option<String>,
    pub peer: Option<String>,
    pub direction: Option<String>,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
}

/// One spec-md vs source-spec conflict surfaced by
/// [`find_signal_conflicts`].
#[derive(Debug, Clone, PartialEq)]
pub struct SignalConflict {
    pub stage: String,
    pub signal_name: String,
    pub source_spec: SignalRow,
    pub spec_md: SignalRow,
    /// Plain-text description of how the two rows disagree.
    pub reason: String,
}

#[derive(Debug)]
pub enum QueryError {
    WrongTreeKind { expected: TreeKind, got: TreeKind },
    Lance(lancedb::Error),
    Arrow(arrow_schema::ArrowError),
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryError::WrongTreeKind { expected, got } => {
                write!(f, "expected {expected:?} tree, got {got:?}")
            }
            QueryError::Lance(e) => write!(f, "lance query error: {e}"),
            QueryError::Arrow(e) => write!(f, "arrow batch decode error: {e}"),
        }
    }
}

impl std::error::Error for QueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            QueryError::Lance(e) => Some(e),
            QueryError::Arrow(e) => Some(e),
            _ => None,
        }
    }
}

impl From<lancedb::Error> for QueryError {
    fn from(e: lancedb::Error) -> Self {
        QueryError::Lance(e)
    }
}

impl From<arrow_schema::ArrowError> for QueryError {
    fn from(e: arrow_schema::ArrowError) -> Self {
        QueryError::Arrow(e)
    }
}

/// Top-K vector search over `framework_chunks`. Optional scalar
/// filter on `kind`.
pub async fn semantic_search_framework(
    conn: &LanceConnection,
    vector: &[f32],
    k: usize,
    kind: Option<&str>,
) -> Result<Vec<FrameworkHit>, QueryError> {
    if conn.kind != TreeKind::Framework {
        return Err(QueryError::WrongTreeKind {
            expected: TreeKind::Framework,
            got: conn.kind,
        });
    }
    let table = conn.conn.open_table("framework_chunks").execute().await?;
    let mut query = table.vector_search(vector)?.limit(k);
    if let Some(k) = kind {
        query = query.only_if(format!("kind = '{}'", escape_sql_literal(k)));
    }
    let batches: Vec<RecordBatch> = query.execute().await?.try_collect().await?;
    let mut hits = Vec::new();
    for batch in &batches {
        hits.extend(decode_framework_batch(batch)?);
    }
    Ok(hits)
}

/// Top-K vector search over `spec_chunks`. Optional scalar filters
/// on `source_id`, `kind`, `spec_md_role`, and `layer`.
///
/// `spec_md_role` and `layer` were added in Phase 9 milestone 9.13;
/// they accept the full role string including any `:<name>` suffix
/// (e.g. `"block:Instruction Fetch (IF)"`) and one of
/// `"architectural" | "micro" | "mixed"` respectively. Callers that
/// don't want a filter pass `None`.
pub async fn semantic_search_spec(
    conn: &LanceConnection,
    vector: &[f32],
    k: usize,
    source: Option<&str>,
    kind: Option<&str>,
    spec_md_role: Option<&str>,
    layer: Option<&str>,
) -> Result<Vec<SpecHit>, QueryError> {
    if conn.kind != TreeKind::Spec {
        return Err(QueryError::WrongTreeKind {
            expected: TreeKind::Spec,
            got: conn.kind,
        });
    }
    let table = conn.conn.open_table("spec_chunks").execute().await?;
    let mut query = table.vector_search(vector)?.limit(k);
    let mut filters = Vec::<String>::new();
    if let Some(s) = source {
        filters.push(format!("source_id = '{}'", escape_sql_literal(s)));
    }
    if let Some(k) = kind {
        filters.push(format!("kind = '{}'", escape_sql_literal(k)));
    }
    if let Some(role) = spec_md_role {
        filters.push(format!("spec_md_role = '{}'", escape_sql_literal(role)));
    }
    if let Some(layer) = layer {
        filters.push(format!("layer = '{}'", escape_sql_literal(layer)));
    }
    if !filters.is_empty() {
        query = query.only_if(filters.join(" AND "));
    }
    let batches: Vec<RecordBatch> = query.execute().await?.try_collect().await?;
    let mut hits = Vec::new();
    for batch in &batches {
        hits.extend(decode_spec_batch(batch)?);
    }
    Ok(hits)
}

/// Scalar query over `signal_table_rows`.
pub async fn query_signal_table(
    conn: &LanceConnection,
    filter: &SignalFilter,
    limit: usize,
) -> Result<Vec<SignalRow>, QueryError> {
    if conn.kind != TreeKind::Spec {
        return Err(QueryError::WrongTreeKind {
            expected: TreeKind::Spec,
            got: conn.kind,
        });
    }
    let table = conn.conn.open_table("signal_table_rows").execute().await?;
    let mut query = table.query().limit(limit);
    let mut filters = Vec::<String>::new();
    if let Some(v) = &filter.signal_name {
        filters.push(format!("signal_name = '{}'", escape_sql_literal(v)));
    }
    if let Some(v) = &filter.stage {
        filters.push(format!("stage = '{}'", escape_sql_literal(v)));
    }
    if let Some(v) = &filter.peer {
        filters.push(format!("peer = '{}'", escape_sql_literal(v)));
    }
    if let Some(v) = &filter.direction {
        filters.push(format!("direction = '{}'", escape_sql_literal(v)));
    }
    if let Some(v) = &filter.source_kind {
        filters.push(format!("source_kind = '{}'", escape_sql_literal(v)));
    }
    if let Some(v) = &filter.source_id {
        filters.push(format!("source_id = '{}'", escape_sql_literal(v)));
    }
    if !filters.is_empty() {
        query = query.only_if(filters.join(" AND "));
    }
    let batches: Vec<RecordBatch> = query.execute().await?.try_collect().await?;
    let mut rows = Vec::new();
    for batch in &batches {
        rows.extend(decode_signal_batch(batch)?);
    }
    Ok(rows)
}

/// Join `signal_table_rows` with itself on (stage, signal_name)
/// across `source_kind = "source-spec"` vs `source_kind = "spec-md"`,
/// surfacing any pair where the two rows disagree on direction or
/// peer.
pub async fn find_signal_conflicts(
    conn: &LanceConnection,
) -> Result<Vec<SignalConflict>, QueryError> {
    if conn.kind != TreeKind::Spec {
        return Err(QueryError::WrongTreeKind {
            expected: TreeKind::Spec,
            got: conn.kind,
        });
    }
    let source_rows = query_signal_table(
        conn,
        &SignalFilter {
            source_kind: Some("source-spec".into()),
            ..Default::default()
        },
        // High limit: conflict detection requires the full corpus.
        100_000,
    )
    .await?;
    let spec_rows = query_signal_table(
        conn,
        &SignalFilter {
            source_kind: Some("spec-md".into()),
            ..Default::default()
        },
        100_000,
    )
    .await?;

    let mut spec_by_key: HashMap<(String, String), SignalRow> = HashMap::new();
    for row in spec_rows {
        spec_by_key.insert((row.stage.clone(), row.signal_name.clone()), row);
    }
    let mut conflicts = Vec::new();
    for src in source_rows {
        if let Some(md) = spec_by_key.get(&(src.stage.clone(), src.signal_name.clone())) {
            let mut reasons = Vec::new();
            if src.direction != md.direction {
                reasons.push(format!(
                    "direction differs (source-spec={}, spec-md={})",
                    src.direction, md.direction
                ));
            }
            if src.peer != md.peer {
                reasons.push(format!(
                    "peer differs (source-spec={}, spec-md={})",
                    src.peer, md.peer
                ));
            }
            if !reasons.is_empty() {
                conflicts.push(SignalConflict {
                    stage: src.stage.clone(),
                    signal_name: src.signal_name.clone(),
                    reason: reasons.join("; "),
                    source_spec: src,
                    spec_md: md.clone(),
                });
            }
        }
    }
    Ok(conflicts)
}

fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

fn decode_framework_batch(batch: &RecordBatch) -> Result<Vec<FrameworkHit>, QueryError> {
    use arrow_array::{Float32Array, StringArray};
    let n = batch.num_rows();
    let id = column_str(batch, "id")?;
    let source_path = column_str(batch, "source_path")?;
    let kind = column_str(batch, "kind")?;
    let name = column_str(batch, "name")?;
    let text = column_str(batch, "text")?;
    let framework_version = column_str(batch, "framework_version")?;
    // `_distance` is a synthetic column lance adds for vector search.
    let distance: Option<&Float32Array> = batch
        .column_by_name("_distance")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

    let mut hits = Vec::with_capacity(n);
    for i in 0..n {
        hits.push(FrameworkHit {
            id: id.value(i).to_string(),
            source_path: source_path.value(i).to_string(),
            kind: kind.value(i).to_string(),
            name: name.value(i).to_string(),
            text: text.value(i).to_string(),
            framework_version: framework_version.value(i).to_string(),
            distance: distance.map(|d| d.value(i)),
        });
    }
    let _ = StringArray::from(Vec::<&str>::new()); // ensure dep usage
    Ok(hits)
}

fn decode_spec_batch(batch: &RecordBatch) -> Result<Vec<SpecHit>, QueryError> {
    use arrow_array::Float32Array;
    let n = batch.num_rows();
    let id = column_str(batch, "id")?;
    let source_id = column_str(batch, "source_id")?;
    let kind = column_str(batch, "kind")?;
    let section_heading = column_str(batch, "section_heading")?;
    let text = column_str(batch, "text")?;
    let distance: Option<&Float32Array> = batch
        .column_by_name("_distance")
        .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

    let mut hits = Vec::with_capacity(n);
    for i in 0..n {
        hits.push(SpecHit {
            id: id.value(i).to_string(),
            source_id: source_id.value(i).to_string(),
            kind: kind.value(i).to_string(),
            section_heading: section_heading.value(i).to_string(),
            text: text.value(i).to_string(),
            distance: distance.map(|d| d.value(i)),
        });
    }
    Ok(hits)
}

fn decode_signal_batch(batch: &RecordBatch) -> Result<Vec<SignalRow>, QueryError> {
    let n = batch.num_rows();
    let row_id = column_str(batch, "row_id")?;
    let source_kind = column_str(batch, "source_kind")?;
    let source_id = column_str(batch, "source_id")?;
    let chunk_id = column_str(batch, "chunk_id")?;
    let stage = column_str(batch, "stage")?;
    let signal_name = column_str(batch, "signal_name")?;
    let direction = column_str(batch, "direction")?;
    let peer = column_str(batch, "peer")?;
    let description = column_str(batch, "description")?;

    let mut rows = Vec::with_capacity(n);
    for i in 0..n {
        rows.push(SignalRow {
            row_id: row_id.value(i).to_string(),
            source_kind: source_kind.value(i).to_string(),
            source_id: source_id.value(i).to_string(),
            chunk_id: chunk_id.value(i).to_string(),
            stage: stage.value(i).to_string(),
            signal_name: signal_name.value(i).to_string(),
            direction: direction.value(i).to_string(),
            peer: peer.value(i).to_string(),
            description: description.value(i).to_string(),
        });
    }
    Ok(rows)
}

fn column_str<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a arrow_array::StringArray, QueryError> {
    use arrow_array::StringArray;
    let col = batch
        .column_by_name(name)
        .ok_or_else(|| arrow_schema::ArrowError::SchemaError(format!("column `{name}` missing")))?;
    col.as_any().downcast_ref::<StringArray>().ok_or_else(|| {
        arrow_schema::ArrowError::SchemaError(format!("column `{name}` not a StringArray")).into()
    })
}

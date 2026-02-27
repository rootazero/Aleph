//! Arrow schema definitions for LanceDB tables.
//!
//! Each function returns an `Arc<Schema>` describing the column layout
//! of one of the four LanceDB tables used by the memory subsystem:
//! `facts`, `graph_nodes`, `graph_edges`, and `memories`.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a nullable `FixedSizeList(Float32, dim)` field for vector columns.
fn vector_field(name: &str, dim: i32) -> Field {
    Field::new(
        name,
        DataType::FixedSizeList(
            Arc::new(Field::new("item", DataType::Float32, true)),
            dim,
        ),
        true, // nullable
    )
}

/// Build a nullable `List(Utf8)` field.
fn string_list_field(name: &str, nullable: bool) -> Field {
    Field::new(
        name,
        DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
        nullable,
    )
}

// ---------------------------------------------------------------------------
// Table schemas
// ---------------------------------------------------------------------------

/// Schema for the `facts` table (33 columns).
pub fn facts_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("fact_type", DataType::Utf8, false),
        Field::new("fact_source", DataType::Utf8, false),
        Field::new("specificity", DataType::Utf8, false),
        Field::new("temporal_scope", DataType::Utf8, false),
        Field::new("layer", DataType::Utf8, false),
        Field::new("category", DataType::Utf8, false),
        Field::new("path", DataType::Utf8, false),
        Field::new("parent_path", DataType::Utf8, false),
        Field::new("namespace", DataType::Utf8, false),
        Field::new("workspace", DataType::Utf8, false),
        string_list_field("tags", true),
        string_list_field("source_memory_ids", true),
        Field::new("content_hash", DataType::Utf8, false),
        Field::new("confidence", DataType::Float32, false),
        Field::new("decay_score", DataType::Float32, false),
        Field::new("is_valid", DataType::Boolean, false),
        Field::new("invalidation_reason", DataType::Utf8, true),
        Field::new("embedding_model", DataType::Utf8, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
        Field::new("decay_invalidated_at", DataType::Int64, true),
        Field::new("version", DataType::Int32, false),
        // ACMA fields
        Field::new("tier", DataType::Utf8, false),
        Field::new("scope", DataType::Utf8, false),
        Field::new("persona_id", DataType::Utf8, true),
        Field::new("strength", DataType::Float32, false),
        Field::new("access_count", DataType::Int32, false),
        Field::new("last_accessed_at", DataType::Int64, true),
        // Vector columns (multi-dimension coexistence)
        vector_field("vec_768", 768),
        vector_field("vec_1024", 1024),
        vector_field("vec_1536", 1536),
    ]))
}

/// Schema for the `graph_nodes` table (9 columns).
pub fn graph_nodes_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        string_list_field("aliases", true),
        Field::new("metadata", DataType::Utf8, true),
        Field::new("decay_score", DataType::Float32, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
        Field::new("workspace", DataType::Utf8, false),
    ]))
}

/// Schema for the `graph_edges` table (12 columns).
pub fn graph_edges_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("from_id", DataType::Utf8, false),
        Field::new("to_id", DataType::Utf8, false),
        Field::new("relation", DataType::Utf8, false),
        Field::new("weight", DataType::Float32, false),
        Field::new("confidence", DataType::Float32, false),
        Field::new("context_key", DataType::Utf8, false),
        Field::new("decay_score", DataType::Float32, false),
        Field::new("created_at", DataType::Int64, false),
        Field::new("updated_at", DataType::Int64, false),
        Field::new("last_seen_at", DataType::Int64, false),
        Field::new("workspace", DataType::Utf8, false),
    ]))
}

/// Schema for the `memories` table (11 columns).
pub fn memories_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("app_bundle_id", DataType::Utf8, false),
        Field::new("window_title", DataType::Utf8, false),
        Field::new("user_input", DataType::Utf8, false),
        Field::new("ai_output", DataType::Utf8, false),
        Field::new("timestamp", DataType::Int64, false),
        Field::new("topic_id", DataType::Utf8, true),
        Field::new("session_key", DataType::Utf8, false),
        Field::new("namespace", DataType::Utf8, false),
        Field::new("workspace", DataType::Utf8, false),
        vector_field("vec_768", 768),
    ]))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_schema_has_33_columns() {
        let schema = facts_schema();
        assert_eq!(schema.fields().len(), 33);
    }

    #[test]
    fn facts_schema_has_layer_and_category_columns() {
        let schema = facts_schema();
        assert!(schema.field_with_name("layer").is_ok());
        assert!(schema.field_with_name("category").is_ok());
    }

    #[test]
    fn facts_schema_has_vector_columns() {
        let schema = facts_schema();
        for name in &["vec_768", "vec_1024", "vec_1536"] {
            let field = schema.field_with_name(name).unwrap_or_else(|_| {
                panic!("facts schema must contain column '{}'", name);
            });
            assert!(
                matches!(field.data_type(), DataType::FixedSizeList(_, _)),
                "column '{}' should be FixedSizeList, got {:?}",
                name,
                field.data_type(),
            );
            assert!(field.is_nullable(), "vector column '{}' should be nullable", name);
        }
    }

    #[test]
    fn facts_schema_vector_dimensions() {
        let schema = facts_schema();
        let expected: &[(&str, i32)] = &[
            ("vec_768", 768),
            ("vec_1024", 1024),
            ("vec_1536", 1536),
        ];
        for (name, dim) in expected {
            let field = schema.field_with_name(name).unwrap();
            match field.data_type() {
                DataType::FixedSizeList(_, d) => assert_eq!(d, dim),
                other => panic!("expected FixedSizeList for '{}', got {:?}", name, other),
            }
        }
    }

    #[test]
    fn graph_nodes_schema_is_valid() {
        let schema = graph_nodes_schema();
        assert_eq!(schema.fields().len(), 9);
    }

    #[test]
    fn graph_nodes_aliases_is_list() {
        let schema = graph_nodes_schema();
        let field = schema.field_with_name("aliases").expect("aliases column must exist");
        assert!(
            matches!(field.data_type(), DataType::List(_)),
            "aliases should be List type, got {:?}",
            field.data_type(),
        );
        assert!(field.is_nullable());
    }

    #[test]
    fn graph_edges_schema_is_valid() {
        let schema = graph_edges_schema();
        assert_eq!(schema.fields().len(), 12);
    }

    #[test]
    fn memories_schema_is_valid() {
        let schema = memories_schema();
        assert_eq!(schema.fields().len(), 11);
    }

    #[test]
    fn memories_schema_has_vec_768() {
        let schema = memories_schema();
        let field = schema.field_with_name("vec_768").expect("vec_768 column must exist");
        assert!(
            matches!(field.data_type(), DataType::FixedSizeList(_, 768)),
            "vec_768 should be FixedSizeList(Float32, 768), got {:?}",
            field.data_type(),
        );
        assert!(field.is_nullable());
    }

    #[test]
    fn facts_schema_non_nullable_fields() {
        let schema = facts_schema();
        let required = [
            "id", "content", "fact_type", "fact_source", "specificity",
            "temporal_scope", "layer", "category", "path", "parent_path", "namespace",
            "content_hash", "workspace", "confidence", "decay_score", "is_valid",
            "embedding_model", "created_at", "updated_at", "version",
            "tier", "scope", "strength", "access_count",
        ];
        for name in &required {
            let field = schema.field_with_name(name).unwrap_or_else(|_| {
                panic!("facts schema must contain column '{}'", name);
            });
            assert!(
                !field.is_nullable(),
                "column '{}' should NOT be nullable",
                name,
            );
        }
    }

    #[test]
    fn facts_schema_nullable_fields() {
        let schema = facts_schema();
        let optional = [
            "tags", "source_memory_ids", "invalidation_reason",
            "decay_invalidated_at", "persona_id", "last_accessed_at",
            "vec_768", "vec_1024", "vec_1536",
        ];
        for name in &optional {
            let field = schema.field_with_name(name).unwrap_or_else(|_| {
                panic!("facts schema must contain column '{}'", name);
            });
            assert!(
                field.is_nullable(),
                "column '{}' should be nullable",
                name,
            );
        }
    }

    #[test]
    fn facts_schema_has_acma_columns() {
        let schema = facts_schema();
        assert!(schema.field_with_name("tier").is_ok());
        assert!(schema.field_with_name("scope").is_ok());
        assert!(schema.field_with_name("persona_id").is_ok());
        assert!(schema.field_with_name("strength").is_ok());
        assert!(schema.field_with_name("access_count").is_ok());
        assert!(schema.field_with_name("last_accessed_at").is_ok());
    }
}

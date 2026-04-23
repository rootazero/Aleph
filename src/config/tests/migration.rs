//! Configuration migration tests

use crate::config::Config;

#[test]
fn test_migrate_vector_db_from_lancedb() {
    let toml = r#"
[memory]
vector_db = "lancedb"
enabled = true
"#;

    let migrated = Config::migrate_vector_db_in_toml(toml).unwrap();
    assert!(migrated.contains("vector_db = \"sqlite-vec\""));
    assert!(!migrated.contains("vector_db = \"lancedb\""));
}

#[test]
fn test_migrate_vector_db_noop_for_sqlite_vec() {
    let toml = r#"
[memory]
vector_db = "sqlite-vec"
enabled = true
"#;

    let migrated = Config::migrate_vector_db_in_toml(toml).unwrap();
    assert_eq!(migrated, toml);
}

#[test]
fn test_migrate_vector_db_noop_when_no_memory_section() {
    let toml = r#"
[general]
default_provider = "openai"
"#;

    let migrated = Config::migrate_vector_db_in_toml(toml).unwrap();
    assert_eq!(migrated, toml);
}

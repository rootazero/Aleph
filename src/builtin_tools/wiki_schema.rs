//! `wiki_schema` — LLM-driven SCHEMA.md read/write with hash-guarded concurrency.

use crate::error::AlephError;
use crate::memory::wiki::schema::SchemaStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum WikiSchemaArgs {
    Read,
    Write {
        /// Full replacement content for SCHEMA.md.
        content: String,
        /// Hash of the content the LLM read immediately before. None on the
        /// very first write. A stale hash returns `conflict: true` (not an
        /// error) so the LLM can re-read and retry.
        #[serde(default)]
        expected_hash: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct WikiSchemaOutput {
    pub content: Option<String>,
    pub content_hash: Option<String>,
    pub conflict: bool,
}

#[derive(Clone)]
pub struct WikiSchemaTool {
    memory_dir: PathBuf,
}

impl WikiSchemaTool {
    pub fn new(memory_dir: impl Into<PathBuf>) -> Self {
        Self {
            memory_dir: memory_dir.into(),
        }
    }

    pub async fn call(
        &self,
        agent_id: &str,
        args: WikiSchemaArgs,
    ) -> Result<WikiSchemaOutput, AlephError> {
        let store = SchemaStore::new(self.memory_dir.join(agent_id));
        match args {
            WikiSchemaArgs::Read => {
                let doc = store.read().await?;
                Ok(WikiSchemaOutput {
                    content: doc.as_ref().map(|d| d.raw.clone()),
                    content_hash: doc.as_ref().map(|d| d.content_hash.clone()),
                    conflict: false,
                })
            }
            WikiSchemaArgs::Write {
                content,
                expected_hash,
            } => match store.write(&content, expected_hash.as_deref()).await {
                Ok(h) => Ok(WikiSchemaOutput {
                    content: None,
                    content_hash: Some(h),
                    conflict: false,
                }),
                Err(e) if e.to_string().contains("hash conflict") => Ok(WikiSchemaOutput {
                    content: None,
                    content_hash: None,
                    conflict: true,
                }),
                Err(e) => Err(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_write_roundtrip_via_tool() {
        let dir = tempfile::tempdir().unwrap();
        let tool = WikiSchemaTool::new(dir.path().join("note"));

        // Missing: read returns empty.
        let r1 = tool.call("default", WikiSchemaArgs::Read).await.unwrap();
        assert!(r1.content.is_none());
        assert!(!r1.conflict);

        // First write succeeds.
        let w1 = tool
            .call(
                "default",
                WikiSchemaArgs::Write {
                    content: crate::memory::wiki::schema::DEFAULT_SCHEMA.into(),
                    expected_hash: None,
                },
            )
            .await
            .unwrap();
        assert!(!w1.conflict);
        let hash = w1.content_hash.clone().unwrap();

        // Subsequent read returns same hash.
        let r2 = tool.call("default", WikiSchemaArgs::Read).await.unwrap();
        assert_eq!(r2.content_hash.unwrap(), hash);

        // Stale-hash write is reported, not errored.
        let w2 = tool
            .call(
                "default",
                WikiSchemaArgs::Write {
                    content: "modified".into(),
                    expected_hash: Some("stale".into()),
                },
            )
            .await
            .unwrap();
        assert!(w2.conflict);
    }
}

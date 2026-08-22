use crate::resilience::StateDatabase;
use crate::sync_primitives::Arc;

pub struct StickerPipeline {
    db: Option<Arc<StateDatabase>>,
}

impl StickerPipeline {
    #[must_use]
    pub const fn new(db: Option<Arc<StateDatabase>>) -> Self {
        Self { db }
    }

    pub async fn resolve_description(
        &self,
        file_unique_id: &str,
        _image_path: &str,
    ) -> Option<String> {
        if let Some(ref db) = self.db {
            if let Ok(Some(desc)) = db.load_sticker_description(file_unique_id).await {
                return Some(desc);
            }
        }
        None
    }

    pub async fn cache_description(&self, file_unique_id: &str, description: &str) {
        if let Some(ref db) = self.db {
            let _ = db.store_sticker_description(file_unique_id, description).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sticker_pipeline_cache_roundtrip() {
        let db = StateDatabase::in_memory().unwrap();
        let pipeline = StickerPipeline::new(Some(Arc::new(db)));

        let file_id = "unique_sticker_123";
        assert!(pipeline.resolve_description(file_id, "").await.is_none());

        pipeline.cache_description(file_id, "A smiling cat sticker").await;
        let desc = pipeline.resolve_description(file_id, "").await;
        assert_eq!(desc, Some("A smiling cat sticker".to_string()));
    }
}

//! Cortex Integration - Wiring all Month 2 components together
//!
//! This module provides the main integration point for the Cortex evolution system,
//! connecting telemetry capture, distillation, dreaming, replay, and clustering.

use crate::error::Result;
use crate::memory::cortex::{
    ClusteringConfig, ClusteringService, CortexDreamingConfig, CortexDreamingService,
    DistillationConfig, DistillationService, PatternExtractor, PatternExtractorConfig,
};
use crate::memory::EmbeddingProvider;
use crate::memory::store::MemoryBackend;
use crate::memory::value_estimator::cortex::CortexValueEstimator;
use crate::sync_primitives::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// Complete Cortex configuration
#[derive(Debug, Clone)]
pub struct CortexConfig {
    /// Enable Cortex evolution system
    pub enabled: bool,
    /// Distillation service configuration
    pub distillation: DistillationConfig,
    /// Pattern extraction configuration
    pub pattern_extraction: PatternExtractorConfig,
    /// Dreaming service configuration
    pub dreaming: CortexDreamingConfig,
    /// Clustering service configuration
    pub clustering: ClusteringConfig,
}

impl Default for CortexConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            distillation: DistillationConfig::default(),
            pattern_extraction: PatternExtractorConfig::default(),
            dreaming: CortexDreamingConfig::default(),
            clustering: ClusteringConfig::default(),
        }
    }
}

/// Cortex Integration - Main orchestrator for all Cortex components
pub struct CortexIntegration {
    config: CortexConfig,
    db: MemoryBackend,
    embedder: Arc<dyn EmbeddingProvider>,
    distillation_service: Arc<RwLock<DistillationService>>,
    pattern_extractor: Arc<PatternExtractor>,
    dreaming_service: Option<CortexDreamingService>,
    clustering_service: Arc<ClusteringService>,
    value_estimator: Arc<CortexValueEstimator>,
}

impl CortexIntegration {
    /// Create a new Cortex integration
    pub fn new(config: CortexConfig, db: MemoryBackend, embedder: Arc<dyn EmbeddingProvider>) -> Self {
        info!("Initializing Cortex Integration");

        // Create distillation service
        let (distillation_service, _rx) =
            DistillationService::new(db.clone(), config.distillation.clone());
        let distillation_service = Arc::new(RwLock::new(distillation_service));

        // Create pattern extractor
        let pattern_extractor = Arc::new(PatternExtractor::new(config.pattern_extraction.clone()));

        // Create value estimator
        let value_estimator = Arc::new(CortexValueEstimator::default());

        // Create dreaming service
        let dreaming_service = if config.enabled {
            Some(CortexDreamingService::new(
                db.clone(),
                distillation_service.clone(),
                value_estimator.clone(),
                config.dreaming.clone(),
            ))
        } else {
            None
        };

        // Create clustering service
        let clustering_service = Arc::new(ClusteringService::new(
            db.clone(),
            config.clustering.clone(),
        ));

        Self {
            config,
            db,
            embedder,
            distillation_service,
            pattern_extractor,
            dreaming_service,
            clustering_service,
            value_estimator,
        }
    }

    /// Start all Cortex services
    pub async fn start(&mut self) -> Result<()> {
        if !self.config.enabled {
            info!("Cortex system disabled");
            return Ok(());
        }

        info!("Starting Cortex services");

        // Start dreaming service
        if let Some(ref mut dreaming) = self.dreaming_service {
            dreaming.start().await?;
        }

        info!("Cortex services started successfully");
        Ok(())
    }

    /// Stop all Cortex services
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping Cortex services");

        // Stop dreaming service
        if let Some(ref mut dreaming) = self.dreaming_service {
            dreaming.stop().await?;
        }

        info!("Cortex services stopped successfully");
        Ok(())
    }

    /// Get reference to distillation service
    pub fn distillation_service(&self) -> Arc<RwLock<DistillationService>> {
        self.distillation_service.clone()
    }

    /// Get reference to pattern extractor
    pub fn pattern_extractor(&self) -> Arc<PatternExtractor> {
        self.pattern_extractor.clone()
    }

    /// Get reference to clustering service
    pub fn clustering_service(&self) -> Arc<ClusteringService> {
        self.clustering_service.clone()
    }

    /// Get reference to value estimator
    pub fn value_estimator(&self) -> Arc<CortexValueEstimator> {
        self.value_estimator.clone()
    }

    /// Get reference to embedder
    pub fn embedder(&self) -> Arc<dyn EmbeddingProvider> {
        self.embedder.clone()
    }

    /// Get reference to database
    pub fn database(&self) -> MemoryBackend {
        self.db.clone()
    }

    /// Run clustering on experiences
    pub async fn run_clustering(&self) -> Result<usize> {
        let clusters = self.clustering_service.cluster_experiences().await?;
        Ok(clusters.len())
    }

    /// Get dreaming service metrics
    pub fn dreaming_metrics(&self) -> Option<(u64, u64, u64, u64)> {
        self.dreaming_service.as_ref().map(|d| d.metrics())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_db() -> (MemoryBackend, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let backend =
            crate::memory::store::lance::LanceMemoryBackend::open_or_create(temp_dir.path())
                .await
                .unwrap();
        (Arc::new(backend), temp_dir)
    }

    #[tokio::test]
    async fn test_cortex_integration_lifecycle() {
        let (db, _temp) = create_test_db().await;
        let config = CortexConfig::default();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(1024, "mock-model"),
        );

        let mut cortex = CortexIntegration::new(config, db, embedder);

        // Start services
        cortex.start().await.unwrap();

        // Check that services are accessible (no panics)
        let _ = cortex.distillation_service().read().await;
        let _ = cortex.pattern_extractor();
        let _ = cortex.clustering_service();

        // Stop services
        cortex.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_cortex_integration_disabled() {
        let (db, _temp) = create_test_db().await;
        let config = CortexConfig {
            enabled: false,
            ..CortexConfig::default()
        };
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
            crate::memory::embedding_provider::tests::MockEmbeddingProvider::new(1024, "mock-model"),
        );

        let mut cortex = CortexIntegration::new(config, db, embedder);

        // Start should succeed but do nothing
        cortex.start().await.unwrap();

        // Dreaming service should not be created
        assert!(cortex.dreaming_service.is_none());

        cortex.stop().await.unwrap();
    }

    #[test]
    fn test_cortex_config_default() {
        let config = CortexConfig::default();
        assert!(config.enabled);
        assert!(config.distillation.enable_realtime);
        assert!(config.dreaming.enable_scheduled);
        assert!(config.clustering.enabled);
    }
}

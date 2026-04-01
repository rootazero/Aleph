//! SkillPrefetch — async skill discovery during model inference.
//!
//! Runs skill scanning in a background task while the LLM is thinking,
//! so newly available skills can be injected into the next prompt without
//! adding latency to the main loop.

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::task::JoinHandle;

// =============================================================================
// Types
// =============================================================================

/// Lightweight skill metadata for prompt injection.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub schema: Option<Value>,
}

/// Source for discovering available skills.
pub trait SkillDiscoverySource: Send + Sync {
    fn discover(&self) -> Pin<Box<dyn Future<Output = Vec<SkillInfo>> + Send + '_>>;
}

/// Manages async skill prefetching during model inference.
pub struct SkillPrefetcher {
    source: Arc<dyn SkillDiscoverySource>,
    last_known: Mutex<Vec<SkillInfo>>,
    scan_interval: Duration,
    last_scan: Mutex<Option<Instant>>,
}

// =============================================================================
// Implementation
// =============================================================================

impl SkillPrefetcher {
    pub fn new(source: Arc<dyn SkillDiscoverySource>, scan_interval: Duration) -> Self {
        Self {
            source,
            last_known: Mutex::new(Vec::new()),
            scan_interval,
            last_scan: Mutex::new(None),
        }
    }

    /// Start async scan if enough time elapsed. Returns JoinHandle or None (throttled).
    pub fn start_scan(&self) -> Option<JoinHandle<Option<Vec<SkillInfo>>>> {
        let now = Instant::now();

        // Check throttle
        {
            let mut last = self.last_scan.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(prev) = *last {
                if now.duration_since(prev) < self.scan_interval {
                    return None;
                }
            }
            *last = Some(now);
        }

        // Clone what we need for the spawned task
        let source = Arc::clone(&self.source);
        let old_skills: Vec<SkillInfo> = self
            .last_known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        Some(tokio::spawn(async move {
            let new_skills = source.discover().await;
            if skills_changed(&old_skills, &new_skills) {
                Some(new_skills)
            } else {
                None
            }
        }))
    }

    /// Update cache after applying discovered skills.
    pub fn commit(&self, skills: Vec<SkillInfo>) {
        *self.last_known.lock().unwrap_or_else(|e| e.into_inner()) = skills;
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Compare by name set (order-independent).
fn skills_changed(old: &[SkillInfo], new: &[SkillInfo]) -> bool {
    let old_names: HashSet<&str> = old.iter().map(|s| s.name.as_str()).collect();
    let new_names: HashSet<&str> = new.iter().map(|s| s.name.as_str()).collect();
    old_names != new_names
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A mock discovery source that returns a configurable list of skills.
    struct MockSource {
        skills: Mutex<Vec<SkillInfo>>,
    }

    impl MockSource {
        fn new(skills: Vec<SkillInfo>) -> Self {
            Self {
                skills: Mutex::new(skills),
            }
        }

        fn set_skills(&self, skills: Vec<SkillInfo>) {
            *self.skills.lock().unwrap() = skills;
        }
    }

    impl SkillDiscoverySource for MockSource {
        fn discover(&self) -> Pin<Box<dyn Future<Output = Vec<SkillInfo>> + Send + '_>> {
            Box::pin(async { self.skills.lock().unwrap().clone() })
        }
    }

    fn make_skill(name: &str) -> SkillInfo {
        SkillInfo {
            name: name.to_string(),
            description: format!("{name} skill"),
            schema: None,
        }
    }

    #[tokio::test]
    async fn scan_returns_skills_on_first_call() {
        let source = Arc::new(MockSource::new(vec![make_skill("alpha")]));
        let prefetcher = SkillPrefetcher::new(source, Duration::from_secs(30));

        let handle = prefetcher.start_scan().expect("first scan should not be throttled");
        let result = handle.await.unwrap();
        assert!(result.is_some(), "first scan should return Some(skills)");
        assert_eq!(result.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn scan_throttled_by_interval() {
        let source = Arc::new(MockSource::new(vec![make_skill("alpha")]));
        let prefetcher = SkillPrefetcher::new(source, Duration::from_secs(30));

        let _handle = prefetcher.start_scan().expect("first scan should fire");
        let second = prefetcher.start_scan();
        assert!(second.is_none(), "second scan within interval should be throttled");
    }

    #[tokio::test]
    async fn scan_returns_none_when_no_changes() {
        let skills = vec![make_skill("alpha"), make_skill("beta")];
        let source = Arc::new(MockSource::new(skills.clone()));
        // Use zero interval so we can scan repeatedly without waiting
        let prefetcher = SkillPrefetcher::new(source, Duration::ZERO);

        // First scan returns skills (cache is empty → different)
        let handle = prefetcher.start_scan().unwrap();
        let result = handle.await.unwrap();
        assert!(result.is_some());

        // Commit the discovered skills
        prefetcher.commit(result.unwrap());

        // Second scan: source returns same skills → None
        let handle = prefetcher.start_scan().unwrap();
        let result = handle.await.unwrap();
        assert!(result.is_none(), "should return None when skills unchanged");
    }

    #[tokio::test]
    async fn scan_detects_new_skills() {
        let source = Arc::new(MockSource::new(vec![make_skill("alpha")]));
        let prefetcher = SkillPrefetcher::new(Arc::clone(&source) as Arc<dyn SkillDiscoverySource>, Duration::ZERO);

        // First scan + commit
        let handle = prefetcher.start_scan().unwrap();
        let result = handle.await.unwrap().unwrap();
        prefetcher.commit(result);

        // Change the source
        source.set_skills(vec![make_skill("alpha"), make_skill("gamma")]);

        // New scan should detect the change
        let handle = prefetcher.start_scan().unwrap();
        let result = handle.await.unwrap();
        assert!(result.is_some(), "should detect new skills");
        assert_eq!(result.unwrap().len(), 2);
    }
}

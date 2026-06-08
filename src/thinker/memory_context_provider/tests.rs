use super::*;
use crate::config::types::memory::MemoryInjectionMode;
use crate::error::AlephError;
use crate::memory::notes::orientation::types::{
    IndexStats, LogEntry, OrientationSnapshot, TokenBudget,
};
use crate::memory::notes::orientation::NoteOrientation;
use crate::memory::notes::profile::types::{ProfileSection, UserProfile};
use crate::memory::notes::profile::ProfileSynthesizer;
use async_trait::async_trait;
use std::collections::BTreeMap;

struct FixedOrient;

#[async_trait]
impl NoteOrientation for FixedOrient {
    async fn bootstrap(&self, _: &str) -> Result<(), AlephError> {
        Ok(())
    }
    async fn read_snapshot(
        &self,
        _: &str,
        _: TokenBudget,
    ) -> Result<OrientationSnapshot, AlephError> {
        Ok(OrientationSnapshot {
            schema_text: "# Memory Schema\n## Domain\nTest".into(),
            index_text: "# Index\n## learning (1)\n- [[learning/rust]] — fact".into(),
            recent_log_tail: "## [2026-04-14] ingest | touched=3".into(),
        })
    }
    async fn record_ingest(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
        Ok(())
    }
    async fn record_query(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
        Ok(())
    }
    async fn record_lint(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
        Ok(())
    }
    async fn record_session_end(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
        Ok(())
    }
    async fn rebuild_index(&self, _: &str) -> Result<IndexStats, AlephError> {
        Ok(IndexStats::default())
    }
    async fn rotate_log_if_needed(&self, _: &str) -> Result<bool, AlephError> {
        Ok(false)
    }
    fn invalidate(&self, _: &str, _: &str) {}
}

struct NoopOrient;

#[async_trait]
impl NoteOrientation for NoopOrient {
    async fn bootstrap(&self, _: &str) -> Result<(), AlephError> {
        Ok(())
    }
    async fn read_snapshot(
        &self,
        _: &str,
        _: TokenBudget,
    ) -> Result<OrientationSnapshot, AlephError> {
        Ok(OrientationSnapshot {
            schema_text: "x".into(),
            index_text: "y".into(),
            recent_log_tail: "z".into(),
        })
    }
    async fn record_ingest(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
        Ok(())
    }
    async fn record_query(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
        Ok(())
    }
    async fn record_lint(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
        Ok(())
    }
    async fn record_session_end(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
        Ok(())
    }
    async fn rebuild_index(&self, _: &str) -> Result<IndexStats, AlephError> {
        Ok(IndexStats::default())
    }
    async fn rotate_log_if_needed(&self, _: &str) -> Result<bool, AlephError> {
        Ok(false)
    }
    fn invalidate(&self, _: &str, _: &str) {}
}

struct FixedProfile;

#[async_trait]
impl ProfileSynthesizer for FixedProfile {
    async fn bootstrap(&self, _agent_id: &str) -> Result<UserProfile, AlephError> {
        unimplemented!()
    }

    async fn current(&self, _agent_id: &str) -> Result<Option<UserProfile>, AlephError> {
        let mut sections = BTreeMap::new();
        sections.insert(
            ProfileSection::Identity.heading().to_string(),
            vec!["Software engineer".to_string()],
        );
        Ok(Some(UserProfile {
            schema_version: 1,
            updated: "2026-04-17".to_string(),
            revision: 3,
            last_session: "ses_001".to_string(),
            confidence: "medium".to_string(),
            raw: "# User Profile\n## Identity\n- Software engineer\n".to_string(),
            sections,
            content_hash: "abc".to_string(),
        }))
    }

    async fn update(
        &self,
        _agent_id: &str,
        _signal: crate::memory::notes::profile::types::SessionSignal,
    ) -> Result<crate::memory::notes::profile::types::UpdateOutcome, AlephError> {
        unimplemented!()
    }
}

#[tokio::test]
async fn tools_mode_returns_none_regardless_of_envelope() {
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools);
    let msg = provider
        .build_memory_user_message("agent-1", "any query", None)
        .await
        .unwrap();
    assert!(msg.is_none(), "Tools mode must not auto-inject");
}

#[tokio::test]
async fn context_mode_with_empty_envelope_returns_none() {
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context);
    let msg = provider
        .build_memory_user_message("agent-1", "any query", None)
        .await
        .unwrap();
    assert!(
        msg.is_none(),
        "empty envelope must short-circuit to None in Context mode"
    );
}

#[tokio::test]
async fn hybrid_mode_with_empty_envelope_returns_none() {
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Hybrid);
    let msg = provider
        .build_memory_user_message("agent-1", "any query", None)
        .await
        .unwrap();
    assert!(msg.is_none());
}

#[tokio::test]
async fn build_memory_user_message_invokes_on_retrieve_extension() {
    use crate::memory::extensions::traits::MemoryExtension;
    use crate::memory::extensions::{MemoryExtensionRegistry, RetrieveCtx};
    use crate::sync_primitives::{Arc, Mutex};
    use async_trait::async_trait;

    struct Recorder(Mutex<u32>);
    #[async_trait]
    impl MemoryExtension for Recorder {
        fn name(&self) -> &str {
            "test.recorder"
        }
        async fn on_retrieve(
            &self,
            _ctx: &RetrieveCtx,
            _env: &mut crate::memory::assembler::envelope::MemoryEnvelope,
        ) -> Result<(), AlephError> {
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            Ok(())
        }
    }

    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Hybrid);
    let rec = Arc::new(Recorder(Mutex::new(0)));
    let reg = MemoryExtensionRegistry::new();
    reg.register(rec.clone());
    let provider = provider.with_extensions(Arc::new(reg));

    // Empty envelope → still invokes on_retrieve before the emptiness check.
    let _ = provider
        .build_memory_user_message("a1", "q", None)
        .await
        .unwrap();
    assert_eq!(
        *rec.0.lock().unwrap_or_else(|e| e.into_inner()),
        1,
        "on_retrieve must be dispatched"
    );
}

#[tokio::test]
async fn tools_mode_skips_on_retrieve_dispatch() {
    use crate::memory::extensions::traits::MemoryExtension;
    use crate::memory::extensions::{MemoryExtensionRegistry, RetrieveCtx};
    use crate::sync_primitives::{Arc, Mutex};
    use async_trait::async_trait;

    struct Recorder(Mutex<u32>);
    #[async_trait]
    impl MemoryExtension for Recorder {
        fn name(&self) -> &str {
            "test.recorder"
        }
        async fn on_retrieve(
            &self,
            _ctx: &RetrieveCtx,
            _env: &mut crate::memory::assembler::envelope::MemoryEnvelope,
        ) -> Result<(), AlephError> {
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            Ok(())
        }
    }

    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools);
    let rec = Arc::new(Recorder(Mutex::new(0)));
    let reg = MemoryExtensionRegistry::new();
    reg.register(rec.clone());
    let provider = provider.with_extensions(Arc::new(reg));

    let out = provider
        .build_memory_user_message("a1", "q", None)
        .await
        .unwrap();
    assert!(out.is_none());
    assert_eq!(
        *rec.0.lock().unwrap_or_else(|e| e.into_inner()),
        0,
        "Tools mode must not call on_retrieve"
    );
}

#[tokio::test]
async fn orientation_message_injected_in_context_mode() {
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
        .with_orientation(Arc::new(FixedOrient));

    let msg = provider
        .build_orientation_user_message("default", MemoryInjectionMode::Context)
        .await
        .unwrap();
    let m = msg.expect("context mode should inject");
    let text = format!("{m:?}");
    assert!(text.contains("NoteOrientation"));
    assert!(text.contains("# Memory Schema"));
    assert!(text.contains("# Index"));
    assert!(text.contains("touched=3"));
}

#[tokio::test]
async fn orientation_skipped_in_tools_mode() {
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools)
        .with_orientation(Arc::new(NoopOrient));

    let msg = provider
        .build_orientation_user_message("default", MemoryInjectionMode::Tools)
        .await
        .unwrap();
    assert!(msg.is_none());
}

#[tokio::test]
async fn profile_message_injected_in_context_mode() {
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
        .with_profile(Arc::new(FixedProfile));

    let msg = provider
        .build_profile_user_message("default", MemoryInjectionMode::Context)
        .await
        .unwrap();
    let m = msg.expect("context mode should inject profile");
    let text = format!("{m:?}");
    assert!(text.contains("<UserProfile>"));
    assert!(text.contains("</UserProfile>"));
}

#[tokio::test]
async fn profile_skipped_in_tools_mode() {
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools)
        .with_profile(Arc::new(FixedProfile));

    let msg = provider
        .build_profile_user_message("default", MemoryInjectionMode::Tools)
        .await
        .unwrap();
    assert!(msg.is_none());
}

#[tokio::test]
async fn first_call_captures_snapshot_subsequent_calls_hit_cache() {
    use crate::memory::curated::CuratedConfig;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
        .with_curated_config(CuratedConfig {
            memory_char_limit: 100,
            user_char_limit: 100,
            legacy_warn_threshold: 0.95,
        })
        .with_curated_root_for_test(dir.path().to_path_buf());

    // Pre-seed MEMORY.md (with trailing § so it's classified as curated,
    // not legacy — see Task 7's serialize-with-trailing-delimiter fix).
    let agent_dir = dir.path().join("agent-x");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("MEMORY.md"), "fact one\n§\nfact two\n§\n").unwrap();

    let m1 = provider
        .build_curated_message("agent-x", "ses-1")
        .await
        .unwrap();
    assert!(m1.is_some(), "first call must produce a message");
    let txt1 = format!("{:?}", m1);
    assert!(
        txt1.contains("fact one"),
        "rendered block must include entries"
    );
    assert!(
        txt1.contains("CuratedMemory"),
        "must be wrapped in <CuratedMemory>"
    );

    // Mutate disk; same session_key must NOT reflect the change.
    std::fs::write(
        agent_dir.join("MEMORY.md"),
        "fact one\n§\nfact two\n§\nfact three\n§\n",
    )
    .unwrap();
    let m2 = provider
        .build_curated_message("agent-x", "ses-1")
        .await
        .unwrap();
    let txt2 = format!("{:?}", m2);
    assert!(
        !txt2.contains("fact three"),
        "snapshot must be frozen for ses-1 — second call should hit cache"
    );

    // Eviction → next call rebuilds and picks up the disk change. Note
    // that the per-agent store is also cached, so we must evict it via
    // the public surface — but `invalidate_curated` only evicts the
    // snapshot keyed by session. The store stays cached (its
    // `current_entries()` is read fresh each capture, but it was
    // initialized from the OLD body). For the test to observe the
    // disk-side mutation, we also clear the per-agent store map.
    provider.invalidate_curated("ses-1").await;
    provider.curated_stores.remove("agent-x");

    let m3 = provider
        .build_curated_message("agent-x", "ses-1")
        .await
        .unwrap();
    let txt3 = format!("{:?}", m3);
    assert!(
        txt3.contains("fact three"),
        "after invalidate + store reset, must pick up disk change"
    );
}

#[tokio::test]
async fn invalidate_curated_for_agent_drops_all_sessions_only_for_target() {
    use crate::memory::curated::CuratedConfig;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let provider = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
        .with_curated_config(CuratedConfig {
            memory_char_limit: 200,
            user_char_limit: 200,
            legacy_warn_threshold: 0.95,
        })
        .with_curated_root_for_test(dir.path().to_path_buf());

    // Pre-seed two agents, each with curated entries.
    let a_dir = dir.path().join("agent-A");
    let b_dir = dir.path().join("agent-B");
    std::fs::create_dir_all(&a_dir).unwrap();
    std::fs::create_dir_all(&b_dir).unwrap();
    std::fs::write(a_dir.join("MEMORY.md"), "a-old\n§\n").unwrap();
    std::fs::write(b_dir.join("MEMORY.md"), "b-old\n§\n").unwrap();

    // Prime caches: 2 sessions for agent-A, 1 for agent-B.
    provider
        .build_curated_message("agent-A", "ses-1")
        .await
        .unwrap();
    provider
        .build_curated_message("agent-A", "ses-2")
        .await
        .unwrap();
    provider
        .build_curated_message("agent-B", "ses-1")
        .await
        .unwrap();

    assert_eq!(provider.curated_snapshots.read().await.len(), 3);

    // Invalidate ALL agent-A sessions; agent-B must remain.
    provider.invalidate_curated_for_agent("agent-A").await;

    let snaps = provider.curated_snapshots.read().await;
    assert_eq!(snaps.len(), 1, "only agent-B's snapshot should survive");
    assert!(
        snaps.contains_key(&("agent-B".to_string(), "ses-1".to_string())),
        "agent-B/ses-1 must still be cached"
    );
    assert!(
        !snaps.contains_key(&("agent-A".to_string(), "ses-1".to_string())),
        "agent-A/ses-1 must be evicted"
    );
    assert!(
        !snaps.contains_key(&("agent-A".to_string(), "ses-2".to_string())),
        "agent-A/ses-2 must be evicted"
    );
}

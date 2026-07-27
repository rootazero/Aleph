//! `ProfileSynthesizer` trait and filesystem-backed implementation.
//!
//! The synthesizer is responsible for bootstrapping and incrementally updating
//! `USER.md` via LLM calls.  It owns the merge algorithm: rate-limiting,
//! hash-guarded writes, diff computation, and optional orientation logging.

use crate::sync_primitives::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tracing::{debug, warn};

use crate::config::types::memory::UserProfileConfig;
use crate::error::AlephError;
use crate::memory::notes::orientation::types::{LogAction, LogEntry};
use crate::memory::notes::orientation::NoteOrientation;
use crate::memory::notes::profile::prompts::{
    build_merge_user_prompt, PROMPT_PROFILE_BOOTSTRAP, PROMPT_PROFILE_MERGE,
};
use crate::memory::notes::profile::store::{render_user_md, ProfileStore};
use crate::memory::notes::profile::types::{
    ProfileDiff, ProfileSection, SessionSignal, UpdateOutcome, UserProfile,
};
use crate::providers::adapter::RequestPayload;
use crate::providers::message::{ContentBlock, UnifiedMessage};
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;
use crate::utils::json_extract::extract_json_robust;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction for user-profile lifecycle: bootstrap, read, merge-update.
#[async_trait]
pub trait ProfileSynthesizer: Send + Sync {
    /// Create an initial USER.md if none exists, or return the existing one.
    async fn bootstrap(&self, agent_id: &str) -> Result<UserProfile, AlephError>;

    /// Read the current profile (returns `None` if USER.md is absent).
    async fn current(&self, agent_id: &str) -> Result<Option<UserProfile>, AlephError>;

    /// Merge a session signal into the existing profile.
    async fn update(
        &self,
        agent_id: &str,
        signal: SessionSignal,
    ) -> Result<UpdateOutcome, AlephError>;
}

// ---------------------------------------------------------------------------
// FsProfileSynthesizer
// ---------------------------------------------------------------------------

/// Filesystem-backed profile synthesizer.
pub struct FsProfileSynthesizer {
    memory_dir: PathBuf,
    provider: Arc<dyn AiProvider>,
    orientation: Option<Arc<dyn NoteOrientation>>,
    last_update: Mutex<HashMap<String, Instant>>,
    min_interval: Duration,
    max_bullets_per_section: usize,
    bootstrap_on_first_session_end: bool,
}

impl FsProfileSynthesizer {
    /// Create with sensible defaults (30 min interval, 20 max bullets).
    pub fn new(memory_dir: impl Into<PathBuf>, provider: Arc<dyn AiProvider>) -> Self {
        Self {
            memory_dir: memory_dir.into(),
            provider,
            orientation: None,
            last_update: Mutex::new(HashMap::new()),
            min_interval: Duration::from_secs(30 * 60),
            max_bullets_per_section: 20,
            bootstrap_on_first_session_end: true,
        }
    }

    /// Attach an orientation logger.
    pub fn with_orientation(mut self, o: Arc<dyn NoteOrientation>) -> Self {
        self.orientation = Some(o);
        self
    }

    /// Override defaults from config.
    pub fn with_config(mut self, cfg: &UserProfileConfig) -> Self {
        self.min_interval = Duration::from_secs(u64::from(cfg.profile_min_interval_minutes) * 60);
        self.max_bullets_per_section = cfg.max_bullets_per_section;
        self.bootstrap_on_first_session_end = cfg.bootstrap_on_first_session_end;
        self
    }

    // -- helpers --

    fn store(&self, agent_id: &str) -> ProfileStore {
        ProfileStore::new(self.memory_dir.join(agent_id))
    }

    /// Call the LLM with a system prompt and a single user message.
    async fn llm_call(&self, system: &str, user_msg: &str) -> Result<String, AlephError> {
        let user = UnifiedMessage::User {
            content: vec![ContentBlock::Text {
                text: user_msg.to_string(),
                cache_control: None,
            }],
        };
        let messages = [user];
        let req = RequestPayload {
            messages: &messages,
            system_prompt: Some(system),
            system_blocks: None,
            ..Default::default()
        };
        let resp = self.provider.process(req).await?;
        resp.text
            .ok_or_else(|| AlephError::other("LLM returned no text for profile synthesis"))
    }

    /// Parse the six-section JSON returned by the LLM.  Returns sections map
    /// and optional extra fields (confidence, `stance_shift`, outcome).
    fn parse_sections_json(
        &self,
        raw: &str,
    ) -> Result<(BTreeMap<String, Vec<String>>, serde_json::Value), AlephError> {
        let val = extract_json_robust(raw)
            .ok_or_else(|| AlephError::other("LLM response is not valid JSON"))?;

        // Merge responses nest sections under "sections"; bootstrap puts them
        // at the top level.
        let section_root = val.get("sections").unwrap_or(&val);

        let mut sections = BTreeMap::new();
        for s in ProfileSection::ALL {
            let heading = s.heading();
            let arr = section_root
                .get(heading)
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            sections.insert(heading.to_string(), arr);
        }

        Ok((sections, val))
    }

    /// Validate that all 6 sections are present and each has at most
    /// `max_bullets` bullets.  Returns `Err` on violation.
    fn validate_sections(
        &self,
        sections: &BTreeMap<String, Vec<String>>,
        max_bullets: usize,
    ) -> Result<(), AlephError> {
        for s in ProfileSection::ALL {
            let heading = s.heading();
            let bullets = sections
                .get(heading)
                .ok_or_else(|| AlephError::other(format!("missing section: {heading}")))?;
            if bullets.len() > max_bullets {
                return Err(AlephError::other(format!(
                    "section '{heading}' has {} bullets (max {max_bullets})",
                    bullets.len()
                )));
            }
        }
        Ok(())
    }

    /// Compute a diff between old and new section maps.
    fn compute_diff(
        old: &BTreeMap<String, Vec<String>>,
        new: &BTreeMap<String, Vec<String>>,
        stance_shift: Option<String>,
    ) -> ProfileDiff {
        let mut diff = ProfileDiff {
            stance_shift_note: stance_shift,
            ..Default::default()
        };

        for s in ProfileSection::ALL {
            let heading = s.heading().to_string();
            let old_bullets: Vec<&str> = old
                .get(&heading)
                .map(|v| v.iter().map(|s| s.as_str()).collect())
                .unwrap_or_default();
            let new_bullets: Vec<&str> = new
                .get(&heading)
                .map(|v| v.iter().map(|s| s.as_str()).collect())
                .unwrap_or_default();

            let mut modified = false;
            for b in &new_bullets {
                if !old_bullets.contains(b) {
                    // rust-doctor-disable-next-line excessive-clone
                    diff.added_bullets.push((heading.clone(), (*b).to_string()));
                    modified = true;
                }
            }
            for b in &old_bullets {
                if !new_bullets.contains(b) {
                    diff.removed_bullets
                        // rust-doctor-disable-next-line excessive-clone
                        .push((heading.clone(), (*b).to_string()));
                    modified = true;
                }
            }
            if modified {
                diff.sections_modified.push(heading);
            }
        }

        diff
    }

    /// Log an action to orientation if available.
    async fn log_orientation(&self, agent_id: &str, action: LogAction, summary: &str) {
        if let Some(ref o) = self.orientation {
            let entry = LogEntry {
                timestamp_utc: chrono::Utc::now().timestamp(),
                action,
                summary: summary.to_string(),
                detail_lines: vec![],
            };
            if let Err(e) = o.record_session_end(agent_id, entry).await {
                warn!("orientation log failed: {e}");
            }
        }
    }
}

#[async_trait]
impl ProfileSynthesizer for FsProfileSynthesizer {
    async fn bootstrap(&self, agent_id: &str) -> Result<UserProfile, AlephError> {
        let store = self.store(agent_id);

        // If USER.md already exists, return it.
        if let Some(existing) = store.read().await? {
            debug!(agent_id, "bootstrap: USER.md already exists, returning");
            return Ok(existing);
        }

        // Call LLM
        let raw = self
            .llm_call(PROMPT_PROFILE_BOOTSTRAP, "Bootstrap the user profile.")
            .await?;

        let (sections, val) = self.parse_sections_json(&raw)?;

        // Bootstrap validation: max 5 bullets per section
        self.validate_sections(&sections, 5)?;

        let confidence = val
            .get("confidence")
            .and_then(|v| v.as_str())
            .unwrap_or("low")
            .to_string();

        let md = render_user_md(
            1,
            "bootstrap",
            &confidence,
            &sections,
            &std::collections::BTreeMap::new(),
        );
        store.write(&md, None).await?;

        self.log_orientation(agent_id, LogAction::Bootstrap, "bootstrapped USER.md")
            .await;

        // Re-read so content_hash is correct
        store
            .read()
            .await?
            .ok_or_else(|| AlephError::other("USER.md missing after write"))
    }

    async fn current(&self, agent_id: &str) -> Result<Option<UserProfile>, AlephError> {
        self.store(agent_id).read().await
    }

    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn update(
        &self,
        agent_id: &str,
        signal: SessionSignal,
    ) -> Result<UpdateOutcome, AlephError> {
        // Rate-limit check
        {
            let guard = self.last_update.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(last) = guard.get(agent_id) {
                if last.elapsed() < self.min_interval {
                    debug!(agent_id, "update rate-limited");
                    return Ok(UpdateOutcome::Unchanged);
                }
            }
        }

        let store = self.store(agent_id);

        // Read current profile
        let profile = match store.read().await? {
            Some(p) => p,
            None => {
                if self.bootstrap_on_first_session_end {
                    let p = self.bootstrap(agent_id).await?;
                    return Ok(UpdateOutcome::Bootstrapped {
                        revision: p.revision,
                    });
                }
                return Ok(UpdateOutcome::Unchanged);
            }
        };

        // rust-doctor-disable-next-line excessive-clone
        let h0 = profile.content_hash.clone();
        let old_revision = profile.revision;
        // rust-doctor-disable-next-line excessive-clone
        let old_sections = profile.sections.clone();

        // Build LLM prompt
        let user_prompt = build_merge_user_prompt(&profile, &signal);
        let raw = self.llm_call(PROMPT_PROFILE_MERGE, &user_prompt).await?;

        let (sections, val) = self.parse_sections_json(&raw)?;

        // Check outcome
        let outcome_str = val
            .get("outcome")
            .and_then(|v| v.as_str())
            .unwrap_or("unchanged");

        if outcome_str == "unchanged" {
            return Ok(UpdateOutcome::Unchanged);
        }

        // Validate sections
        self.validate_sections(&sections, self.max_bullets_per_section)?;

        // Parse confidence (default to "medium" if invalid)
        let confidence = val
            .get("confidence")
            .and_then(|v| v.as_str())
            .filter(|c| matches!(*c, "low" | "medium" | "high"))
            .unwrap_or("medium")
            .to_string();

        // Stance shift
        let stance_shift = val
            .get("stance_shift")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Compute diff
        let diff = Self::compute_diff(&old_sections, &sections, stance_shift);

        let new_revision = old_revision + 1;
        // rust-doctor-disable-next-line excessive-clone
        let mut sources = profile.sources.clone();
        for section in &diff.sections_modified {
            // rust-doctor-disable-next-line excessive-clone
            let entry = sources.entry(section.clone()).or_default();
            if !entry.contains(&signal.session_id) {
                // rust-doctor-disable-next-line excessive-clone
                entry.push(signal.session_id.clone());
            }
        }
        let md = render_user_md(
            new_revision,
            &signal.session_id,
            &confidence,
            &sections,
            &sources,
        );

        // Hash-guarded write with one retry on conflict
        match store.write(&md, Some(&h0)).await {
            Ok(_) => {}
            Err(_) => {
                // Concurrent update detected — re-read and attempt single re-plan
                let current = store.read().await?;
                // rust-doctor-disable-next-line excessive-clone
                let current_hash = current.as_ref().map(|p| p.content_hash.clone());
                if let Some(ref ch) = current_hash {
                    // Re-try write with the fresh hash
                    if store.write(&md, Some(ch)).await.is_err() {
                        warn!(agent_id, "profile update aborted after conflict retry");
                        return Ok(UpdateOutcome::Unchanged);
                    }
                } else {
                    return Ok(UpdateOutcome::Unchanged);
                }
            }
        }

        // Update rate-limit timestamp
        {
            let mut guard = self.last_update.lock().unwrap_or_else(|e| e.into_inner());
            guard.insert(agent_id.to_string(), Instant::now());
        }

        self.log_orientation(
            agent_id,
            LogAction::Profile,
            &format!("profile updated to revision {new_revision}"),
        )
        .await;

        Ok(UpdateOutcome::Revised {
            revision: new_revision,
            diff,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::recording_mock::RecordingMockProvider;

    /// Valid bootstrap JSON that the mock LLM returns.
    fn bootstrap_json() -> String {
        serde_json::json!({
            "Identity": ["Software engineer", "Based in Berlin"],
            "Communication Style": ["Direct and concise"],
            "Motivations": ["Building reliable systems"],
            "Current Focus": ["Rust async patterns"],
            "Stance Shifts": [],
            "Open Questions": ["Best approach for distributed sync"],
            "confidence": "low"
        })
        .to_string()
    }

    /// Valid merge JSON that revises the profile.
    fn merge_revised_json() -> String {
        serde_json::json!({
            "outcome": "revised",
            "sections": {
                "Identity": ["Software engineer", "Based in Berlin", "Early 30s"],
                "Communication Style": ["Direct and concise"],
                "Motivations": ["Building reliable systems"],
                "Current Focus": ["Memory evolution design"],
                "Stance Shifts": ["2026-04-17: Shifted to event-sourced memory"],
                "Open Questions": ["Best approach for distributed sync"]
            },
            "stance_shift": "Shifted to event-sourced memory",
            "confidence": "medium"
        })
        .to_string()
    }

    /// Merge JSON where outcome is unchanged.
    fn merge_unchanged_json() -> String {
        serde_json::json!({
            "outcome": "unchanged",
            "sections": {
                "Identity": ["Software engineer", "Based in Berlin"],
                "Communication Style": ["Direct and concise"],
                "Motivations": ["Building reliable systems"],
                "Current Focus": ["Rust async patterns"],
                "Stance Shifts": [],
                "Open Questions": ["Best approach for distributed sync"]
            },
            "confidence": "medium"
        })
        .to_string()
    }

    fn test_signal() -> SessionSignal {
        SessionSignal {
            reason: "session_end".to_string(),
            digest_text: "Discussed memory evolution design.".to_string(),
            recent_user_turns: vec!["How should we structure the memory layer?".to_string()],
            session_id: "ses_001".to_string(),
        }
    }

    fn make_synth(canned: &str) -> (FsProfileSynthesizer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(RecordingMockProvider::new(canned.to_string()));
        let synth = FsProfileSynthesizer::new(dir.path(), provider);
        (synth, dir)
    }

    #[tokio::test]
    async fn bootstrap_creates_user_md() {
        let (synth, _dir) = make_synth(&bootstrap_json());
        let profile = synth.bootstrap("agent_a").await.unwrap();

        assert_eq!(profile.revision, 1);
        assert_eq!(profile.confidence, "low");
        // Sections with bullets are present; empty ones (e.g. Stance Shifts)
        // are omitted by parse_user_md since there are no bullet lines.
        assert!(profile.sections.len() >= 5);
        assert!(profile.sections.get("Identity").unwrap().len() <= 5);

        // Verify it persists on disk
        let current = synth.current("agent_a").await.unwrap();
        assert!(current.is_some());
        assert_eq!(current.unwrap().revision, 1);
    }

    #[tokio::test]
    async fn update_revises_profile() {
        // Bootstrap first
        let (synth, _dir) = make_synth(&bootstrap_json());
        synth.bootstrap("agent_b").await.unwrap();

        // Now create a new synth with merge response (different canned)
        let provider = Arc::new(RecordingMockProvider::new(merge_revised_json()));
        let synth2 = FsProfileSynthesizer {
            memory_dir: synth.memory_dir.clone(),
            provider,
            orientation: None,
            last_update: Mutex::new(HashMap::new()),
            min_interval: Duration::from_secs(0),
            max_bullets_per_section: 20,
            bootstrap_on_first_session_end: true,
        };

        let outcome = synth2.update("agent_b", test_signal()).await.unwrap();
        match outcome {
            UpdateOutcome::Revised { revision, diff } => {
                assert_eq!(revision, 2);
                assert!(!diff.added_bullets.is_empty());
                assert!(diff.stance_shift_note.is_some());
            }
            other => panic!("expected Revised, got {other:?}"),
        }

        // Verify on disk
        let profile = synth2.current("agent_b").await.unwrap().unwrap();
        assert_eq!(profile.revision, 2);
        assert!(profile
            .sections
            .get("Identity")
            .unwrap()
            .contains(&"Early 30s".to_string()));
    }

    #[tokio::test]
    async fn update_unchanged_when_llm_says_so() {
        let (synth, _dir) = make_synth(&bootstrap_json());
        synth.bootstrap("agent_c").await.unwrap();

        let provider = Arc::new(RecordingMockProvider::new(merge_unchanged_json()));
        let synth2 = FsProfileSynthesizer {
            memory_dir: synth.memory_dir.clone(),
            provider,
            orientation: None,
            last_update: Mutex::new(HashMap::new()),
            min_interval: Duration::from_secs(0),
            max_bullets_per_section: 20,
            bootstrap_on_first_session_end: true,
        };

        let outcome = synth2.update("agent_c", test_signal()).await.unwrap();
        assert!(matches!(outcome, UpdateOutcome::Unchanged));
    }

    #[tokio::test]
    async fn update_rate_limited() {
        let (synth, _dir) = make_synth(&bootstrap_json());
        synth.bootstrap("agent_d").await.unwrap();

        // Create synth with 1-hour min_interval and set last_update to now
        let provider = Arc::new(RecordingMockProvider::new(merge_revised_json()));
        let synth2 = FsProfileSynthesizer {
            memory_dir: synth.memory_dir.clone(),
            provider,
            orientation: None,
            last_update: Mutex::new(HashMap::from([("agent_d".to_string(), Instant::now())])),
            min_interval: Duration::from_secs(3600),
            max_bullets_per_section: 20,
            bootstrap_on_first_session_end: true,
        };

        // Should return Unchanged without calling LLM
        let outcome = synth2.update("agent_d", test_signal()).await.unwrap();
        assert!(matches!(outcome, UpdateOutcome::Unchanged));
    }

    #[tokio::test]
    async fn rate_limit_is_per_agent() {
        // agent_e was just updated; a long interval keeps it rate-limited.
        // agent_f has never been seen and must NOT be starved by agent_e.
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(RecordingMockProvider::new(bootstrap_json()));
        let synth = FsProfileSynthesizer {
            memory_dir: dir.path().to_path_buf(),
            provider,
            orientation: None,
            last_update: Mutex::new(HashMap::from([("agent_e".to_string(), Instant::now())])),
            min_interval: Duration::from_secs(3600),
            max_bullets_per_section: 20,
            bootstrap_on_first_session_end: true,
        };

        // agent_e is inside its own window -> rate-limited.
        let e = synth.update("agent_e", test_signal()).await.unwrap();
        assert!(matches!(e, UpdateOutcome::Unchanged));

        // agent_f has no prior entry -> allowed to proceed (bootstraps here).
        let f = synth.update("agent_f", test_signal()).await.unwrap();
        assert!(matches!(f, UpdateOutcome::Bootstrapped { .. }));
    }
}

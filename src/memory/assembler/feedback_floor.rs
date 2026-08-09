//! `FeedbackFloorLoader` — always-on surfacing of High/Critical feedback rules.
//!
//! Query-relevance retrieval can miss a standing correction when the current
//! turn's text doesn't lexically match it — which is exactly when the user is
//! most likely to repeat the mistake the rule guards against. This loader scans
//! the agent's `feedback/` notes and unconditionally surfaces the High/Critical
//! ones, mirroring how [`UserProfileLoader`](super::profile::UserProfileLoader)
//! always injects the profile. Low/Med feedback stays purely relevance-gated.

use crate::memory::notes::{KnowledgeNote, Severity};
use crate::sync_primitives::Arc;
use std::path::PathBuf;

/// Max feedback files to read per assembly — bounds per-turn disk I/O so the
/// floor cost stays comparable to the single-file profile load even if the
/// feedback corpus grows large.
const SCAN_CAP: usize = 64;
/// Max High/Critical rules promoted into the always-on floor.
const FLOOR_CAP: usize = 6;

/// One always-on feedback rule, parsed from a `feedback/*.md` note.
#[derive(Debug, Clone)]
pub struct FeedbackFloorEntry {
    /// Scheme-less note path, e.g. `"feedback/no-force-push"`.
    pub path: String,
    pub title: String,
    /// Note body with frontmatter stripped (the rule text).
    pub body: String,
    pub severity: Severity,
    pub updated_at: i64,
}

/// Loader for `memory/note/{agent_id}/feedback/*.md`. Behind an `Arc` so it can
/// be shared across the assembler and its tests.
pub struct FeedbackFloorLoader {
    memory_dir: PathBuf,
}

impl FeedbackFloorLoader {
    #[must_use]
    pub fn new(memory_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { memory_dir })
    }

    /// Scan `{agent_id}/feedback/*.md` and return the High/Critical rules,
    /// Critical-first then most-recently-updated, capped at [`FLOOR_CAP`].
    ///
    /// Never errors: a missing directory or an unreadable/unparseable file
    /// simply yields fewer entries.
    pub async fn load(&self, agent_id: &str) -> Vec<FeedbackFloorEntry> {
        self.load_many(std::slice::from_ref(&agent_id.to_string()))
            .await
    }

    /// The floor across every partition this session may read.
    ///
    /// The writer and the reader used to disagree about where feedback lives.
    /// `flag_user_correction` writes through `caller_memory_partition`, and a
    /// zero-config loopback Panel session resolves to `Personal(u-owner)` — so
    /// the factory-default write lands in `main__u-owner/feedback/`, while this
    /// loader was handed the bare persona (`main`) and scanned a directory that
    /// never receives anything. The always-on floor was therefore empty in the
    /// out-of-the-box install, and silently so: those rules still appeared when
    /// retrieval happened to match them lexically, so the only thing missing
    /// was the case the floor exists for — the turn whose text does *not*
    /// mention the mistake the user is about to repeat.
    ///
    /// Callers pass `project_scope::session_read_ids`, the same derivation
    /// `Gatherer::fetch_notes` uses, so org-wide base rules keep reaching
    /// everyone (base is always in that set) and a session additionally sees
    /// its own.
    ///
    /// Ranking and [`FLOOR_CAP`] are applied **once, after merging**: capping
    /// per partition would hand a session with two partitions twice the floor,
    /// and would let a partition with three Critical rules crowd out another's.
    pub async fn load_many(&self, agent_ids: &[String]) -> Vec<FeedbackFloorEntry> {
        let mut out: Vec<FeedbackFloorEntry> = Vec::new();
        for agent_id in agent_ids {
            out.extend(self.load_partition(agent_id).await);
        }
        // Critical before High (Severity derives Ord in ascending declaration
        // order), then freshest first.
        out.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        // Two partitions can hold the same relative path (`feedback/no-force-push`
        // written once org-wide and once personally); the newer one already sorts
        // first, so keeping the first occurrence keeps the fresher rule.
        let mut seen = std::collections::HashSet::new();
        out.retain(|e| seen.insert(e.path.clone()));
        out.truncate(FLOOR_CAP);
        out
    }

    /// Scan one partition's `feedback/` directory. `SCAN_CAP` bounds the I/O.
    async fn load_partition(&self, agent_id: &str) -> Vec<FeedbackFloorEntry> {
        let dir = self.memory_dir.join(agent_id).join("feedback");
        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };

        // Collect candidates first so `SCAN_CAP` can drop the *oldest* files
        // rather than whatever the filesystem happened to enumerate last.
        // Truncating in readdir order meant that past 64 notes, which rules
        // made the always-on floor was decided by directory iteration order —
        // a Critical rule written yesterday could be invisible while a High
        // one from last year was injected into every request.
        let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let file = entry.path();
            if file.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let mtime = entry
                .metadata()
                .await
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            candidates.push((mtime, file));
        }
        candidates.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
        candidates.truncate(SCAN_CAP);

        let mut out: Vec<FeedbackFloorEntry> = Vec::new();
        for (_, file) in candidates {
            let Ok(content) = tokio::fs::read_to_string(&file).await else {
                continue;
            };
            let stem = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let Ok(note) = KnowledgeNote::from_markdown(&stem, &content) else {
                continue;
            };
            // Only standing-directive severities are always-on; Low/Med remain
            // relevance-gated via ordinary retrieval.
            if note.severity < Severity::High {
                continue;
            }
            out.push(FeedbackFloorEntry {
                path: format!("feedback/{stem}"),
                title: note.title.clone(),
                body: strip_frontmatter(&content),
                severity: note.severity,
                updated_at: note.updated_at,
            });
        }
        out
    }
}

/// Strip a leading `---\n ... \n---\n` YAML frontmatter block, returning the
/// body. Mirrors `profile::strip_frontmatter` (kept local — two trivial
/// call sites do not warrant a shared util per the rule of three).
fn strip_frontmatter(s: &str) -> String {
    let trimmed = s.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            return rest[end + 5..].trim_start().to_string();
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn write_feedback(dir: &std::path::Path, name: &str, severity: &str, rule: &str) {
        let feedback_dir = dir.join("default").join("feedback");
        tokio::fs::create_dir_all(&feedback_dir).await.unwrap();
        let content = format!(
            "---\ncategory: feedback\nseverity: {severity}\nconfidence: 0.9\n---\n\n- {rule}\n"
        );
        tokio::fs::write(feedback_dir.join(format!("{name}.md")), content)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn missing_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let loader = FeedbackFloorLoader::new(tmp.path().to_path_buf());
        assert!(loader.load("default").await.is_empty());
    }

    #[tokio::test]
    async fn surfaces_high_and_critical_only() {
        let tmp = tempfile::tempdir().unwrap();
        write_feedback(tmp.path(), "low-rule", "low", "minor preference").await;
        write_feedback(tmp.path(), "med-rule", "med", "moderate rule").await;
        write_feedback(tmp.path(), "high-rule", "high", "always run fmt").await;
        write_feedback(tmp.path(), "crit-rule", "critical", "never force-push main").await;

        let loader = FeedbackFloorLoader::new(tmp.path().to_path_buf());
        let entries = loader.load("default").await;

        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths.len(), 2, "only high + critical are always-on");
        // Critical sorts before High.
        assert_eq!(entries[0].path, "feedback/crit-rule");
        assert_eq!(entries[0].severity, Severity::Critical);
        assert_eq!(entries[1].path, "feedback/high-rule");
        assert!(entries[0].body.contains("never force-push main"));
        assert!(!paths.contains(&"feedback/low-rule"));
        assert!(!paths.contains(&"feedback/med-rule"));
    }

    #[tokio::test]
    async fn caps_floor_size() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..(FLOOR_CAP + 4) {
            write_feedback(tmp.path(), &format!("rule-{i}"), "high", "a rule").await;
        }
        let loader = FeedbackFloorLoader::new(tmp.path().to_path_buf());
        let entries = loader.load("default").await;
        assert_eq!(entries.len(), FLOOR_CAP);
    }

    #[test]
    fn strip_frontmatter_handles_missing_block() {
        assert_eq!(strip_frontmatter("no frontmatter"), "no frontmatter");
    }
}

//! FeedbackFloorLoader — always-on surfacing of High/Critical feedback rules.
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
    pub fn new(memory_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { memory_dir })
    }

    /// Scan `{agent_id}/feedback/*.md` and return the High/Critical rules,
    /// Critical-first then most-recently-updated, capped at [`FLOOR_CAP`].
    ///
    /// Never errors: a missing directory or an unreadable/unparseable file
    /// simply yields fewer entries.
    pub async fn load(&self, agent_id: &str) -> Vec<FeedbackFloorEntry> {
        let dir = self.memory_dir.join(agent_id).join("feedback");
        let mut read_dir = match tokio::fs::read_dir(&dir).await {
            Ok(rd) => rd,
            Err(_) => return Vec::new(),
        };

        let mut out: Vec<FeedbackFloorEntry> = Vec::new();
        let mut scanned = 0usize;
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            if scanned >= SCAN_CAP {
                break;
            }
            let file = entry.path();
            if file.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            scanned += 1;
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

        // Critical before High (Severity derives Ord in ascending declaration
        // order), then freshest first.
        out.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        out.truncate(FLOOR_CAP);
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

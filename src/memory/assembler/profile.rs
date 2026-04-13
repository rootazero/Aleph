//! UserProfileLoader — reads `personal/profile.md` for an agent. If the file
//! is missing or unreadable, returns `None` (never an error).

use crate::sync_primitives::Arc;
use std::path::PathBuf;

/// Loader for `memory/note/{agent_id}/personal/profile.md`. Kept behind an
/// `Arc` so it can be shared across the assembler and its tests.
pub struct UserProfileLoader {
    memory_dir: PathBuf,
}

impl UserProfileLoader {
    pub fn new(memory_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self { memory_dir })
    }

    /// Returns the profile body if readable, else `None`. Stripped of
    /// frontmatter for direct injection.
    pub async fn load(&self, agent_id: &str) -> Option<String> {
        let path = self
            .memory_dir
            .join(agent_id)
            .join("personal")
            .join("profile.md");
        let body = tokio::fs::read_to_string(&path).await.ok()?;
        Some(strip_frontmatter(&body))
    }

    /// Expose the expected path for diagnostics/tests.
    #[allow(dead_code)]
    pub fn path_for(&self, agent_id: &str) -> PathBuf {
        self.memory_dir
            .join(agent_id)
            .join("personal")
            .join("profile.md")
    }
}

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

    #[tokio::test]
    async fn missing_file_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let loader = UserProfileLoader::new(tmp.path().to_path_buf());
        assert!(loader.load("default").await.is_none());
    }

    #[tokio::test]
    async fn reads_profile_and_strips_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("default").join("personal");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let content = "---\ncategory: personal\n---\nuser prefers rust";
        tokio::fs::write(dir.join("profile.md"), content)
            .await
            .unwrap();

        let loader = UserProfileLoader::new(tmp.path().to_path_buf());
        let got = loader.load("default").await.unwrap();
        assert_eq!(got, "user prefers rust");
    }

    #[test]
    fn strip_frontmatter_preserves_body_without_frontmatter() {
        assert_eq!(strip_frontmatter("hello"), "hello");
    }
}

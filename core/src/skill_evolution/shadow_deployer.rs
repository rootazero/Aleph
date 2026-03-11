//! Shadow deployer for evolved skills with promote/demote lifecycle.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use tokio::fs;

use super::lifecycle::{
    EvolvedSkillMetadata, LifecycleTransition, ShadowState, SkillLifecycleState, SkillOrigin,
};

/// Represents a deployed shadow skill on disk.
#[derive(Debug, Clone)]
pub struct ShadowDeployment {
    pub skill_path: PathBuf,
    pub meta_path: PathBuf,
    pub skill_id: String,
}

/// Deploys, promotes, and demotes evolved skills.
pub struct ShadowDeployer {
    pub evolved_dir: PathBuf,
    pub official_dir: PathBuf,
}

impl ShadowDeployer {
    pub fn new(evolved_dir: PathBuf, official_dir: PathBuf) -> Self {
        Self {
            evolved_dir,
            official_dir,
        }
    }

    /// Deploy a skill into shadow (evolved) directory.
    pub async fn deploy(
        &self,
        skill_id: &str,
        skill_content: &str,
        pattern_id: &str,
    ) -> Result<ShadowDeployment> {
        let skill_dir = self.evolved_dir.join(skill_id);
        fs::create_dir_all(&skill_dir).await?;

        let skill_path = skill_dir.join("SKILL.md");
        fs::write(&skill_path, skill_content).await?;

        let meta = EvolvedSkillMetadata {
            skill_id: skill_id.to_string(),
            lifecycle: SkillLifecycleState::Shadow(ShadowState {
                deployed_at: now_ms(),
                invocation_count: 0,
                success_count: 0,
            }),
            origin: SkillOrigin {
                pattern_id: pattern_id.to_string(),
                source_experiences: vec![],
                generator_version: env!("CARGO_PKG_VERSION").to_string(),
                created_at: now_ms(),
            },
            risk_level: "low".to_string(),
            validation_history: vec![],
        };

        let meta_path = skill_dir.join("metadata.json");
        let meta_json = serde_json::to_string_pretty(&meta)?;
        fs::write(&meta_path, meta_json).await?;

        Ok(ShadowDeployment {
            skill_path,
            meta_path,
            skill_id: skill_id.to_string(),
        })
    }

    /// Promote a skill from evolved to official directory.
    pub async fn promote(&self, skill_id: &str) -> Result<LifecycleTransition> {
        let src = self.evolved_dir.join(skill_id);
        let dst = self.official_dir.join(skill_id);

        fs::create_dir_all(&self.official_dir).await?;
        fs::rename(&src, &dst).await?;

        // Update metadata lifecycle
        let meta_path = dst.join("metadata.json");
        let meta_raw = fs::read_to_string(&meta_path).await?;
        let mut meta: EvolvedSkillMetadata = serde_json::from_str(&meta_raw)?;
        meta.lifecycle = SkillLifecycleState::Promoted {
            promoted_at: now_ms(),
            shadow_duration_days: 0,
        };
        let meta_json = serde_json::to_string_pretty(&meta)?;
        fs::write(&meta_path, meta_json).await?;

        Ok(LifecycleTransition::Promoted)
    }

    /// Demote a skill by removing it from the evolved directory.
    pub async fn demote(&self, skill_id: &str, reason: &str) -> Result<LifecycleTransition> {
        let skill_dir = self.evolved_dir.join(skill_id);

        tracing::info!(
            skill_id = skill_id,
            reason = reason,
            "Demoting skill from shadow deployment"
        );

        fs::remove_dir_all(&skill_dir).await?;

        Ok(LifecycleTransition::Demoted {
            reason: reason.to_string(),
        })
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deploy_creates_files() {
        let tmp = tempfile::tempdir().unwrap();
        let evolved = tmp.path().join("evolved");
        let official = tmp.path().join("official");
        let deployer = ShadowDeployer::new(evolved.clone(), official);

        let deployment = deployer
            .deploy("test-skill", "# Test Skill\nDoes things.", "pattern-001")
            .await
            .unwrap();

        assert!(deployment.skill_path.exists());
        assert!(deployment.meta_path.exists());
        assert_eq!(deployment.skill_id, "test-skill");

        let content = fs::read_to_string(&deployment.skill_path).await.unwrap();
        assert!(content.contains("Test Skill"));
    }

    #[tokio::test]
    async fn promote_moves_to_official() {
        let tmp = tempfile::tempdir().unwrap();
        let evolved = tmp.path().join("evolved");
        let official = tmp.path().join("official");
        let deployer = ShadowDeployer::new(evolved.clone(), official.clone());

        deployer
            .deploy("test-skill", "# Skill", "pattern-001")
            .await
            .unwrap();

        let transition = deployer.promote("test-skill").await.unwrap();
        assert_eq!(transition, LifecycleTransition::Promoted);
        assert!(official.join("test-skill").join("SKILL.md").exists());
        assert!(!evolved.join("test-skill").exists());
    }

    #[tokio::test]
    async fn demote_removes_from_evolved() {
        let tmp = tempfile::tempdir().unwrap();
        let evolved = tmp.path().join("evolved");
        let official = tmp.path().join("official");
        let deployer = ShadowDeployer::new(evolved.clone(), official);

        deployer
            .deploy("test-skill", "# Skill", "pattern-001")
            .await
            .unwrap();

        let transition = deployer.demote("test-skill", "too slow").await.unwrap();
        assert_eq!(
            transition,
            LifecycleTransition::Demoted {
                reason: "too slow".to_string()
            }
        );
        assert!(!evolved.join("test-skill").exists());
    }
}

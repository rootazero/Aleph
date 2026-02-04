# Skill 进化系统实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 实现经验自动固化为 Skill 的闭环：跟踪执行指标 → 检测重复模式 → 生成 Skill 建议 → 自动 Git 提交。

**Architecture:** 新增 `skill_evolution` 模块，复用 Memory 系统的架构模式（双层存储、Ebbinghaus 衰减、指标聚合）。通过 `EvolutionTracker` 跟踪执行，`SolidificationDetector` 检测阈值，`SkillGenerator` 生成 SKILL.md，`GitCommitter` 自动提交。

**Tech Stack:** 复用 memory 模块的 sqlite-vec 基础设施；serde_json 序列化；git2 自动提交

---

## 现有模块分析

**可复用：**
- ✅ `MemoryStrength` - access_count, last_accessed 追踪模式
- ✅ `DecayConfig` - Ebbinghaus 衰减配置
- ✅ `VectorDatabase` - sqlite 基础设施
- ✅ `SkillsRegistry` - Skill 发现和加载
- ✅ `Skill` / `SkillFrontmatter` - Skill 结构定义

**需要新增：**
- ❌ `SkillExecution` - 执行记录
- ❌ `SkillMetrics` - 聚合指标 (use_count, success_count)
- ❌ `EvolutionTracker` - 执行跟踪器
- ❌ `SolidificationDetector` - 固化建议检测器
- ❌ `SkillGenerator` - SKILL.md 生成器
- ❌ `GitCommitter` - Git 自动提交

---

## Task 1: 定义核心类型

**Files:**
- Create: `core/src/skill_evolution/types.rs`
- Create: `core/src/skill_evolution/mod.rs`

**Step 1: 创建 types.rs**

创建 `/Volumes/TBU4/Workspace/Aleph/core/src/skill_evolution/types.rs`：

```rust
//! Core types for skill evolution system.
//!
//! Tracks skill executions and metrics to enable automatic solidification.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Status of a skill execution
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Success,
    PartialSuccess,
    Failed,
    Error,
}

/// A single skill execution record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecution {
    /// Unique execution ID
    pub id: String,
    /// Skill ID (or pattern hash for ad-hoc patterns)
    pub skill_id: String,
    /// Session ID where execution occurred
    pub session_id: String,
    /// Unix timestamp when invoked
    pub invoked_at: i64,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Execution status
    pub status: ExecutionStatus,
    /// User satisfaction score (0.0-1.0) if feedback provided
    pub satisfaction: Option<f32>,
    /// Context description (what was the user trying to do)
    pub context: String,
    /// Input summary (truncated)
    pub input_summary: String,
    /// Output length in characters
    pub output_length: u32,
}

impl SkillExecution {
    /// Create a new successful execution
    pub fn success(
        skill_id: impl Into<String>,
        session_id: impl Into<String>,
        context: impl Into<String>,
        input_summary: impl Into<String>,
        duration_ms: u64,
        output_length: u32,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            skill_id: skill_id.into(),
            session_id: session_id.into(),
            invoked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            duration_ms,
            status: ExecutionStatus::Success,
            satisfaction: None,
            context: context.into(),
            input_summary: input_summary.into(),
            output_length,
        }
    }

    /// Create a failed execution
    pub fn failed(
        skill_id: impl Into<String>,
        session_id: impl Into<String>,
        context: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            skill_id: skill_id.into(),
            session_id: session_id.into(),
            invoked_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64,
            duration_ms: 0,
            status: ExecutionStatus::Failed,
            satisfaction: None,
            context: context.into(),
            input_summary: String::new(),
            output_length: 0,
        }
    }

    /// Set user satisfaction
    pub fn with_satisfaction(mut self, score: f32) -> Self {
        self.satisfaction = Some(score.clamp(0.0, 1.0));
        self
    }
}

/// Aggregated metrics for a skill or pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetrics {
    /// Skill ID or pattern hash
    pub skill_id: String,
    /// Total number of executions
    pub total_executions: u64,
    /// Number of successful executions
    pub successful_executions: u64,
    /// Average duration in milliseconds
    pub avg_duration_ms: f32,
    /// Average satisfaction score (if feedback exists)
    pub avg_satisfaction: Option<f32>,
    /// Failure rate (0.0-1.0)
    pub failure_rate: f32,
    /// Last execution timestamp
    pub last_used: i64,
    /// First execution timestamp
    pub first_used: i64,
    /// Context frequency map (context -> count)
    pub context_frequency: HashMap<String, u32>,
}

impl SkillMetrics {
    /// Create empty metrics
    pub fn new(skill_id: impl Into<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        Self {
            skill_id: skill_id.into(),
            total_executions: 0,
            successful_executions: 0,
            avg_duration_ms: 0.0,
            avg_satisfaction: None,
            failure_rate: 0.0,
            last_used: now,
            first_used: now,
            context_frequency: HashMap::new(),
        }
    }

    /// Success rate (0.0-1.0)
    pub fn success_rate(&self) -> f32 {
        if self.total_executions == 0 {
            0.0
        } else {
            self.successful_executions as f32 / self.total_executions as f32
        }
    }

    /// Check if metrics meet solidification threshold
    pub fn meets_threshold(&self, config: &SolidificationConfig) -> bool {
        self.successful_executions >= config.min_success_count as u64
            && self.success_rate() >= config.min_success_rate
    }
}

/// Configuration for solidification detection
#[derive(Debug, Clone)]
pub struct SolidificationConfig {
    /// Minimum successful executions before suggesting solidification
    pub min_success_count: u32,
    /// Minimum success rate (0.0-1.0)
    pub min_success_rate: f32,
    /// Minimum days since first use
    pub min_age_days: u32,
    /// Maximum days since last use (to avoid stale patterns)
    pub max_idle_days: u32,
}

impl Default for SolidificationConfig {
    fn default() -> Self {
        Self {
            min_success_count: 3,
            min_success_rate: 0.8,
            min_age_days: 1,
            max_idle_days: 30,
        }
    }
}

/// A suggestion to solidify a pattern into a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolidificationSuggestion {
    /// Pattern hash or temporary skill ID
    pub pattern_id: String,
    /// Suggested skill name
    pub suggested_name: String,
    /// Suggested description
    pub suggested_description: String,
    /// Confidence score (0.0-1.0)
    pub confidence: f32,
    /// Metrics that triggered this suggestion
    pub metrics: SkillMetrics,
    /// Sample contexts where this pattern was used
    pub sample_contexts: Vec<String>,
    /// Generated instructions preview
    pub instructions_preview: String,
}

/// Result of skill generation
#[derive(Debug, Clone)]
pub enum GenerationResult {
    /// Successfully generated skill
    Generated {
        skill_id: String,
        file_path: String,
        diff_preview: String,
    },
    /// Skill already exists
    AlreadyExists { skill_id: String },
    /// Generation failed
    Failed { reason: String },
}

/// Result of git commit operation
#[derive(Debug, Clone)]
pub enum CommitResult {
    /// Successfully committed
    Committed {
        commit_hash: String,
        files_changed: Vec<String>,
    },
    /// Nothing to commit
    NothingToCommit,
    /// Commit failed
    Failed { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skill_execution_success() {
        let exec = SkillExecution::success(
            "test-skill",
            "session-1",
            "refactoring code",
            "refactor the auth module",
            1500,
            2000,
        );
        assert_eq!(exec.status, ExecutionStatus::Success);
        assert_eq!(exec.skill_id, "test-skill");
    }

    #[test]
    fn test_skill_metrics_success_rate() {
        let mut metrics = SkillMetrics::new("test");
        metrics.total_executions = 10;
        metrics.successful_executions = 8;
        assert_eq!(metrics.success_rate(), 0.8);
    }

    #[test]
    fn test_solidification_threshold() {
        let config = SolidificationConfig::default();
        let mut metrics = SkillMetrics::new("test");

        // Not enough executions
        metrics.total_executions = 2;
        metrics.successful_executions = 2;
        assert!(!metrics.meets_threshold(&config));

        // Meets threshold
        metrics.total_executions = 4;
        metrics.successful_executions = 4;
        assert!(metrics.meets_threshold(&config));
    }
}
```

**Step 2: 创建 mod.rs**

创建 `/Volumes/TBU4/Workspace/Aleph/core/src/skill_evolution/mod.rs`：

```rust
//! Skill evolution system.
//!
//! Tracks skill executions, detects patterns, and suggests solidification
//! of repeated successful patterns into permanent skills.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
//! │ EvolutionTracker│────▶│SolidificationDet│────▶│  SkillGenerator │
//! │  (Log Executions)│     │ (Check Thresholds)│    │ (Create SKILL.md)│
//! └─────────────────┘     └─────────────────┘     └────────┬────────┘
//!                                                          │
//!                                                          ▼
//!                                                  ┌─────────────────┐
//!                                                  │   GitCommitter  │
//!                                                  │ (Auto-commit)   │
//!                                                  └─────────────────┘
//! ```

pub mod types;

pub use types::{
    CommitResult, ExecutionStatus, GenerationResult, SkillExecution, SkillMetrics,
    SolidificationConfig, SolidificationSuggestion,
};
```

**Step 3: 更新 lib.rs**

在 `/Volumes/TBU4/Workspace/Aleph/core/src/lib.rs` 添加模块声明和导出。

**Step 4: 运行测试**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo test skill_evolution::types::tests
```

**Step 5: Commit**

```bash
git add core/src/skill_evolution/ core/src/lib.rs
git commit -m "feat(skill_evolution): add core types for skill evolution system"
```

---

## Task 2: 实现 EvolutionTracker

**Files:**
- Create: `core/src/skill_evolution/tracker.rs`
- Modify: `core/src/skill_evolution/mod.rs`

**Step 1: 创建 tracker.rs**

创建 `/Volumes/TBU4/Workspace/Aleph/core/src/skill_evolution/tracker.rs`：

```rust
//! EvolutionTracker - logs skill executions and maintains metrics.
//!
//! Uses SQLite for persistence with similar schema to memory module.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use rusqlite::{params, Connection, OptionalExtension};
use tracing::{debug, info, warn};

use crate::error::{AlephError, Result};

use super::types::{ExecutionStatus, SkillExecution, SkillMetrics};

/// SQL schema for skill evolution tables
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS skill_executions (
    id TEXT PRIMARY KEY,
    skill_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    invoked_at INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    status TEXT NOT NULL,
    satisfaction REAL,
    context TEXT NOT NULL,
    input_summary TEXT NOT NULL,
    output_length INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_executions_skill ON skill_executions(skill_id);
CREATE INDEX IF NOT EXISTS idx_executions_time ON skill_executions(invoked_at);

CREATE TABLE IF NOT EXISTS skill_metrics (
    skill_id TEXT PRIMARY KEY,
    total_executions INTEGER NOT NULL,
    successful_executions INTEGER NOT NULL,
    avg_duration_ms REAL NOT NULL,
    avg_satisfaction REAL,
    failure_rate REAL NOT NULL,
    last_used INTEGER NOT NULL,
    first_used INTEGER NOT NULL,
    context_frequency TEXT NOT NULL
);
"#;

/// Tracker for skill executions and metrics
pub struct EvolutionTracker {
    conn: Arc<RwLock<Connection>>,
    /// In-memory cache for hot metrics
    metrics_cache: Arc<RwLock<HashMap<String, SkillMetrics>>>,
}

impl EvolutionTracker {
    /// Create a new tracker with database at the given path
    pub fn new(db_path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(db_path.as_ref()).map_err(|e| AlephError::ConfigError {
            message: format!("Failed to open evolution database: {}", e),
            suggestion: Some("Check database path permissions".to_string()),
        })?;

        conn.execute_batch(SCHEMA).map_err(|e| AlephError::ConfigError {
            message: format!("Failed to create schema: {}", e),
            suggestion: None,
        })?;

        info!(path = %db_path.as_ref().display(), "Evolution tracker initialized");

        Ok(Self {
            conn: Arc::new(RwLock::new(conn)),
            metrics_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Create an in-memory tracker (for testing)
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|e| AlephError::ConfigError {
            message: format!("Failed to create in-memory database: {}", e),
            suggestion: None,
        })?;

        conn.execute_batch(SCHEMA).map_err(|e| AlephError::ConfigError {
            message: format!("Failed to create schema: {}", e),
            suggestion: None,
        })?;

        Ok(Self {
            conn: Arc::new(RwLock::new(conn)),
            metrics_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Log a skill execution
    pub fn log_execution(&self, execution: &SkillExecution) -> Result<()> {
        let conn = self.conn.write().map_err(|_| AlephError::Other {
            message: "Failed to acquire database lock".to_string(),
            suggestion: None,
        })?;

        let status_str = match execution.status {
            ExecutionStatus::Success => "success",
            ExecutionStatus::PartialSuccess => "partial_success",
            ExecutionStatus::Failed => "failed",
            ExecutionStatus::Error => "error",
        };

        conn.execute(
            "INSERT INTO skill_executions (id, skill_id, session_id, invoked_at, duration_ms, status, satisfaction, context, input_summary, output_length)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                execution.id,
                execution.skill_id,
                execution.session_id,
                execution.invoked_at,
                execution.duration_ms,
                status_str,
                execution.satisfaction,
                execution.context,
                execution.input_summary,
                execution.output_length,
            ],
        ).map_err(|e| AlephError::ConfigError {
            message: format!("Failed to insert execution: {}", e),
            suggestion: None,
        })?;

        debug!(skill_id = %execution.skill_id, status = %status_str, "Logged execution");

        // Update metrics
        drop(conn);
        self.update_metrics(&execution.skill_id)?;

        Ok(())
    }

    /// Get metrics for a skill
    pub fn get_metrics(&self, skill_id: &str) -> Result<Option<SkillMetrics>> {
        // Check cache first
        {
            let cache = self.metrics_cache.read().map_err(|_| AlephError::Other {
                message: "Failed to acquire cache lock".to_string(),
                suggestion: None,
            })?;
            if let Some(metrics) = cache.get(skill_id) {
                return Ok(Some(metrics.clone()));
            }
        }

        // Load from database
        let conn = self.conn.read().map_err(|_| AlephError::Other {
            message: "Failed to acquire database lock".to_string(),
            suggestion: None,
        })?;

        let result: Option<(i64, i64, f64, Option<f64>, f64, i64, i64, String)> = conn
            .query_row(
                "SELECT total_executions, successful_executions, avg_duration_ms, avg_satisfaction, failure_rate, last_used, first_used, context_frequency
                 FROM skill_metrics WHERE skill_id = ?1",
                params![skill_id],
                |row| Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                )),
            )
            .optional()
            .map_err(|e| AlephError::ConfigError {
                message: format!("Failed to query metrics: {}", e),
                suggestion: None,
            })?;

        match result {
            Some((total, successful, avg_duration, avg_satisfaction, failure_rate, last_used, first_used, context_json)) => {
                let context_frequency: HashMap<String, u32> = serde_json::from_str(&context_json).unwrap_or_default();
                Ok(Some(SkillMetrics {
                    skill_id: skill_id.to_string(),
                    total_executions: total as u64,
                    successful_executions: successful as u64,
                    avg_duration_ms: avg_duration as f32,
                    avg_satisfaction: avg_satisfaction.map(|s| s as f32),
                    failure_rate: failure_rate as f32,
                    last_used,
                    first_used,
                    context_frequency,
                }))
            }
            None => Ok(None),
        }
    }

    /// Get all skills that meet the solidification threshold
    pub fn get_solidification_candidates(
        &self,
        config: &super::SolidificationConfig,
    ) -> Result<Vec<SkillMetrics>> {
        let conn = self.conn.read().map_err(|_| AlephError::Other {
            message: "Failed to acquire database lock".to_string(),
            suggestion: None,
        })?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let min_age_secs = config.min_age_days as i64 * 86400;
        let max_idle_secs = config.max_idle_days as i64 * 86400;

        let mut stmt = conn
            .prepare(
                "SELECT skill_id, total_executions, successful_executions, avg_duration_ms, avg_satisfaction, failure_rate, last_used, first_used, context_frequency
                 FROM skill_metrics
                 WHERE successful_executions >= ?1
                   AND (1.0 - failure_rate) >= ?2
                   AND (?3 - first_used) >= ?4
                   AND (?3 - last_used) <= ?5",
            )
            .map_err(|e| AlephError::ConfigError {
                message: format!("Failed to prepare query: {}", e),
                suggestion: None,
            })?;

        let rows = stmt
            .query_map(
                params![
                    config.min_success_count,
                    config.min_success_rate,
                    now,
                    min_age_secs,
                    max_idle_secs,
                ],
                |row| {
                    let context_json: String = row.get(8)?;
                    let context_frequency: HashMap<String, u32> =
                        serde_json::from_str(&context_json).unwrap_or_default();
                    Ok(SkillMetrics {
                        skill_id: row.get(0)?,
                        total_executions: row.get::<_, i64>(1)? as u64,
                        successful_executions: row.get::<_, i64>(2)? as u64,
                        avg_duration_ms: row.get::<_, f64>(3)? as f32,
                        avg_satisfaction: row.get::<_, Option<f64>>(4)?.map(|s| s as f32),
                        failure_rate: row.get::<_, f64>(5)? as f32,
                        last_used: row.get(6)?,
                        first_used: row.get(7)?,
                        context_frequency,
                    })
                },
            )
            .map_err(|e| AlephError::ConfigError {
                message: format!("Failed to query candidates: {}", e),
                suggestion: None,
            })?;

        let candidates: Vec<SkillMetrics> = rows.filter_map(|r| r.ok()).collect();
        info!(count = candidates.len(), "Found solidification candidates");
        Ok(candidates)
    }

    /// Update metrics for a skill based on all executions
    fn update_metrics(&self, skill_id: &str) -> Result<()> {
        let conn = self.conn.write().map_err(|_| AlephError::Other {
            message: "Failed to acquire database lock".to_string(),
            suggestion: None,
        })?;

        // Aggregate from executions
        let stats: (i64, i64, f64, Option<f64>, i64, i64) = conn
            .query_row(
                "SELECT
                    COUNT(*) as total,
                    SUM(CASE WHEN status = 'success' OR status = 'partial_success' THEN 1 ELSE 0 END) as successful,
                    AVG(duration_ms) as avg_duration,
                    AVG(satisfaction) as avg_satisfaction,
                    MAX(invoked_at) as last_used,
                    MIN(invoked_at) as first_used
                 FROM skill_executions WHERE skill_id = ?1",
                params![skill_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .map_err(|e| AlephError::ConfigError {
                message: format!("Failed to aggregate stats: {}", e),
                suggestion: None,
            })?;

        let (total, successful, avg_duration, avg_satisfaction, last_used, first_used) = stats;
        let failure_rate = if total > 0 {
            1.0 - (successful as f64 / total as f64)
        } else {
            0.0
        };

        // Get context frequency
        let mut context_freq: HashMap<String, u32> = HashMap::new();
        {
            let mut stmt = conn
                .prepare("SELECT context FROM skill_executions WHERE skill_id = ?1")
                .map_err(|e| AlephError::ConfigError {
                    message: format!("Failed to prepare context query: {}", e),
                    suggestion: None,
                })?;
            let contexts = stmt
                .query_map(params![skill_id], |row| row.get::<_, String>(0))
                .map_err(|e| AlephError::ConfigError {
                    message: format!("Failed to query contexts: {}", e),
                    suggestion: None,
                })?;
            for ctx in contexts.filter_map(|r| r.ok()) {
                *context_freq.entry(ctx).or_insert(0) += 1;
            }
        }

        let context_json = serde_json::to_string(&context_freq).unwrap_or_else(|_| "{}".to_string());

        // Upsert metrics
        conn.execute(
            "INSERT INTO skill_metrics (skill_id, total_executions, successful_executions, avg_duration_ms, avg_satisfaction, failure_rate, last_used, first_used, context_frequency)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(skill_id) DO UPDATE SET
                total_executions = ?2,
                successful_executions = ?3,
                avg_duration_ms = ?4,
                avg_satisfaction = ?5,
                failure_rate = ?6,
                last_used = ?7,
                context_frequency = ?9",
            params![
                skill_id,
                total,
                successful,
                avg_duration,
                avg_satisfaction,
                failure_rate,
                last_used,
                first_used,
                context_json,
            ],
        ).map_err(|e| AlephError::ConfigError {
            message: format!("Failed to upsert metrics: {}", e),
            suggestion: None,
        })?;

        // Update cache
        drop(conn);
        let metrics = SkillMetrics {
            skill_id: skill_id.to_string(),
            total_executions: total as u64,
            successful_executions: successful as u64,
            avg_duration_ms: avg_duration as f32,
            avg_satisfaction: avg_satisfaction.map(|s| s as f32),
            failure_rate: failure_rate as f32,
            last_used,
            first_used,
            context_frequency: context_freq,
        };

        let mut cache = self.metrics_cache.write().map_err(|_| AlephError::Other {
            message: "Failed to acquire cache lock".to_string(),
            suggestion: None,
        })?;
        cache.insert(skill_id.to_string(), metrics);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_and_get_metrics() {
        let tracker = EvolutionTracker::in_memory().unwrap();

        // Log some executions
        for i in 0..5 {
            let exec = SkillExecution::success(
                "test-skill",
                format!("session-{}", i),
                "testing",
                "test input",
                100,
                500,
            );
            tracker.log_execution(&exec).unwrap();
        }

        let metrics = tracker.get_metrics("test-skill").unwrap().unwrap();
        assert_eq!(metrics.total_executions, 5);
        assert_eq!(metrics.successful_executions, 5);
        assert_eq!(metrics.success_rate(), 1.0);
    }

    #[test]
    fn test_solidification_candidates() {
        let tracker = EvolutionTracker::in_memory().unwrap();
        let config = super::super::SolidificationConfig {
            min_success_count: 3,
            min_success_rate: 0.8,
            min_age_days: 0, // For testing
            max_idle_days: 100,
        };

        // Log enough executions
        for i in 0..4 {
            let exec = SkillExecution::success(
                "candidate-skill",
                format!("session-{}", i),
                "testing",
                "test input",
                100,
                500,
            );
            tracker.log_execution(&exec).unwrap();
        }

        let candidates = tracker.get_solidification_candidates(&config).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].skill_id, "candidate-skill");
    }
}
```

**Step 2: 更新 mod.rs**

添加 `pub mod tracker;` 和 `pub use tracker::EvolutionTracker;`

**Step 3: Commit**

```bash
git add core/src/skill_evolution/
git commit -m "feat(skill_evolution): implement EvolutionTracker for execution logging"
```

---

## Task 3: 实现 SolidificationDetector

**Files:**
- Create: `core/src/skill_evolution/detector.rs`
- Modify: `core/src/skill_evolution/mod.rs`

**Step 1: 创建 detector.rs**

创建 `/Volumes/TBU4/Workspace/Aleph/core/src/skill_evolution/detector.rs`：

```rust
//! SolidificationDetector - detects patterns ready for solidification.
//!
//! Analyzes execution metrics and generates solidification suggestions.

use std::sync::Arc;

use tracing::info;

use crate::error::Result;
use crate::providers::AiProvider;

use super::tracker::EvolutionTracker;
use super::types::{SkillMetrics, SolidificationConfig, SolidificationSuggestion};

/// System prompt for generating skill suggestions
const SUGGESTION_SYSTEM_PROMPT: &str = r#"You are a skill extraction expert. Based on the execution metrics and sample contexts, generate a skill suggestion.

Output a JSON object:
{
  "suggested_name": "kebab-case-name",
  "suggested_description": "One sentence description",
  "instructions_preview": "Markdown instructions (2-3 paragraphs max)"
}

Rules:
- Name should be descriptive and kebab-case
- Description should explain what the skill does
- Instructions should be concise but complete
- Focus on the most common use cases from the contexts
- Output ONLY valid JSON, no markdown"#;

/// Detector for solidification candidates
pub struct SolidificationDetector {
    tracker: Arc<EvolutionTracker>,
    config: SolidificationConfig,
    provider: Option<Arc<dyn AiProvider>>,
}

impl SolidificationDetector {
    /// Create a new detector
    pub fn new(tracker: Arc<EvolutionTracker>) -> Self {
        Self {
            tracker,
            config: SolidificationConfig::default(),
            provider: None,
        }
    }

    /// Set configuration
    pub fn with_config(mut self, config: SolidificationConfig) -> Self {
        self.config = config;
        self
    }

    /// Set AI provider for generating suggestions
    pub fn with_provider(mut self, provider: Arc<dyn AiProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Detect all candidates ready for solidification
    pub fn detect_candidates(&self) -> Result<Vec<SkillMetrics>> {
        self.tracker.get_solidification_candidates(&self.config)
    }

    /// Generate a solidification suggestion for a candidate
    pub async fn generate_suggestion(
        &self,
        metrics: &SkillMetrics,
    ) -> Result<SolidificationSuggestion> {
        let sample_contexts: Vec<String> = metrics
            .context_frequency
            .iter()
            .take(5)
            .map(|(ctx, _)| ctx.clone())
            .collect();

        // If we have an AI provider, use it to generate better suggestions
        if let Some(provider) = &self.provider {
            let prompt = format!(
                r#"Generate a skill suggestion based on these metrics:

Pattern ID: {}
Total executions: {}
Success rate: {:.0}%
Sample contexts:
{}

Generate a skill that captures this pattern."#,
                metrics.skill_id,
                metrics.total_executions,
                metrics.success_rate() * 100.0,
                sample_contexts
                    .iter()
                    .map(|c| format!("- {}", c))
                    .collect::<Vec<_>>()
                    .join("\n")
            );

            let response = provider.process(&prompt, Some(SUGGESTION_SYSTEM_PROMPT)).await?;
            let parsed = parse_suggestion_response(&response, metrics, &sample_contexts)?;
            return Ok(parsed);
        }

        // Fallback: generate simple suggestion
        let suggested_name = generate_name_from_contexts(&sample_contexts);
        let suggested_description = format!(
            "Auto-generated skill from {} successful executions",
            metrics.successful_executions
        );

        Ok(SolidificationSuggestion {
            pattern_id: metrics.skill_id.clone(),
            suggested_name,
            suggested_description,
            confidence: metrics.success_rate(),
            metrics: metrics.clone(),
            sample_contexts,
            instructions_preview: "# Instructions\n\nThis skill was auto-generated from repeated successful patterns.".to_string(),
        })
    }

    /// Check if any candidates exist
    pub fn has_candidates(&self) -> Result<bool> {
        let candidates = self.detect_candidates()?;
        Ok(!candidates.is_empty())
    }
}

/// Parse AI-generated suggestion response
fn parse_suggestion_response(
    response: &str,
    metrics: &SkillMetrics,
    sample_contexts: &[String],
) -> Result<SolidificationSuggestion> {
    use crate::error::AlephError;
    use crate::spec_driven::spec_writer::extract_json;

    let json_str = extract_json(response);

    #[derive(serde::Deserialize)]
    struct SuggestionResponse {
        suggested_name: String,
        suggested_description: String,
        instructions_preview: String,
    }

    let parsed: SuggestionResponse = serde_json::from_str(&json_str).map_err(|e| {
        AlephError::Other {
            message: format!("Failed to parse suggestion: {}", e),
            suggestion: None,
        }
    })?;

    Ok(SolidificationSuggestion {
        pattern_id: metrics.skill_id.clone(),
        suggested_name: parsed.suggested_name,
        suggested_description: parsed.suggested_description,
        confidence: metrics.success_rate(),
        metrics: metrics.clone(),
        sample_contexts: sample_contexts.to_vec(),
        instructions_preview: parsed.instructions_preview,
    })
}

/// Generate a simple name from contexts
fn generate_name_from_contexts(contexts: &[String]) -> String {
    if contexts.is_empty() {
        return "auto-skill".to_string();
    }

    // Extract common words
    let first = contexts[0].to_lowercase();
    let words: Vec<&str> = first.split_whitespace().take(3).collect();

    words.join("-").chars().filter(|c| c.is_alphanumeric() || *c == '-').collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_name_from_contexts() {
        let contexts = vec!["refactor authentication module".to_string()];
        let name = generate_name_from_contexts(&contexts);
        assert_eq!(name, "refactor-authentication-module");
    }

    #[test]
    fn test_generate_name_empty() {
        let contexts: Vec<String> = vec![];
        let name = generate_name_from_contexts(&contexts);
        assert_eq!(name, "auto-skill");
    }
}
```

**Step 2: 更新 mod.rs**

**Step 3: Commit**

```bash
git add core/src/skill_evolution/
git commit -m "feat(skill_evolution): implement SolidificationDetector for pattern detection"
```

---

## Task 4: 实现 SkillGenerator

**Files:**
- Create: `core/src/skill_evolution/generator.rs`
- Modify: `core/src/skill_evolution/mod.rs`

**Step 1: 创建 generator.rs**

创建 `/Volumes/TBU4/Workspace/Aleph/core/src/skill_evolution/generator.rs`：

```rust
//! SkillGenerator - generates SKILL.md files from suggestions.
//!
//! Creates properly formatted skill files with frontmatter and instructions.

use std::fs;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

use crate::error::{AlephError, Result};

use super::types::{GenerationResult, SolidificationSuggestion};

/// SKILL.md template
const SKILL_TEMPLATE: &str = r#"---
name: {name}
description: {description}
allowed-tools: []
triggers:
{triggers}
---

{instructions}

---

*Auto-generated by Aleph Skill Evolution System*
*Pattern ID: {pattern_id}*
*Confidence: {confidence:.0}%*
"#;

/// Generator for skill files
pub struct SkillGenerator {
    /// Directory where skills are stored
    skills_dir: PathBuf,
}

impl SkillGenerator {
    /// Create a new generator with the given skills directory
    pub fn new(skills_dir: impl Into<PathBuf>) -> Self {
        Self {
            skills_dir: skills_dir.into(),
        }
    }

    /// Create with default skills directory (~/.aleph/skills)
    pub fn with_default_dir() -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| AlephError::Other {
            message: "Could not determine home directory".to_string(),
            suggestion: None,
        })?;
        let skills_dir = home.join(".aleph").join("skills");
        Ok(Self::new(skills_dir))
    }

    /// Generate a skill from a suggestion
    pub fn generate(&self, suggestion: &SolidificationSuggestion) -> Result<GenerationResult> {
        let skill_dir = self.skills_dir.join(&suggestion.suggested_name);

        // Check if skill already exists
        if skill_dir.exists() {
            info!(skill_id = %suggestion.suggested_name, "Skill already exists");
            return Ok(GenerationResult::AlreadyExists {
                skill_id: suggestion.suggested_name.clone(),
            });
        }

        // Generate triggers from sample contexts
        let triggers = suggestion
            .sample_contexts
            .iter()
            .take(3)
            .map(|ctx| {
                // Extract first few words as trigger
                let words: Vec<&str> = ctx.split_whitespace().take(2).collect();
                format!("  - {}", words.join(" "))
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Generate SKILL.md content
        let content = SKILL_TEMPLATE
            .replace("{name}", &suggestion.suggested_name)
            .replace("{description}", &suggestion.suggested_description)
            .replace("{triggers}", &triggers)
            .replace("{instructions}", &suggestion.instructions_preview)
            .replace("{pattern_id}", &suggestion.pattern_id)
            .replace("{confidence}", &format!("{}", suggestion.confidence * 100.0));

        // Create directory
        fs::create_dir_all(&skill_dir).map_err(|e| AlephError::Other {
            message: format!("Failed to create skill directory: {}", e),
            suggestion: None,
        })?;

        // Write SKILL.md
        let file_path = skill_dir.join("SKILL.md");
        fs::write(&file_path, &content).map_err(|e| AlephError::Other {
            message: format!("Failed to write SKILL.md: {}", e),
            suggestion: None,
        })?;

        info!(
            skill_id = %suggestion.suggested_name,
            path = %file_path.display(),
            "Generated skill"
        );

        // Generate diff preview
        let diff_preview = format!(
            "+++ {}\n@@ -0,0 +1,{} @@\n{}",
            file_path.display(),
            content.lines().count(),
            content
                .lines()
                .map(|l| format!("+{}", l))
                .collect::<Vec<_>>()
                .join("\n")
        );

        Ok(GenerationResult::Generated {
            skill_id: suggestion.suggested_name.clone(),
            file_path: file_path.to_string_lossy().to_string(),
            diff_preview,
        })
    }

    /// Preview what would be generated (without writing)
    pub fn preview(&self, suggestion: &SolidificationSuggestion) -> String {
        let triggers = suggestion
            .sample_contexts
            .iter()
            .take(3)
            .map(|ctx| {
                let words: Vec<&str> = ctx.split_whitespace().take(2).collect();
                format!("  - {}", words.join(" "))
            })
            .collect::<Vec<_>>()
            .join("\n");

        SKILL_TEMPLATE
            .replace("{name}", &suggestion.suggested_name)
            .replace("{description}", &suggestion.suggested_description)
            .replace("{triggers}", &triggers)
            .replace("{instructions}", &suggestion.instructions_preview)
            .replace("{pattern_id}", &suggestion.pattern_id)
            .replace("{confidence}", &format!("{}", suggestion.confidence * 100.0))
    }

    /// Get the path where a skill would be created
    pub fn get_skill_path(&self, name: &str) -> PathBuf {
        self.skills_dir.join(name).join("SKILL.md")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_suggestion() -> SolidificationSuggestion {
        SolidificationSuggestion {
            pattern_id: "pattern-123".to_string(),
            suggested_name: "test-skill".to_string(),
            suggested_description: "A test skill".to_string(),
            confidence: 0.9,
            metrics: super::super::SkillMetrics::new("test"),
            sample_contexts: vec!["refactor code".to_string(), "improve tests".to_string()],
            instructions_preview: "# Test Skill\n\nDo the thing.".to_string(),
        }
    }

    #[test]
    fn test_generate_skill() {
        let dir = TempDir::new().unwrap();
        let generator = SkillGenerator::new(dir.path());
        let suggestion = test_suggestion();

        let result = generator.generate(&suggestion).unwrap();
        match result {
            GenerationResult::Generated { skill_id, file_path, .. } => {
                assert_eq!(skill_id, "test-skill");
                assert!(Path::new(&file_path).exists());
            }
            _ => panic!("Expected Generated result"),
        }
    }

    #[test]
    fn test_generate_existing() {
        let dir = TempDir::new().unwrap();
        let generator = SkillGenerator::new(dir.path());
        let suggestion = test_suggestion();

        // Generate first time
        generator.generate(&suggestion).unwrap();

        // Try to generate again
        let result = generator.generate(&suggestion).unwrap();
        match result {
            GenerationResult::AlreadyExists { skill_id } => {
                assert_eq!(skill_id, "test-skill");
            }
            _ => panic!("Expected AlreadyExists result"),
        }
    }

    #[test]
    fn test_preview() {
        let dir = TempDir::new().unwrap();
        let generator = SkillGenerator::new(dir.path());
        let suggestion = test_suggestion();

        let preview = generator.preview(&suggestion);
        assert!(preview.contains("name: test-skill"));
        assert!(preview.contains("A test skill"));
        assert!(preview.contains("# Test Skill"));
    }
}
```

**Step 2: 更新 mod.rs**

**Step 3: Commit**

```bash
git add core/src/skill_evolution/
git commit -m "feat(skill_evolution): implement SkillGenerator for SKILL.md creation"
```

---

## Task 5: 实现 GitCommitter

**Files:**
- Create: `core/src/skill_evolution/git.rs`
- Modify: `core/src/skill_evolution/mod.rs`

**Step 1: 创建 git.rs**

创建 `/Volumes/TBU4/Workspace/Aleph/core/src/skill_evolution/git.rs`：

```rust
//! GitCommitter - auto-commits generated skills to git.
//!
//! Handles staging, committing, and optionally pushing skill changes.

use std::path::Path;
use std::process::Command;

use tracing::{debug, info, warn};

use crate::error::{AlephError, Result};

use super::types::CommitResult;

/// Git operations for skill evolution
pub struct GitCommitter {
    /// Repository root directory
    repo_root: String,
    /// Whether to auto-push after commit
    auto_push: bool,
    /// Remote name for push (default: origin)
    remote: String,
    /// Branch name for push (default: main)
    branch: String,
}

impl GitCommitter {
    /// Create a new committer for the given repository
    pub fn new(repo_root: impl Into<String>) -> Self {
        Self {
            repo_root: repo_root.into(),
            auto_push: false,
            remote: "origin".to_string(),
            branch: "main".to_string(),
        }
    }

    /// Enable auto-push after commit
    pub fn with_auto_push(mut self, enabled: bool) -> Self {
        self.auto_push = enabled;
        self
    }

    /// Set remote for push
    pub fn with_remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = remote.into();
        self
    }

    /// Set branch for push
    pub fn with_branch(mut self, branch: impl Into<String>) -> Self {
        self.branch = branch.into();
        self
    }

    /// Commit a generated skill
    pub fn commit_skill(&self, skill_path: &str, skill_name: &str) -> Result<CommitResult> {
        // Check if path exists
        if !Path::new(skill_path).exists() {
            return Ok(CommitResult::Failed {
                reason: format!("Skill path does not exist: {}", skill_path),
            });
        }

        // Check if we're in a git repo
        if !self.is_git_repo() {
            return Ok(CommitResult::Failed {
                reason: "Not a git repository".to_string(),
            });
        }

        // Stage the skill directory
        let skill_dir = Path::new(skill_path).parent().unwrap_or(Path::new(skill_path));
        let stage_result = self.run_git(&["add", &skill_dir.to_string_lossy()]);
        if let Err(e) = stage_result {
            return Ok(CommitResult::Failed {
                reason: format!("Failed to stage: {}", e),
            });
        }

        // Check if there are changes to commit
        let status = self.run_git(&["status", "--porcelain"]);
        match status {
            Ok(output) if output.trim().is_empty() => {
                debug!("No changes to commit");
                return Ok(CommitResult::NothingToCommit);
            }
            Err(e) => {
                return Ok(CommitResult::Failed {
                    reason: format!("Failed to check status: {}", e),
                });
            }
            _ => {}
        }

        // Commit
        let message = format!(
            "feat(skills): auto-generate {} skill\n\nGenerated by Aleph Skill Evolution System",
            skill_name
        );
        let commit_result = self.run_git(&["commit", "-m", &message]);
        match commit_result {
            Ok(_) => {}
            Err(e) => {
                return Ok(CommitResult::Failed {
                    reason: format!("Failed to commit: {}", e),
                });
            }
        }

        // Get commit hash
        let hash = self
            .run_git(&["rev-parse", "HEAD"])
            .unwrap_or_else(|_| "unknown".to_string())
            .trim()
            .to_string();

        info!(
            skill = %skill_name,
            commit = %hash,
            "Committed skill"
        );

        // Auto-push if enabled
        if self.auto_push {
            match self.push() {
                Ok(_) => info!("Pushed to {}/{}", self.remote, self.branch),
                Err(e) => warn!("Failed to push: {}", e),
            }
        }

        Ok(CommitResult::Committed {
            commit_hash: hash,
            files_changed: vec![skill_path.to_string()],
        })
    }

    /// Push to remote
    pub fn push(&self) -> Result<()> {
        self.run_git(&["push", &self.remote, &self.branch])?;
        Ok(())
    }

    /// Check if current directory is a git repo
    fn is_git_repo(&self) -> bool {
        self.run_git(&["rev-parse", "--git-dir"]).is_ok()
    }

    /// Run a git command
    fn run_git(&self, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(&self.repo_root)
            .output()
            .map_err(|e| AlephError::Other {
                message: format!("Failed to run git: {}", e),
                suggestion: Some("Ensure git is installed".to_string()),
            })?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(AlephError::Other {
                message: format!(
                    "Git command failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
                suggestion: None,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_git_repo() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Initialize git repo
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Configure git user (required for commits)
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        // Create initial commit
        fs::write(dir.path().join("README.md"), "# Test").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "Initial"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        dir
    }

    #[test]
    fn test_commit_skill() {
        let dir = setup_git_repo();
        let committer = GitCommitter::new(dir.path().to_string_lossy().to_string());

        // Create a skill file
        let skill_dir = dir.path().join("skills").join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(&skill_path, "# Test Skill").unwrap();

        let result = committer
            .commit_skill(&skill_path.to_string_lossy(), "test-skill")
            .unwrap();

        match result {
            CommitResult::Committed { commit_hash, files_changed } => {
                assert!(!commit_hash.is_empty());
                assert_eq!(files_changed.len(), 1);
            }
            _ => panic!("Expected Committed result"),
        }
    }

    #[test]
    fn test_nothing_to_commit() {
        let dir = setup_git_repo();
        let committer = GitCommitter::new(dir.path().to_string_lossy().to_string());

        // Use existing committed file
        let readme_path = dir.path().join("README.md");

        let result = committer
            .commit_skill(&readme_path.to_string_lossy(), "readme")
            .unwrap();

        match result {
            CommitResult::NothingToCommit => {}
            _ => panic!("Expected NothingToCommit"),
        }
    }
}
```

**Step 2: 更新 mod.rs 完整导出**

```rust
//! Skill evolution system.

pub mod detector;
pub mod generator;
pub mod git;
pub mod tracker;
pub mod types;

pub use detector::SolidificationDetector;
pub use generator::SkillGenerator;
pub use git::GitCommitter;
pub use tracker::EvolutionTracker;
pub use types::{
    CommitResult, ExecutionStatus, GenerationResult, SkillExecution, SkillMetrics,
    SolidificationConfig, SolidificationSuggestion,
};
```

**Step 3: 更新 lib.rs 导出**

```rust
// Skill Evolution exports
pub use crate::skill_evolution::{
    CommitResult, EvolutionTracker, ExecutionStatus, GenerationResult, GitCommitter,
    SkillExecution, SkillGenerator, SkillMetrics, SolidificationConfig,
    SolidificationDetector, SolidificationSuggestion,
};
```

**Step 4: Commit**

```bash
git add core/src/skill_evolution/ core/src/lib.rs
git commit -m "feat(skill_evolution): implement GitCommitter for auto-commit"
```

---

## Task 6: 最终验证和文档

**Step 1: 运行所有测试**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo test skill_evolution::
```

**Step 2: 编译验证**

```bash
cd /Volumes/TBU4/Workspace/Aleph/core && cargo check
```

**Step 3: 更新设计文档**

修改 `/Volumes/TBU4/Workspace/Aleph/docs/plans/2026-01-31-aleph-beyond-openclaw-design.md`：

```markdown
### Milestone 5: Skill 进化系统

- [x] Memory 经验验证追踪 (EvolutionTracker - use_count, success_count)
- [x] 固化建议生成器 (SolidificationDetector - 阈值触发)
- [x] Skill 补丁生成 + diff 预览 (SkillGenerator)
- [x] Git 自动 Commit + Push (GitCommitter)

**验收**: ✅ 重复解决同一问题 3 次后，AI 主动建议固化
```

**Step 4: Final Commit**

```bash
git add docs/plans/
git commit -m "docs: mark Milestone 5 (skill evolution) as complete"
```

---

## 验收标准

完成本计划后，应满足以下条件：

1. ✅ `SkillExecution` 记录每次技能执行
2. ✅ `SkillMetrics` 聚合 use_count, success_count, success_rate
3. ✅ `EvolutionTracker` 持久化到 SQLite
4. ✅ `SolidificationDetector` 检测阈值触发建议
5. ✅ `SkillGenerator` 生成 SKILL.md 文件
6. ✅ `GitCommitter` 自动 commit 和可选 push
7. ✅ 配置支持 min_success_count=3, min_success_rate=0.8

---

## 依赖关系

```
Milestone 1 (PtySupervisor) ✅
    │
    └──► Milestone 5 (Skill 进化) ← 当前
              │
              └──► 可选：与 Memory 系统深度集成
```

---

*生成时间: 2026-01-31*

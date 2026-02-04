# Agent Execution Mode Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make Aleph behave as a true AI agent - directly executing tasks with plan/confirm flow instead of over-asking questions.

**Architecture:**
- IntentClassifier (3-level: regex → keywords → LLM) in `intent/` module
- DefaultsResolver (3-tier: preference → preset → inference) for smart defaults
- Agent Mode Prompt injection in `payload/assembler.rs`
- Cursor Agent UI flow using existing HaloState

**Tech Stack:** Rust (core), Swift (UI), UniFFI (bindings), regex crate

---

## Phase 1: IntentClassifier Module

### Task 1.1: Create Intent Module Structure

**Files:**
- Create: `Aether/core/src/intent/classifier.rs`
- Create: `Aether/core/src/intent/task_category.rs`
- Modify: `Aether/core/src/intent/mod.rs`

**Step 1: Write failing test for TaskCategory**

```rust
// In Aleph/core/src/intent/task_category.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_category_display() {
        assert_eq!(TaskCategory::FileOrganize.as_str(), "file_organize");
        assert_eq!(TaskCategory::FileTransfer.as_str(), "file_transfer");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd Aleph/core && cargo test task_category -v`
Expected: FAIL with "cannot find type TaskCategory"

**Step 3: Write TaskCategory implementation**

```rust
// In Aleph/core/src/intent/task_category.rs

/// Categories of executable tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskCategory {
    /// File organization (sort, classify)
    FileOrganize,
    /// File transfer (move, copy)
    FileTransfer,
    /// File cleanup (delete, archive)
    FileCleanup,
    /// Code execution
    CodeExecution,
    /// Application automation
    AppAutomation,
    /// Document generation
    DocumentGenerate,
    /// Data processing
    DataProcess,
}

impl TaskCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FileOrganize => "file_organize",
            Self::FileTransfer => "file_transfer",
            Self::FileCleanup => "file_cleanup",
            Self::CodeExecution => "code_execution",
            Self::AppAutomation => "app_automation",
            Self::DocumentGenerate => "document_generate",
            Self::DataProcess => "data_process",
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd Aleph/core && cargo test task_category -v`
Expected: PASS

**Step 5: Commit**

```bash
git add Aleph/core/src/intent/task_category.rs
git commit -m "feat(intent): add TaskCategory enum for executable task classification"
```

---

### Task 1.2: Create ExecutionIntent Enum

**Files:**
- Modify: `Aether/core/src/intent/classifier.rs`

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_intent_is_executable() {
        let task = ExecutableTask {
            category: TaskCategory::FileOrganize,
            action: "整理文件".to_string(),
            target: Some("/Downloads".to_string()),
            confidence: 0.95,
        };
        let intent = ExecutionIntent::Executable(task);
        assert!(intent.is_executable());
    }

    #[test]
    fn test_execution_intent_conversational() {
        let intent = ExecutionIntent::Conversational;
        assert!(!intent.is_executable());
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cd Aleph/core && cargo test execution_intent -v`
Expected: FAIL

**Step 3: Write ExecutionIntent implementation**

```rust
// In Aleph/core/src/intent/classifier.rs

use super::task_category::TaskCategory;

/// Result of intent classification
#[derive(Debug, Clone)]
pub enum ExecutionIntent {
    /// Directly executable task - trigger Agent mode
    Executable(ExecutableTask),
    /// Needs clarification - ask ONE question max
    Ambiguous {
        task_hint: String,
        clarification: String,
    },
    /// Pure conversation - normal chat flow
    Conversational,
}

/// An executable task with metadata
#[derive(Debug, Clone)]
pub struct ExecutableTask {
    /// Task category
    pub category: TaskCategory,
    /// Action description extracted from input
    pub action: String,
    /// Target path or object (if detected)
    pub target: Option<String>,
    /// Classification confidence (0.0-1.0)
    pub confidence: f32,
}

impl ExecutionIntent {
    pub fn is_executable(&self) -> bool {
        matches!(self, Self::Executable(_))
    }

    pub fn is_ambiguous(&self) -> bool {
        matches!(self, Self::Ambiguous { .. })
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd Aleph/core && cargo test execution_intent -v`
Expected: PASS

**Step 5: Commit**

```bash
git add Aleph/core/src/intent/classifier.rs
git commit -m "feat(intent): add ExecutionIntent enum and ExecutableTask struct"
```

---

### Task 1.3: Implement L1 Regex Pattern Matching

**Files:**
- Modify: `Aether/core/src/intent/classifier.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_l1_regex_file_organize() {
    let classifier = IntentClassifier::new();
    let result = classifier.match_regex("帮我整理一下这个文件夹里的文件");
    assert!(result.is_some());
    let task = result.unwrap();
    assert_eq!(task.category, TaskCategory::FileOrganize);
}

#[test]
fn test_l1_regex_file_transfer() {
    let classifier = IntentClassifier::new();
    let result = classifier.match_regex("把这些文件移动到Documents目录");
    assert!(result.is_some());
    let task = result.unwrap();
    assert_eq!(task.category, TaskCategory::FileTransfer);
}

#[test]
fn test_l1_regex_no_match() {
    let classifier = IntentClassifier::new();
    let result = classifier.match_regex("今天天气怎么样");
    assert!(result.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cd Aleph/core && cargo test l1_regex -v`
Expected: FAIL with "cannot find value IntentClassifier"

**Step 3: Write L1 regex implementation**

```rust
use regex::Regex;
use once_cell::sync::Lazy;

/// Regex patterns for L1 classification (Chinese + English)
static EXECUTABLE_PATTERNS: Lazy<Vec<(Regex, TaskCategory)>> = Lazy::new(|| {
    vec![
        // FileOrganize: 整理/归类/分类 + 文件
        (Regex::new(r"(?i)(整理|归类|分类|organize|sort|classify).*文件|files?").unwrap(), TaskCategory::FileOrganize),
        // FileTransfer: 移动/复制/拷贝 + 到
        (Regex::new(r"(?i)(移动|复制|拷贝|转移|move|copy|transfer).*到|to").unwrap(), TaskCategory::FileTransfer),
        // FileCleanup: 删除/清理/清空
        (Regex::new(r"(?i)(删除|清理|清空|清除|delete|remove|clean)").unwrap(), TaskCategory::FileCleanup),
        // CodeExecution: 运行/执行 + 脚本/代码
        (Regex::new(r"(?i)(运行|执行|跑一下|run|execute).*(?:脚本|代码|script|code)").unwrap(), TaskCategory::CodeExecution),
        // DocumentGenerate: 生成/创建/导出 + 文档/报告
        (Regex::new(r"(?i)(生成|创建|导出|写|generate|create|export).*(?:文档|报告|document|report)").unwrap(), TaskCategory::DocumentGenerate),
    ]
});

/// Path extraction pattern
static PATH_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"['"]?([/~][^\s'"]+|[A-Za-z]:\\[^\s'"]+)['"]?"#).unwrap()
});

/// Intent classifier with 3-level classification
pub struct IntentClassifier {
    confidence_threshold: f32,
}

impl IntentClassifier {
    pub fn new() -> Self {
        Self {
            confidence_threshold: 0.7,
        }
    }

    /// L1: Regex pattern matching (<5ms)
    pub fn match_regex(&self, input: &str) -> Option<ExecutableTask> {
        for (pattern, category) in EXECUTABLE_PATTERNS.iter() {
            if pattern.is_match(input) {
                let target = self.extract_path(input);
                return Some(ExecutableTask {
                    category: *category,
                    action: input.to_string(),
                    target,
                    confidence: 1.0, // Regex match = high confidence
                });
            }
        }
        None
    }

    /// Extract file path from input
    fn extract_path(&self, input: &str) -> Option<String> {
        PATH_PATTERN.captures(input).map(|c| c[1].to_string())
    }
}

impl Default for IntentClassifier {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd Aleph/core && cargo test l1_regex -v`
Expected: PASS

**Step 5: Commit**

```bash
git add Aleph/core/src/intent/classifier.rs
git commit -m "feat(intent): implement L1 regex pattern matching for IntentClassifier"
```

---

### Task 1.4: Implement L2 Keyword Matching

**Files:**
- Modify: `Aether/core/src/intent/classifier.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_l2_keywords_file_organize() {
    let classifier = IntentClassifier::new();
    // This input doesn't match L1 regex exactly but has keywords
    let result = classifier.match_keywords("能不能帮忙把下载里的东西按类型分一下");
    assert!(result.is_some());
    let task = result.unwrap();
    assert_eq!(task.category, TaskCategory::FileOrganize);
    assert!(task.confidence < 1.0); // Lower confidence than regex
}

#[test]
fn test_l2_keywords_no_match() {
    let classifier = IntentClassifier::new();
    let result = classifier.match_keywords("你好，请问你是谁");
    assert!(result.is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cd Aleph/core && cargo test l2_keywords -v`
Expected: FAIL with "no method named match_keywords"

**Step 3: Write L2 keyword implementation**

```rust
/// Keyword sets for L2 classification
struct KeywordSet {
    verbs: &'static [&'static str],
    nouns: &'static [&'static str],
    category: TaskCategory,
}

static KEYWORD_SETS: &[KeywordSet] = &[
    KeywordSet {
        verbs: &["整理", "归类", "分类", "分", "organize", "sort", "classify"],
        nouns: &["文件", "文件夹", "目录", "下载", "files", "folder", "directory", "downloads"],
        category: TaskCategory::FileOrganize,
    },
    KeywordSet {
        verbs: &["移动", "复制", "拷贝", "转移", "move", "copy", "transfer"],
        nouns: &["文件", "文件夹", "到", "files", "folder", "to"],
        category: TaskCategory::FileTransfer,
    },
    KeywordSet {
        verbs: &["删除", "清理", "清空", "移除", "delete", "remove", "clean", "clear"],
        nouns: &["文件", "缓存", "垃圾", "files", "cache", "trash"],
        category: TaskCategory::FileCleanup,
    },
];

impl IntentClassifier {
    /// L2: Keyword + rule matching (<20ms)
    pub fn match_keywords(&self, input: &str) -> Option<ExecutableTask> {
        let input_lower = input.to_lowercase();

        for set in KEYWORD_SETS {
            let has_verb = set.verbs.iter().any(|v| input_lower.contains(v));
            let has_noun = set.nouns.iter().any(|n| input_lower.contains(n));

            if has_verb && has_noun {
                let target = self.extract_path(input);
                return Some(ExecutableTask {
                    category: set.category,
                    action: input.to_string(),
                    target,
                    confidence: 0.85, // Keyword match = good confidence
                });
            }
        }
        None
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd Aleph/core && cargo test l2_keywords -v`
Expected: PASS

**Step 5: Commit**

```bash
git add Aleph/core/src/intent/classifier.rs
git commit -m "feat(intent): implement L2 keyword matching for IntentClassifier"
```

---

### Task 1.5: Implement Async classify() Method

**Files:**
- Modify: `Aether/core/src/intent/classifier.rs`

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn test_classify_executable() {
    let classifier = IntentClassifier::new();
    let result = classifier.classify("帮我整理一下/Downloads/文件夹里的文件").await;
    assert!(matches!(result, ExecutionIntent::Executable(_)));
}

#[tokio::test]
async fn test_classify_conversational() {
    let classifier = IntentClassifier::new();
    let result = classifier.classify("你好").await;
    assert!(matches!(result, ExecutionIntent::Conversational));
}
```

**Step 2: Run test to verify it fails**

Run: `cd Aleph/core && cargo test test_classify -v`
Expected: FAIL

**Step 3: Write classify implementation**

```rust
impl IntentClassifier {
    /// Main classification entry point
    /// Tries L1 → L2 → L3 in order, returns first match
    pub async fn classify(&self, input: &str) -> ExecutionIntent {
        // Skip very short inputs
        if input.trim().len() < 3 {
            return ExecutionIntent::Conversational;
        }

        // L1: Regex matching (<5ms)
        if let Some(task) = self.match_regex(input) {
            return ExecutionIntent::Executable(task);
        }

        // L2: Keyword matching (<20ms)
        if let Some(task) = self.match_keywords(input) {
            return ExecutionIntent::Executable(task);
        }

        // L3: LLM classification (future - for now return Conversational)
        // TODO: Implement LLM-based classification when needed
        ExecutionIntent::Conversational
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cd Aleph/core && cargo test test_classify -v`
Expected: PASS

**Step 5: Commit**

```bash
git add Aleph/core/src/intent/classifier.rs
git commit -m "feat(intent): implement async classify() method with L1/L2 fallback"
```

---

### Task 1.6: Update Module Exports

**Files:**
- Modify: `Aether/core/src/intent/mod.rs`
- Modify: `Aether/core/src/lib.rs`

**Step 1: Update intent/mod.rs**

```rust
//! Intent detection module for AI-powered conversation flow.
//!
//! This module provides:
//! - **AiIntentDetector**: AI-powered detection for capability invocation
//! - **IntentClassifier**: Task classification for Agent execution mode

pub mod ai_detector;
pub mod classifier;
pub mod task_category;

pub use ai_detector::{AiIntentDetector, AiIntentResult};
pub use classifier::{ExecutableTask, ExecutionIntent, IntentClassifier};
pub use task_category::TaskCategory;
```

**Step 2: Update lib.rs exports**

Add to the existing exports:
```rust
pub use crate::intent::{ExecutableTask, ExecutionIntent, IntentClassifier, TaskCategory};
```

**Step 3: Run full test suite**

Run: `cd Aleph/core && cargo test intent -v`
Expected: All intent tests PASS

**Step 4: Commit**

```bash
git add Aleph/core/src/intent/mod.rs Aleph/core/src/lib.rs
git commit -m "feat(intent): export IntentClassifier and related types"
```

---

## Phase 2: DefaultsResolver Module

### Task 2.1: Create Task Parameters Type

**Files:**
- Create: `Aether/core/src/intent/parameters.rs`

**Step 1: Write failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_parameters_default() {
        let params = TaskParameters::default();
        assert_eq!(params.organize_method, OrganizeMethod::ByExtension);
        assert_eq!(params.conflict_resolution, ConflictResolution::Rename);
    }
}
```

**Step 2: Run test to verify it fails**

**Step 3: Write implementation**

```rust
// In Aleph/core/src/intent/parameters.rs

/// How to organize files
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OrganizeMethod {
    #[default]
    ByExtension,
    ByCategory,
    ByDate,
}

/// How to handle file conflicts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictResolution {
    Skip,
    #[default]
    Rename,
    Overwrite,
}

/// Source of parameters
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterSource {
    UserPreference,
    Preset,
    Inference,
    Default,
}

/// Parameters for task execution
#[derive(Debug, Clone)]
pub struct TaskParameters {
    pub organize_method: OrganizeMethod,
    pub conflict_resolution: ConflictResolution,
    pub source: ParameterSource,
}

impl Default for TaskParameters {
    fn default() -> Self {
        Self {
            organize_method: OrganizeMethod::ByExtension,
            conflict_resolution: ConflictResolution::Rename,
            source: ParameterSource::Default,
        }
    }
}

impl TaskParameters {
    pub fn file_organize_by_extension() -> Self {
        Self {
            organize_method: OrganizeMethod::ByExtension,
            ..Default::default()
        }
    }

    pub fn file_organize_by_category() -> Self {
        Self {
            organize_method: OrganizeMethod::ByCategory,
            ..Default::default()
        }
    }

    pub fn file_organize_by_date() -> Self {
        Self {
            organize_method: OrganizeMethod::ByDate,
            ..Default::default()
        }
    }

    pub fn with_source(mut self, source: ParameterSource) -> Self {
        self.source = source;
        self
    }
}
```

**Step 4: Run test to verify it passes**

**Step 5: Commit**

```bash
git add Aleph/core/src/intent/parameters.rs
git commit -m "feat(intent): add TaskParameters for execution defaults"
```

---

### Task 2.2: Create Preset Registry

**Files:**
- Create: `Aether/core/src/intent/presets.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_preset_match_file_organize() {
    let registry = PresetRegistry::default();
    let task = ExecutableTask {
        category: TaskCategory::FileOrganize,
        action: "整理文件".to_string(),
        target: None,
        confidence: 0.9,
    };
    let preset = registry.match_scenario(&task);
    assert!(preset.is_some());
    assert_eq!(preset.unwrap().parameters.organize_method, OrganizeMethod::ByExtension);
}
```

**Step 2: Run test to verify it fails**

**Step 3: Write implementation**

```rust
// In Aleph/core/src/intent/presets.rs

use super::parameters::{OrganizeMethod, ParameterSource, TaskParameters};
use super::task_category::TaskCategory;
use super::classifier::ExecutableTask;

/// A preset scenario with default parameters
#[derive(Debug, Clone)]
pub struct ScenarioPreset {
    pub keywords: Vec<String>,
    pub category: TaskCategory,
    pub parameters: TaskParameters,
}

/// Registry of preset scenarios
pub struct PresetRegistry {
    presets: Vec<ScenarioPreset>,
}

impl Default for PresetRegistry {
    fn default() -> Self {
        Self {
            presets: vec![
                // 整理文件 → 按扩展名分组
                ScenarioPreset {
                    keywords: vec!["整理".to_string(), "organize".to_string(), "sort".to_string()],
                    category: TaskCategory::FileOrganize,
                    parameters: TaskParameters::file_organize_by_extension()
                        .with_source(ParameterSource::Preset),
                },
                // 清理下载 → 按大类分组
                ScenarioPreset {
                    keywords: vec!["清理下载".to_string(), "clean downloads".to_string()],
                    category: TaskCategory::FileOrganize,
                    parameters: TaskParameters::file_organize_by_category()
                        .with_source(ParameterSource::Preset),
                },
            ],
        }
    }
}

impl PresetRegistry {
    pub fn match_scenario(&self, task: &ExecutableTask) -> Option<&ScenarioPreset> {
        let action_lower = task.action.to_lowercase();
        self.presets.iter().find(|p| {
            p.category == task.category
                && p.keywords.iter().any(|k| action_lower.contains(&k.to_lowercase()))
        })
    }
}
```

**Step 4: Run test to verify it passes**

**Step 5: Commit**

```bash
git add Aleph/core/src/intent/presets.rs
git commit -m "feat(intent): add PresetRegistry for default scenarios"
```

---

### Task 2.3: Create DefaultsResolver

**Files:**
- Create: `Aether/core/src/intent/defaults.rs`

**Step 1: Write failing test**

```rust
#[tokio::test]
async fn test_defaults_resolver_preset() {
    let resolver = DefaultsResolver::new();
    let task = ExecutableTask {
        category: TaskCategory::FileOrganize,
        action: "整理文件".to_string(),
        target: Some("/tmp/test".to_string()),
        confidence: 0.9,
    };
    let params = resolver.resolve(&task).await;
    assert_eq!(params.source, ParameterSource::Preset);
}
```

**Step 2: Run test to verify it fails**

**Step 3: Write implementation**

```rust
// In Aleph/core/src/intent/defaults.rs

use super::classifier::ExecutableTask;
use super::parameters::{ParameterSource, TaskParameters};
use super::presets::PresetRegistry;

/// Resolves default parameters for executable tasks
///
/// Three-tier resolution:
/// 1. User preferences (stored in config)
/// 2. Preset scenarios (hardcoded defaults)
/// 3. Context inference (based on file analysis)
pub struct DefaultsResolver {
    presets: PresetRegistry,
    // preferences: PreferenceStore, // TODO: Implement in future
}

impl DefaultsResolver {
    pub fn new() -> Self {
        Self {
            presets: PresetRegistry::default(),
        }
    }

    /// Resolve parameters using 3-tier strategy
    pub async fn resolve(&self, task: &ExecutableTask) -> TaskParameters {
        // Tier 1: Check user preferences (TODO: implement PreferenceStore)
        // if let Some(params) = self.preferences.get_for_task(&task.category, &task.target) {
        //     return params;
        // }

        // Tier 2: Match preset scenario
        if let Some(preset) = self.presets.match_scenario(task) {
            return preset.parameters.clone();
        }

        // Tier 3: Context inference (simplified for now)
        self.infer_from_context(task).await
    }

    /// Infer parameters from context (simplified implementation)
    async fn infer_from_context(&self, _task: &ExecutableTask) -> TaskParameters {
        // TODO: Implement file scanning and inference
        // For now, return defaults
        TaskParameters::default().with_source(ParameterSource::Inference)
    }
}

impl Default for DefaultsResolver {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 4: Run test to verify it passes**

**Step 5: Commit**

```bash
git add Aleph/core/src/intent/defaults.rs
git commit -m "feat(intent): add DefaultsResolver with 3-tier strategy"
```

---

### Task 2.4: Export DefaultsResolver

**Files:**
- Modify: `Aether/core/src/intent/mod.rs`

**Step 1: Update module exports**

```rust
pub mod ai_detector;
pub mod classifier;
pub mod defaults;
pub mod parameters;
pub mod presets;
pub mod task_category;

pub use ai_detector::{AiIntentDetector, AiIntentResult};
pub use classifier::{ExecutableTask, ExecutionIntent, IntentClassifier};
pub use defaults::DefaultsResolver;
pub use parameters::{ConflictResolution, OrganizeMethod, ParameterSource, TaskParameters};
pub use presets::{PresetRegistry, ScenarioPreset};
pub use task_category::TaskCategory;
```

**Step 2: Run tests**

Run: `cd Aleph/core && cargo test intent -v`
Expected: All PASS

**Step 3: Commit**

```bash
git add Aleph/core/src/intent/mod.rs
git commit -m "feat(intent): export DefaultsResolver and parameter types"
```

---

## Phase 3: Agent Mode Prompt Injection

### Task 3.1: Create Agent Mode Prompt Template

**Files:**
- Create: `Aether/core/src/intent/agent_prompt.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_agent_prompt_generation() {
    let prompt = AgentModePrompt::new();
    let text = prompt.generate();
    assert!(text.contains("Agent执行模式"));
    assert!(text.contains("禁止询问选项"));
}
```

**Step 2: Run test to verify it fails**

**Step 3: Write implementation**

```rust
// In Aleph/core/src/intent/agent_prompt.rs

/// Agent mode prompt template
pub struct AgentModePrompt;

impl AgentModePrompt {
    pub fn new() -> Self {
        Self
    }

    /// Generate the agent mode prompt block
    pub fn generate(&self) -> String {
        r#"## Agent执行模式

你已进入Agent执行模式。当前任务已被识别为可执行任务。

### 行为规则（必须遵守）

1. **禁止询问选项** - 不要列出A/B/C选项让用户选择
2. **立即制定计划** - 分解任务为具体步骤
3. **展示计划摘要** - 用简洁格式展示将要执行的操作
4. **等待确认** - 写入/移动/删除操作必须等用户确认
5. **执行并报告** - 确认后执行，实时反馈进度

### 输出格式

当你识别到可执行任务时，必须按以下格式输出：

```json
{
  "__agent_plan__": true,
  "title": "任务标题（一句话描述）",
  "operations": [
    {"action": "create_folder", "target": "PDF"},
    {"action": "move_file", "source": "report.pdf", "target": "PDF/"}
  ],
  "summary": {
    "files_affected": 23,
    "folders_to_create": 5
  }
}
```

### 确认边界（保守模式）

| 操作类型 | 是否需要确认 |
|---------|-------------|
| 扫描/分析/预览 | ❌ 自动执行 |
| 创建文件夹 | ✅ 需要确认 |
| 移动/复制文件 | ✅ 需要确认 |
| 重命名 | ✅ 需要确认 |
| 删除 | ✅ 需要确认 |
| 覆盖已有文件 | ✅ 单独确认 |

**CRITICAL**: 不要询问用户选择方案。直接展示你推断的最佳方案，让用户确认或取消。"#.to_string()
    }
}

impl Default for AgentModePrompt {
    fn default() -> Self {
        Self::new()
    }
}
```

**Step 4: Run test to verify it passes**

**Step 5: Commit**

```bash
git add Aleph/core/src/intent/agent_prompt.rs
git commit -m "feat(intent): add AgentModePrompt template"
```

---

### Task 3.2: Integrate Agent Prompt into PromptAssembler

**Files:**
- Modify: `Aether/core/src/payload/assembler.rs`

**Step 1: Write failing test**

```rust
#[test]
fn test_build_prompt_with_agent_mode() {
    let assembler = PromptAssembler::new(ContextFormat::Markdown);
    let intent = ExecutionIntent::Executable(ExecutableTask {
        category: TaskCategory::FileOrganize,
        action: "整理文件".to_string(),
        target: None,
        confidence: 0.9,
    });
    let prompt = assembler.build_prompt_with_intent("Base prompt.", &[], None, Some(&intent));
    assert!(prompt.contains("Agent执行模式"));
}
```

**Step 2: Run test to verify it fails**

**Step 3: Add method to PromptAssembler**

```rust
// Add to PromptAssembler impl block

use crate::intent::{AgentModePrompt, ExecutionIntent};

/// Build prompt with agent mode injection based on intent
pub fn build_prompt_with_intent(
    &self,
    base_prompt: &str,
    capabilities: &[CapabilityDeclaration],
    context: Option<&AgentContext>,
    intent: Option<&ExecutionIntent>,
) -> String {
    let mut prompt = self.build_capability_aware_prompt(base_prompt, capabilities, context);

    // Inject agent mode prompt if intent is executable
    if let Some(ExecutionIntent::Executable(_)) = intent {
        let agent_prompt = AgentModePrompt::new().generate();
        prompt.push_str("\n\n");
        prompt.push_str(&agent_prompt);
    }

    prompt
}
```

**Step 4: Run test to verify it passes**

**Step 5: Commit**

```bash
git add Aleph/core/src/payload/assembler.rs
git commit -m "feat(payload): add agent mode prompt injection based on intent"
```

---

## Phase 4: UniFFI Bindings

### Task 4.1: Add UniFFI Types for Intent

**Files:**
- Modify: `Aether/core/src/aether.udl`
- Create: `Aether/core/src/intent_ffi.rs`

**Step 1: Add types to aether.udl**

```
// Add to aether.udl

enum TaskCategoryFfi {
    "FileOrganize",
    "FileTransfer",
    "FileCleanup",
    "CodeExecution",
    "AppAutomation",
    "DocumentGenerate",
    "DataProcess",
};

dictionary ExecutableTaskFfi {
    TaskCategoryFfi category;
    string action;
    string? target;
    f32 confidence;
};

[Enum]
interface ExecutionIntentFfi {
    Executable(ExecutableTaskFfi task);
    Ambiguous(string task_hint, string clarification);
    Conversational();
};
```

**Step 2: Create FFI conversion module**

```rust
// In Aleph/core/src/intent_ffi.rs

use crate::intent::{ExecutableTask, ExecutionIntent, TaskCategory};

#[derive(Debug, Clone, uniffi::Enum)]
pub enum TaskCategoryFfi {
    FileOrganize,
    FileTransfer,
    FileCleanup,
    CodeExecution,
    AppAutomation,
    DocumentGenerate,
    DataProcess,
}

impl From<TaskCategory> for TaskCategoryFfi {
    fn from(cat: TaskCategory) -> Self {
        match cat {
            TaskCategory::FileOrganize => Self::FileOrganize,
            TaskCategory::FileTransfer => Self::FileTransfer,
            TaskCategory::FileCleanup => Self::FileCleanup,
            TaskCategory::CodeExecution => Self::CodeExecution,
            TaskCategory::AppAutomation => Self::AppAutomation,
            TaskCategory::DocumentGenerate => Self::DocumentGenerate,
            TaskCategory::DataProcess => Self::DataProcess,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ExecutableTaskFfi {
    pub category: TaskCategoryFfi,
    pub action: String,
    pub target: Option<String>,
    pub confidence: f32,
}

impl From<ExecutableTask> for ExecutableTaskFfi {
    fn from(task: ExecutableTask) -> Self {
        Self {
            category: task.category.into(),
            action: task.action,
            target: task.target,
            confidence: task.confidence,
        }
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum ExecutionIntentFfi {
    Executable { task: ExecutableTaskFfi },
    Ambiguous { task_hint: String, clarification: String },
    Conversational,
}

impl From<ExecutionIntent> for ExecutionIntentFfi {
    fn from(intent: ExecutionIntent) -> Self {
        match intent {
            ExecutionIntent::Executable(task) => Self::Executable { task: task.into() },
            ExecutionIntent::Ambiguous { task_hint, clarification } => {
                Self::Ambiguous { task_hint, clarification }
            }
            ExecutionIntent::Conversational => Self::Conversational,
        }
    }
}
```

**Step 3: Export in lib.rs**

**Step 4: Run UniFFI bindgen**

Run: `cd Aleph/core && cargo build`
Expected: BUILD SUCCESS

**Step 5: Commit**

```bash
git add Aleph/core/src/intent_ffi.rs Aleph/core/src/aether.udl Aleph/core/src/lib.rs
git commit -m "feat(ffi): add UniFFI bindings for intent classification types"
```

---

## Phase 5: Swift UI Integration

### Task 5.1: Add Agent HaloState Cases

**Files:**
- Modify: `Aether/Sources/HaloState.swift`

**Step 1: Add new state cases**

```swift
// Add to HaloState enum

/// Agent plan confirmation (Cursor-style)
case agentPlan(
    planId: String,
    title: String,
    operations: [AgentOperation],
    summary: AgentPlanSummary
)

/// Agent execution progress
case agentProgress(
    planId: String,
    progress: Float,
    currentOperation: String,
    completedCount: Int,
    totalCount: Int
)

/// Agent conflict resolution
case agentConflict(
    planId: String,
    fileName: String,
    targetPath: String,
    applyToAll: Bool
)
```

**Step 2: Add supporting types**

```swift
/// Single operation in agent plan
struct AgentOperation: Equatable {
    let action: String
    let source: String?
    let target: String
}

/// Summary of agent plan
struct AgentPlanSummary: Equatable {
    let filesAffected: Int
    let foldersToCreate: Int
}
```

**Step 3: Update Equatable conformance**

**Step 4: Commit**

```bash
git add Aleph/Sources/HaloState.swift
git commit -m "feat(ui): add agent execution states to HaloState"
```

---

### Task 5.2: Create AgentPlanView Component

**Files:**
- Create: `Aether/Sources/Components/AgentPlanView.swift`

**Step 1: Create the view**

```swift
import SwiftUI

/// Agent plan confirmation view (Cursor-style)
struct AgentPlanView: View {
    let planId: String
    let title: String
    let operations: [AgentOperation]
    let summary: AgentPlanSummary
    let onExecute: () -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            // Header
            HStack {
                Image(systemName: "list.clipboard")
                    .font(.title2)
                Text(title)
                    .font(.headline)
            }

            // Operations list
            VStack(alignment: .leading, spacing: 8) {
                Text(L("agent.plan.operations"))
                    .font(.subheadline)
                    .foregroundColor(.secondary)

                ForEach(operations.prefix(5), id: \.target) { op in
                    HStack {
                        Image(systemName: iconForAction(op.action))
                            .foregroundColor(.blue)
                        Text(op.target)
                            .lineLimit(1)
                    }
                    .font(.caption)
                }

                if operations.count > 5 {
                    Text(L("agent.plan.more_operations", operations.count - 5))
                        .font(.caption)
                        .foregroundColor(.secondary)
                }
            }

            // Summary
            HStack {
                Label("\(summary.filesAffected) \(L("agent.plan.files"))", systemImage: "doc")
                Label("\(summary.foldersToCreate) \(L("agent.plan.folders"))", systemImage: "folder")
            }
            .font(.caption)
            .foregroundColor(.secondary)

            // Actions
            HStack {
                Button(L("common.cancel"), action: onCancel)
                    .buttonStyle(.bordered)

                Button(L("common.execute"), action: onExecute)
                    .buttonStyle(.borderedProminent)
            }
        }
        .padding()
        .background(.ultraThinMaterial)
        .cornerRadius(12)
    }

    private func iconForAction(_ action: String) -> String {
        switch action {
        case "create_folder": return "folder.badge.plus"
        case "move_file": return "arrow.right.doc"
        case "copy_file": return "doc.on.doc"
        case "delete_file": return "trash"
        default: return "gearshape"
        }
    }
}
```

**Step 2: Commit**

```bash
git add Aleph/Sources/Components/AgentPlanView.swift
git commit -m "feat(ui): add AgentPlanView component for plan confirmation"
```

---

### Task 5.3: Create AgentProgressView Component

**Files:**
- Create: `Aether/Sources/Components/AgentProgressView.swift`

**Step 1: Create the view**

```swift
import SwiftUI

/// Agent execution progress view
struct AgentProgressView: View {
    let planId: String
    let progress: Float
    let currentOperation: String
    let completedCount: Int
    let totalCount: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Progress bar
            ProgressView(value: Double(progress))
                .progressViewStyle(.linear)

            // Status
            HStack {
                Image(systemName: "arrow.right.circle")
                    .foregroundColor(.blue)
                Text(currentOperation)
                    .lineLimit(1)
            }
            .font(.caption)

            // Counter
            Text("\(completedCount)/\(totalCount)")
                .font(.caption)
                .foregroundColor(.secondary)
        }
        .padding()
        .background(.ultraThinMaterial)
        .cornerRadius(12)
    }
}
```

**Step 2: Commit**

```bash
git add Aleph/Sources/Components/AgentProgressView.swift
git commit -m "feat(ui): add AgentProgressView component for execution progress"
```

---

## Phase 6: Integration

### Task 6.1: Wire IntentClassifier into Processing Flow

**Files:**
- Modify: `Aether/core/src/ffi/processing.rs` (or equivalent)

**Step 1: Add intent classification before AI call**

```rust
// In the main processing flow

use crate::intent::IntentClassifier;

// Before calling AI provider
let classifier = IntentClassifier::new();
let intent = classifier.classify(&user_input).await;

// Build prompt with intent
let prompt = assembler.build_prompt_with_intent(
    base_prompt,
    &capabilities,
    context.as_ref(),
    Some(&intent),
);

// If executable, pass intent to UI for agent mode handling
if let ExecutionIntent::Executable(task) = &intent {
    // Notify Swift layer about agent mode
    event_handler.on_agent_mode_detected(task.into());
}
```

**Step 2: Add event handler method**

Add to AlephEventHandler trait:
```rust
fn on_agent_mode_detected(&self, task: ExecutableTaskFfi);
```

**Step 3: Commit**

```bash
git commit -m "feat: integrate IntentClassifier into main processing flow"
```

---

### Task 6.2: Final Integration Test

**Files:**
- Create: `Aether/core/src/tests/intent_integration.rs`

**Step 1: Write integration test**

```rust
#[tokio::test]
async fn test_full_intent_classification_flow() {
    let classifier = IntentClassifier::new();
    let resolver = DefaultsResolver::new();

    // Test file organize scenario
    let intent = classifier.classify("帮我整理/Downloads/test文件夹里的文件").await;

    if let ExecutionIntent::Executable(task) = intent {
        assert_eq!(task.category, TaskCategory::FileOrganize);
        assert!(task.target.is_some());

        let params = resolver.resolve(&task).await;
        assert_eq!(params.organize_method, OrganizeMethod::ByExtension);
    } else {
        panic!("Expected Executable intent");
    }
}
```

**Step 2: Run all tests**

Run: `cd Aleph/core && cargo test`
Expected: All PASS

**Step 3: Final commit**

```bash
git add .
git commit -m "feat: complete Agent Execution Mode implementation"
```

---

## Summary

| Phase | Tasks | Key Deliverables |
|-------|-------|------------------|
| 1 | 1.1-1.6 | IntentClassifier with L1/L2 matching |
| 2 | 2.1-2.4 | DefaultsResolver with 3-tier strategy |
| 3 | 3.1-3.2 | Agent Mode Prompt injection |
| 4 | 4.1 | UniFFI bindings |
| 5 | 5.1-5.3 | Swift UI components |
| 6 | 6.1-6.2 | Integration and testing |

**Total: 15 tasks, ~90 commits expected**

---

Plan complete and saved to `docs/plans/2026-01-16-agent-execution-mode-impl.md`. Two execution options:

**1. Subagent-Driven (this session)** - I dispatch fresh subagent per task, review between tasks, fast iteration

**2. Parallel Session (separate)** - Open new session with executing-plans, batch execution with checkpoints

Which approach?

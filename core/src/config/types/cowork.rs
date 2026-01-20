//! Cowork configuration types
//!
//! Contains Cowork task orchestration configuration:
//! - CoworkConfigToml: Main configuration for the Cowork engine
//! - FileOpsConfigToml: File operations executor configuration
//! - ModelProfileConfigToml: AI model profile configuration
//! - ModelRoutingConfigToml: Multi-model routing configuration
//!
//! Cowork is a multi-task orchestration system that decomposes complex requests
//! into DAG-structured task graphs and executes them with parallel scheduling.

use crate::dispatcher::model_router::{
    Capability, CircuitBreakerConfig, CostStrategy, CostTier, HealthConfig, LatencyTier,
    ModelProfile, ModelRoutingRules, ProbeConfig, ScoringConfig, WindowConfig,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// =============================================================================
// CoworkConfigToml
// =============================================================================

/// Cowork task orchestration configuration
///
/// Configures the Cowork engine for multi-task orchestration.
/// This includes task decomposition, parallel execution, and confirmation settings.
///
/// # Example TOML
/// ```toml
/// [cowork]
/// enabled = true
/// require_confirmation = true
/// max_parallelism = 4
/// dry_run = false
/// planner_model = "claude"
/// auto_execute_threshold = 0.9
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoworkConfigToml {
    /// Enable Cowork task orchestration
    #[serde(default = "default_cowork_enabled")]
    pub enabled: bool,

    /// Require user confirmation before executing task graphs
    /// When true, shows confirmation UI with task list before execution
    #[serde(default = "default_require_confirmation")]
    pub require_confirmation: bool,

    /// Maximum number of tasks to run in parallel
    /// Higher values improve throughput but increase resource usage
    #[serde(default = "default_max_parallelism")]
    pub max_parallelism: usize,

    /// Enable dry-run mode (plan tasks but don't execute)
    /// Useful for testing and debugging task graphs
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,

    /// AI provider to use for task planning (LLM decomposition)
    /// If not specified, uses the default provider from [general]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_provider: Option<String>,

    /// Confidence threshold for auto-execution without confirmation
    /// Tasks with confidence >= threshold may bypass confirmation
    /// Range: 0.0 - 1.0 (0.0 = always confirm, 1.0 = never auto-execute)
    #[serde(default = "default_auto_execute_threshold")]
    pub auto_execute_threshold: f32,

    /// Maximum number of tasks allowed in a single graph
    /// Prevents runaway task decomposition
    #[serde(default = "default_max_tasks_per_graph")]
    pub max_tasks_per_graph: usize,

    /// Timeout for individual task execution (seconds)
    /// 0 = no timeout
    #[serde(default = "default_task_timeout_seconds")]
    pub task_timeout_seconds: u64,

    /// Enable sandboxed execution for code tasks
    /// When true, code execution tasks run in isolated environment
    #[serde(default = "default_sandbox_enabled")]
    pub sandbox_enabled: bool,

    /// Categories of tasks that are allowed
    /// Empty list = all categories allowed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_categories: Vec<String>,

    /// Categories of tasks that are blocked
    /// Takes precedence over allowed_categories
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_categories: Vec<String>,

    /// File operations configuration
    #[serde(default)]
    pub file_ops: FileOpsConfigToml,

    /// Code execution configuration
    #[serde(default)]
    pub code_exec: CodeExecConfigToml,

    /// Model profiles configuration
    /// Maps profile ID to profile configuration
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub model_profiles: HashMap<String, ModelProfileConfigToml>,

    /// Model routing configuration
    #[serde(default)]
    pub model_routing: ModelRoutingConfigToml,
}

// =============================================================================
// FileOpsConfigToml
// =============================================================================

/// File operations executor configuration
///
/// Configures permissions and behavior for file system operations.
/// Uses path-based access control with allowed/denied lists.
///
/// # Example TOML
/// ```toml
/// [cowork.file_ops]
/// enabled = true
/// allowed_paths = ["~/Downloads", "~/Documents"]
/// denied_paths = ["~/.ssh", "~/.gnupg"]
/// max_file_size = "100MB"
/// require_confirmation_for_write = true
/// require_confirmation_for_delete = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOpsConfigToml {
    /// Enable file operations executor
    #[serde(default = "default_file_ops_enabled")]
    pub enabled: bool,

    /// Paths that are allowed for file operations (glob patterns)
    /// Empty list = all paths allowed (except denied)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_paths: Vec<String>,

    /// Paths that are denied for file operations (glob patterns)
    /// Takes precedence over allowed_paths
    /// Default denied paths (~/.ssh, ~/.gnupg, etc.) are always applied
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied_paths: Vec<String>,

    /// Maximum file size in bytes for read operations
    /// 0 = unlimited
    /// Accepts human-readable values: "100MB", "1GB", etc.
    #[serde(
        default = "default_max_file_size",
        deserialize_with = "deserialize_file_size"
    )]
    pub max_file_size: u64,

    /// Require confirmation before write operations
    #[serde(default = "default_require_confirmation_for_write")]
    pub require_confirmation_for_write: bool,

    /// Require confirmation before delete operations
    #[serde(default = "default_require_confirmation_for_delete")]
    pub require_confirmation_for_delete: bool,
}

// =============================================================================
// CodeExecConfigToml
// =============================================================================

/// Code execution executor configuration
///
/// Configures code/script execution behavior and security.
/// Code execution is disabled by default for security.
///
/// # Example TOML
/// ```toml
/// [cowork.code_exec]
/// enabled = false
/// default_runtime = "shell"
/// timeout_seconds = 60
/// sandbox_enabled = true
/// allowed_runtimes = ["shell", "python"]
/// allow_network = false
/// working_directory = "~/Downloads"
/// pass_env = ["PATH", "HOME"]
/// blocked_commands = ["rm -rf /", "sudo"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeExecConfigToml {
    /// Enable code execution
    /// SECURITY: Disabled by default
    #[serde(default = "default_code_exec_enabled")]
    pub enabled: bool,

    /// Default runtime for code execution
    #[serde(default = "default_code_exec_runtime")]
    pub default_runtime: String,

    /// Execution timeout in seconds
    #[serde(default = "default_code_exec_timeout")]
    pub timeout_seconds: u64,

    /// Enable sandboxed execution (macOS sandbox-exec)
    #[serde(default = "default_code_exec_sandbox")]
    pub sandbox_enabled: bool,

    /// Allowed runtimes (empty = all)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_runtimes: Vec<String>,

    /// Allow network access in sandbox
    #[serde(default = "default_code_exec_network")]
    pub allow_network: bool,

    /// Working directory for executions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<String>,

    /// Environment variables to pass to executed code
    #[serde(default = "default_code_exec_pass_env")]
    pub pass_env: Vec<String>,

    /// Blocked command patterns (regex)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_commands: Vec<String>,
}

impl Default for CodeExecConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_code_exec_enabled(),
            default_runtime: default_code_exec_runtime(),
            timeout_seconds: default_code_exec_timeout(),
            sandbox_enabled: default_code_exec_sandbox(),
            allowed_runtimes: Vec::new(),
            allow_network: default_code_exec_network(),
            working_directory: None,
            pass_env: default_code_exec_pass_env(),
            blocked_commands: Vec::new(),
        }
    }
}

impl CodeExecConfigToml {
    /// Validate the code execution configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate timeout
        if self.timeout_seconds == 0 {
            return Err("cowork.code_exec.timeout_seconds must be greater than 0".to_string());
        }
        if self.timeout_seconds > 3600 {
            tracing::warn!(
                timeout = self.timeout_seconds,
                "cowork.code_exec.timeout_seconds is very high (>1 hour)"
            );
        }

        // Validate runtime names
        let valid_runtimes = [
            "shell", "bash", "zsh", "python", "python3", "node", "nodejs", "ruby",
        ];
        for runtime in &self.allowed_runtimes {
            if !valid_runtimes.contains(&runtime.as_str()) {
                tracing::warn!(
                    runtime = runtime,
                    "cowork.code_exec.allowed_runtimes contains unknown runtime"
                );
            }
        }

        // Validate blocked command patterns are valid regex
        for pattern in &self.blocked_commands {
            if regex::Regex::new(pattern).is_err() {
                return Err(format!(
                    "cowork.code_exec.blocked_commands contains invalid regex: '{}'",
                    pattern
                ));
            }
        }

        Ok(())
    }

    /// Create a CodeExecutor from this configuration
    pub fn create_executor(
        &self,
        permission_checker: crate::dispatcher::executor::PathPermissionChecker,
    ) -> crate::dispatcher::executor::CodeExecutor {
        use std::path::PathBuf;

        // Expand tilde in working directory
        let working_dir = self.working_directory.as_ref().map(|s| {
            if s.starts_with("~/") {
                if let Some(home) = dirs::home_dir() {
                    return PathBuf::from(s.replacen("~", home.to_string_lossy().as_ref(), 1));
                }
            } else if s == "~" {
                if let Some(home) = dirs::home_dir() {
                    return home;
                }
            }
            PathBuf::from(s)
        });

        crate::dispatcher::executor::CodeExecutor::new(
            self.enabled,
            self.default_runtime.clone(),
            self.timeout_seconds,
            self.sandbox_enabled,
            self.allowed_runtimes.clone(),
            self.allow_network,
            self.blocked_commands.clone(),
            permission_checker,
            working_dir,
            self.pass_env.clone(),
        )
    }
}

// =============================================================================
// ModelProfileConfigToml
// =============================================================================

/// Model profile configuration from TOML
///
/// Defines an AI model's capabilities, cost tier, and performance characteristics.
/// Used for intelligent task-to-model routing in multi-model pipelines.
///
/// # Example TOML
/// ```toml
/// [cowork.model_profiles.claude-opus]
/// provider = "anthropic"
/// model = "claude-opus-4"
/// capabilities = ["reasoning", "code_generation", "long_context"]
/// cost_tier = "high"
/// latency_tier = "slow"
/// max_context = 200000
///
/// [cowork.model_profiles.ollama-llama]
/// provider = "ollama"
/// model = "llama3.2"
/// capabilities = ["local_privacy", "fast_response"]
/// cost_tier = "free"
/// local = true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfileConfigToml {
    /// Provider name (anthropic, openai, google, ollama)
    pub provider: String,

    /// Model name for API calls
    pub model: String,

    /// Capability tags for this model
    #[serde(default)]
    pub capabilities: Vec<Capability>,

    /// Cost tier for cost-aware routing
    #[serde(default)]
    pub cost_tier: CostTier,

    /// Latency tier for latency-sensitive tasks
    #[serde(default)]
    pub latency_tier: LatencyTier,

    /// Maximum context window in tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context: Option<u32>,

    /// Whether this is a local model (no network calls)
    #[serde(default)]
    pub local: bool,

    /// Custom parameters for provider-specific settings
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

impl ModelProfileConfigToml {
    /// Convert to ModelProfile with the given ID
    pub fn to_model_profile(&self, id: String) -> ModelProfile {
        ModelProfile {
            id,
            provider: self.provider.clone(),
            model: self.model.clone(),
            capabilities: self.capabilities.clone(),
            cost_tier: self.cost_tier,
            latency_tier: self.latency_tier,
            max_context: self.max_context,
            local: self.local,
            parameters: self.parameters.clone(),
        }
    }

    /// Validate the model profile configuration
    pub fn validate(&self, profile_id: &str) -> Result<(), String> {
        // Validate provider is not empty
        if self.provider.is_empty() {
            return Err(format!(
                "cowork.model_profiles.{}.provider cannot be empty",
                profile_id
            ));
        }

        // Validate model is not empty
        if self.model.is_empty() {
            return Err(format!(
                "cowork.model_profiles.{}.model cannot be empty",
                profile_id
            ));
        }

        // Validate known providers
        let known_providers = ["anthropic", "openai", "google", "ollama", "gemini"];
        if !known_providers.contains(&self.provider.as_str()) {
            tracing::warn!(
                profile_id = profile_id,
                provider = self.provider,
                "Unknown provider in model profile, routing may not work"
            );
        }

        // Validate max_context if specified
        if let Some(max_ctx) = self.max_context {
            if max_ctx == 0 {
                return Err(format!(
                    "cowork.model_profiles.{}.max_context must be greater than 0",
                    profile_id
                ));
            }
        }

        Ok(())
    }
}

// =============================================================================
// ModelRoutingConfigToml
// =============================================================================

/// Model routing configuration from TOML
///
/// Defines how tasks are routed to different AI models based on task type,
/// required capabilities, and cost optimization strategy.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing]
/// code_generation = "claude-opus"
/// code_review = "claude-sonnet"
/// image_analysis = "gpt-4o"
/// video_understanding = "gemini-pro"
/// long_document = "gemini-pro"
/// quick_tasks = "claude-haiku"
/// privacy_sensitive = "ollama-llama"
/// reasoning = "claude-opus"
/// cost_strategy = "balanced"
/// enable_pipelines = true
/// default_model = "claude-sonnet"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoutingConfigToml {
    /// Model for code generation tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_generation: Option<String>,

    /// Model for code review tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_review: Option<String>,

    /// Model for image analysis tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_analysis: Option<String>,

    /// Model for video understanding tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_understanding: Option<String>,

    /// Model for long document processing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_document: Option<String>,

    /// Model for quick/simple tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_tasks: Option<String>,

    /// Model for privacy-sensitive tasks (should be local)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub privacy_sensitive: Option<String>,

    /// Model for complex reasoning tasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,

    /// Cost optimization strategy
    #[serde(default)]
    pub cost_strategy: CostStrategy,

    /// Enable multi-model pipeline execution
    #[serde(default = "default_enable_pipelines")]
    pub enable_pipelines: bool,

    /// Default model when no specific routing rule matches
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    /// User overrides for specific task types
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub overrides: HashMap<String, String>,

    /// Metrics collection configuration
    #[serde(default)]
    pub metrics: MetricsConfigToml,

    /// Health check configuration
    #[serde(default)]
    pub health: HealthConfigToml,
}

fn default_enable_pipelines() -> bool {
    true
}

impl Default for ModelRoutingConfigToml {
    fn default() -> Self {
        Self {
            code_generation: None,
            code_review: None,
            image_analysis: None,
            video_understanding: None,
            long_document: None,
            quick_tasks: None,
            privacy_sensitive: None,
            reasoning: None,
            cost_strategy: CostStrategy::default(),
            enable_pipelines: true,
            default_model: None,
            overrides: HashMap::new(),
            metrics: MetricsConfigToml::default(),
            health: HealthConfigToml::default(),
        }
    }
}

impl ModelRoutingConfigToml {
    /// Convert to ModelRoutingRules
    #[allow(clippy::field_reassign_with_default)]
    pub fn to_routing_rules(&self) -> ModelRoutingRules {
        let mut rules = ModelRoutingRules::default();

        // Set cost strategy
        rules.cost_strategy = self.cost_strategy;
        rules.enable_pipelines = self.enable_pipelines;
        rules.default_model = self.default_model.clone();

        // Add task type mappings
        if let Some(ref model) = self.code_generation {
            rules
                .task_type_mappings
                .insert("code_generation".to_string(), model.clone());
        }
        if let Some(ref model) = self.code_review {
            rules
                .task_type_mappings
                .insert("code_review".to_string(), model.clone());
        }
        if let Some(ref model) = self.image_analysis {
            rules
                .task_type_mappings
                .insert("image_analysis".to_string(), model.clone());
        }
        if let Some(ref model) = self.video_understanding {
            rules
                .task_type_mappings
                .insert("video_understanding".to_string(), model.clone());
        }
        if let Some(ref model) = self.long_document {
            rules
                .task_type_mappings
                .insert("long_document".to_string(), model.clone());
        }
        if let Some(ref model) = self.quick_tasks {
            rules
                .task_type_mappings
                .insert("quick_tasks".to_string(), model.clone());
        }
        if let Some(ref model) = self.privacy_sensitive {
            rules
                .task_type_mappings
                .insert("privacy_sensitive".to_string(), model.clone());
        }
        if let Some(ref model) = self.reasoning {
            rules
                .task_type_mappings
                .insert("reasoning".to_string(), model.clone());
        }

        // Add user overrides
        for (task_type, model) in &self.overrides {
            rules
                .task_type_mappings
                .insert(task_type.clone(), model.clone());
        }

        // Add capability mappings based on task types
        if let Some(ref model) = self.code_generation {
            rules
                .capability_mappings
                .insert(Capability::CodeGeneration, model.clone());
        }
        if let Some(ref model) = self.code_review {
            rules
                .capability_mappings
                .insert(Capability::CodeReview, model.clone());
        }
        if let Some(ref model) = self.image_analysis {
            rules
                .capability_mappings
                .insert(Capability::ImageUnderstanding, model.clone());
        }
        if let Some(ref model) = self.video_understanding {
            rules
                .capability_mappings
                .insert(Capability::VideoUnderstanding, model.clone());
        }
        if let Some(ref model) = self.long_document {
            rules
                .capability_mappings
                .insert(Capability::LongDocument, model.clone());
        }
        if let Some(ref model) = self.quick_tasks {
            rules
                .capability_mappings
                .insert(Capability::FastResponse, model.clone());
        }
        if let Some(ref model) = self.privacy_sensitive {
            rules
                .capability_mappings
                .insert(Capability::LocalPrivacy, model.clone());
        }
        if let Some(ref model) = self.reasoning {
            rules
                .capability_mappings
                .insert(Capability::Reasoning, model.clone());
        }

        rules
    }

    /// Validate routing configuration against available model profiles
    pub fn validate(&self, available_profiles: &[&str]) -> Result<(), String> {
        let profile_set: std::collections::HashSet<&str> =
            available_profiles.iter().copied().collect();

        // Helper to validate a model reference
        let validate_model = |model: &Option<String>, field: &str| -> Result<(), String> {
            if let Some(ref model_id) = model {
                if !profile_set.contains(model_id.as_str()) {
                    return Err(format!(
                        "cowork.model_routing.{} references unknown profile '{}'. Available: {:?}",
                        field, model_id, available_profiles
                    ));
                }
            }
            Ok(())
        };

        // Validate all model references
        validate_model(&self.code_generation, "code_generation")?;
        validate_model(&self.code_review, "code_review")?;
        validate_model(&self.image_analysis, "image_analysis")?;
        validate_model(&self.video_understanding, "video_understanding")?;
        validate_model(&self.long_document, "long_document")?;
        validate_model(&self.quick_tasks, "quick_tasks")?;
        validate_model(&self.privacy_sensitive, "privacy_sensitive")?;
        validate_model(&self.reasoning, "reasoning")?;
        validate_model(&self.default_model, "default_model")?;

        // Validate overrides
        for (task_type, model_id) in &self.overrides {
            if !profile_set.contains(model_id.as_str()) {
                return Err(format!(
                    "cowork.model_routing.overrides.{} references unknown profile '{}'. Available: {:?}",
                    task_type, model_id, available_profiles
                ));
            }
        }

        // Validate metrics configuration
        self.metrics.validate()?;

        // Validate health configuration
        self.health.validate()?;

        Ok(())
    }

    /// Get all model IDs referenced in routing config
    pub fn referenced_model_ids(&self) -> Vec<&str> {
        let mut ids = Vec::new();

        if let Some(ref m) = self.code_generation {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.code_review {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.image_analysis {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.video_understanding {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.long_document {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.quick_tasks {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.privacy_sensitive {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.reasoning {
            ids.push(m.as_str());
        }
        if let Some(ref m) = self.default_model {
            ids.push(m.as_str());
        }

        for m in self.overrides.values() {
            ids.push(m.as_str());
        }

        ids
    }
}

// Code execution default functions
fn default_code_exec_enabled() -> bool {
    false // Disabled by default for security
}

fn default_code_exec_runtime() -> String {
    "shell".to_string()
}

fn default_code_exec_timeout() -> u64 {
    60 // 1 minute default
}

fn default_code_exec_sandbox() -> bool {
    true // Sandbox enabled by default
}

fn default_code_exec_network() -> bool {
    false // Network disabled by default
}

fn default_code_exec_pass_env() -> Vec<String> {
    vec!["PATH".to_string(), "HOME".to_string(), "USER".to_string()]
}

// =============================================================================
// Default Functions
// =============================================================================

pub fn default_cowork_enabled() -> bool {
    true
}

pub fn default_require_confirmation() -> bool {
    true
}

pub fn default_max_parallelism() -> usize {
    4
}

pub fn default_dry_run() -> bool {
    false
}

pub fn default_auto_execute_threshold() -> f32 {
    0.95 // Very high confidence required for auto-execution
}

pub fn default_max_tasks_per_graph() -> usize {
    20
}

pub fn default_task_timeout_seconds() -> u64 {
    300 // 5 minutes default
}

pub fn default_sandbox_enabled() -> bool {
    true
}

// FileOps default functions
pub fn default_file_ops_enabled() -> bool {
    true
}

pub fn default_max_file_size() -> u64 {
    100 * 1024 * 1024 // 100MB
}

pub fn default_require_confirmation_for_write() -> bool {
    true
}

pub fn default_require_confirmation_for_delete() -> bool {
    true
}

/// Deserialize file size from human-readable string or number
///
/// Supports formats like "100MB", "1GB", "500KB", or plain numbers (bytes)
fn deserialize_file_size<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FileSizeValue {
        Number(u64),
        String(String),
    }

    match FileSizeValue::deserialize(deserializer)? {
        FileSizeValue::Number(n) => Ok(n),
        FileSizeValue::String(s) => parse_file_size(&s).map_err(D::Error::custom),
    }
}

/// Parse human-readable file size string
fn parse_file_size(s: &str) -> Result<u64, String> {
    let s = s.trim().to_uppercase();

    // Try to parse as plain number first
    if let Ok(n) = s.parse::<u64>() {
        return Ok(n);
    }

    // Parse with suffix
    let (num_part, suffix) = if s.ends_with("GB") {
        (&s[..s.len() - 2], 1024 * 1024 * 1024)
    } else if s.ends_with("MB") {
        (&s[..s.len() - 2], 1024 * 1024)
    } else if s.ends_with("KB") {
        (&s[..s.len() - 2], 1024)
    } else if s.ends_with('B') {
        (&s[..s.len() - 1], 1)
    } else {
        return Err(format!(
            "Invalid file size format: '{}'. Use formats like '100MB', '1GB', etc.",
            s
        ));
    };

    let num: u64 = num_part
        .trim()
        .parse()
        .map_err(|_| format!("Invalid number in file size: '{}'", num_part))?;

    Ok(num * suffix)
}

// =============================================================================
// Default Implementation
// =============================================================================

impl Default for CoworkConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_cowork_enabled(),
            require_confirmation: default_require_confirmation(),
            max_parallelism: default_max_parallelism(),
            dry_run: default_dry_run(),
            planner_provider: None,
            auto_execute_threshold: default_auto_execute_threshold(),
            max_tasks_per_graph: default_max_tasks_per_graph(),
            task_timeout_seconds: default_task_timeout_seconds(),
            sandbox_enabled: default_sandbox_enabled(),
            allowed_categories: Vec::new(),
            blocked_categories: Vec::new(),
            file_ops: FileOpsConfigToml::default(),
            code_exec: CodeExecConfigToml::default(),
            model_profiles: HashMap::new(),
            model_routing: ModelRoutingConfigToml::default(),
        }
    }
}

impl Default for FileOpsConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_file_ops_enabled(),
            allowed_paths: Vec::new(),
            denied_paths: Vec::new(),
            max_file_size: default_max_file_size(),
            require_confirmation_for_write: default_require_confirmation_for_write(),
            require_confirmation_for_delete: default_require_confirmation_for_delete(),
        }
    }
}

impl FileOpsConfigToml {
    /// Validate the file ops configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate max_file_size (warn if very large)
        if self.max_file_size > 10 * 1024 * 1024 * 1024 {
            tracing::warn!(
                max_file_size = self.max_file_size,
                "cowork.file_ops.max_file_size is very large (>10GB)"
            );
        }

        // Validate path patterns are valid glob patterns
        for path in &self.allowed_paths {
            if glob::Pattern::new(path).is_err() {
                return Err(format!(
                    "cowork.file_ops.allowed_paths contains invalid glob pattern: '{}'",
                    path
                ));
            }
        }

        for path in &self.denied_paths {
            if glob::Pattern::new(path).is_err() {
                return Err(format!(
                    "cowork.file_ops.denied_paths contains invalid glob pattern: '{}'",
                    path
                ));
            }
        }

        Ok(())
    }

    /// Create a FileOpsExecutor from this configuration
    pub fn create_executor(&self) -> crate::dispatcher::executor::FileOpsExecutor {
        crate::dispatcher::executor::FileOpsExecutor::new(
            self.allowed_paths.clone(),
            self.denied_paths.clone(),
            self.max_file_size,
            self.require_confirmation_for_write,
            self.require_confirmation_for_delete,
        )
    }
}

// =============================================================================
// Conversion to Engine Config
// =============================================================================

impl CoworkConfigToml {
    /// Convert to engine configuration
    ///
    /// This creates a CoworkConfig suitable for the CoworkEngine.
    pub fn to_engine_config(&self) -> crate::dispatcher::CoworkConfig {
        crate::dispatcher::CoworkConfig {
            enabled: self.enabled,
            require_confirmation: self.require_confirmation,
            max_parallelism: self.max_parallelism,
            dry_run: self.dry_run,
            enable_pipelines: self.model_routing.enable_pipelines,
            model_profiles: self.get_model_profiles(),
            routing_rules: Some(self.get_routing_rules()),
        }
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate max_parallelism
        if self.max_parallelism == 0 {
            return Err("cowork.max_parallelism must be greater than 0".to_string());
        }
        if self.max_parallelism > 32 {
            // Warning but not error
            tracing::warn!(
                max_parallelism = self.max_parallelism,
                "cowork.max_parallelism is very high (>32), this may cause resource issues"
            );
        }

        // Validate auto_execute_threshold
        if !(0.0..=1.0).contains(&self.auto_execute_threshold) {
            return Err(format!(
                "cowork.auto_execute_threshold must be between 0.0 and 1.0, got {}",
                self.auto_execute_threshold
            ));
        }

        // Validate max_tasks_per_graph
        if self.max_tasks_per_graph == 0 {
            return Err("cowork.max_tasks_per_graph must be greater than 0".to_string());
        }
        if self.max_tasks_per_graph > 100 {
            tracing::warn!(
                max_tasks = self.max_tasks_per_graph,
                "cowork.max_tasks_per_graph is very high (>100), this may indicate a problem"
            );
        }

        // Validate category names
        let valid_categories = [
            "file_operation",
            "code_execution",
            "document_generation",
            "app_automation",
            "ai_inference",
        ];

        for cat in &self.allowed_categories {
            if !valid_categories.contains(&cat.as_str()) {
                return Err(format!(
                    "cowork.allowed_categories contains unknown category '{}'. Valid: {:?}",
                    cat, valid_categories
                ));
            }
        }

        for cat in &self.blocked_categories {
            if !valid_categories.contains(&cat.as_str()) {
                return Err(format!(
                    "cowork.blocked_categories contains unknown category '{}'. Valid: {:?}",
                    cat, valid_categories
                ));
            }
        }

        // Validate file_ops configuration
        self.file_ops.validate()?;

        // Validate code_exec configuration
        self.code_exec.validate()?;

        // Validate model profiles
        for (profile_id, profile_config) in &self.model_profiles {
            profile_config.validate(profile_id)?;
        }

        // Validate model routing (check profile references)
        let profile_ids: Vec<&str> = self.model_profiles.keys().map(|s| s.as_str()).collect();
        self.model_routing.validate(&profile_ids)?;

        Ok(())
    }

    /// Get all model profiles as ModelProfile objects
    pub fn get_model_profiles(&self) -> Vec<ModelProfile> {
        self.model_profiles
            .iter()
            .map(|(id, config)| config.to_model_profile(id.clone()))
            .collect()
    }

    /// Get model routing rules
    pub fn get_routing_rules(&self) -> ModelRoutingRules {
        self.model_routing.to_routing_rules()
    }

    /// Get a specific model profile by ID
    pub fn get_model_profile(&self, id: &str) -> Option<ModelProfile> {
        self.model_profiles
            .get(id)
            .map(|config| config.to_model_profile(id.to_string()))
    }

    /// Check if a task category is allowed
    pub fn is_category_allowed(&self, category: &str) -> bool {
        // Blocked categories take precedence
        if self.blocked_categories.contains(&category.to_string()) {
            return false;
        }

        // If allowed_categories is empty, all categories are allowed
        if self.allowed_categories.is_empty() {
            return true;
        }

        // Check if category is in allowed list
        self.allowed_categories.contains(&category.to_string())
    }
}

// =============================================================================
// MetricsConfigToml
// =============================================================================

/// Metrics collection configuration from TOML
///
/// Configures runtime metrics collection for intelligent model routing.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing.metrics]
/// enabled = true
/// buffer_size = 10000
/// aggregation_interval_secs = 60
/// flush_interval_secs = 300
/// db_path = "~/.config/aether/metrics.db"
/// exploration_rate = 0.05
///
/// [cowork.model_routing.metrics.windows]
/// short_term_secs = 300
/// medium_term_secs = 3600
/// long_term_secs = 86400
///
/// [cowork.model_routing.metrics.scoring]
/// latency_weight = 0.25
/// cost_weight = 0.25
/// reliability_weight = 0.35
/// quality_weight = 0.15
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfigToml {
    /// Enable metrics collection
    #[serde(default = "default_metrics_enabled")]
    pub enabled: bool,

    /// Ring buffer size for call records
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    /// Interval for aggregating metrics (seconds)
    #[serde(default = "default_aggregation_interval")]
    pub aggregation_interval_secs: u64,

    /// Interval for flushing to persistent storage (seconds)
    #[serde(default = "default_flush_interval")]
    pub flush_interval_secs: u64,

    /// Path to SQLite database for persistence
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_path: Option<String>,

    /// Exploration rate for epsilon-greedy routing (0.0-1.0)
    #[serde(default = "default_exploration_rate")]
    pub exploration_rate: f64,

    /// Time window configuration
    #[serde(default)]
    pub windows: TimeWindowsConfigToml,

    /// Scoring configuration
    #[serde(default)]
    pub scoring: ScoringConfigToml,
}

impl Default for MetricsConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_metrics_enabled(),
            buffer_size: default_buffer_size(),
            aggregation_interval_secs: default_aggregation_interval(),
            flush_interval_secs: default_flush_interval(),
            db_path: None,
            exploration_rate: default_exploration_rate(),
            windows: TimeWindowsConfigToml::default(),
            scoring: ScoringConfigToml::default(),
        }
    }
}

impl MetricsConfigToml {
    /// Convert to MetricsConfig for the collector
    pub fn to_metrics_config(&self) -> crate::dispatcher::model_router::MetricsConfig {
        use crate::dispatcher::model_router::WindowConfig;

        crate::dispatcher::model_router::MetricsConfig {
            buffer_size: self.buffer_size,
            aggregation_interval: std::time::Duration::from_secs(self.aggregation_interval_secs),
            flush_interval: std::time::Duration::from_secs(self.flush_interval_secs),
            window_config: WindowConfig {
                short_term: std::time::Duration::from_secs(self.windows.short_term_secs),
                medium_term: std::time::Duration::from_secs(self.windows.medium_term_secs),
                long_term: std::time::Duration::from_secs(self.windows.long_term_secs),
            },
            persist_enabled: self.db_path.is_some(),
        }
    }

    /// Convert to ScoringConfig for the scorer
    pub fn to_scoring_config(&self) -> ScoringConfig {
        self.scoring.to_scoring_config()
    }

    /// Validate metrics configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.buffer_size == 0 {
            return Err(
                "cowork.model_routing.metrics.buffer_size must be greater than 0".to_string(),
            );
        }

        if self.exploration_rate < 0.0 || self.exploration_rate > 1.0 {
            return Err(format!(
                "cowork.model_routing.metrics.exploration_rate must be between 0.0 and 1.0, got {}",
                self.exploration_rate
            ));
        }

        self.scoring.validate()?;

        Ok(())
    }
}

/// Time windows configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeWindowsConfigToml {
    /// Short-term window in seconds (default 5 minutes)
    #[serde(default = "default_short_term_secs")]
    pub short_term_secs: u64,

    /// Medium-term window in seconds (default 1 hour)
    #[serde(default = "default_medium_term_secs")]
    pub medium_term_secs: u64,

    /// Long-term window in seconds (default 24 hours)
    #[serde(default = "default_long_term_secs")]
    pub long_term_secs: u64,
}

impl Default for TimeWindowsConfigToml {
    fn default() -> Self {
        Self {
            short_term_secs: default_short_term_secs(),
            medium_term_secs: default_medium_term_secs(),
            long_term_secs: default_long_term_secs(),
        }
    }
}

/// Scoring weights configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringConfigToml {
    /// Weight for latency score (0.0-1.0)
    #[serde(default = "default_latency_weight")]
    pub latency_weight: f64,

    /// Weight for cost score (0.0-1.0)
    #[serde(default = "default_cost_weight")]
    pub cost_weight: f64,

    /// Weight for reliability score (0.0-1.0)
    #[serde(default = "default_reliability_weight")]
    pub reliability_weight: f64,

    /// Weight for quality score (0.0-1.0)
    #[serde(default = "default_quality_weight")]
    pub quality_weight: f64,

    /// Target latency in milliseconds for scoring
    #[serde(default = "default_latency_target_ms")]
    pub latency_target_ms: u64,

    /// Maximum acceptable latency in milliseconds
    #[serde(default = "default_latency_max_ms")]
    pub latency_max_ms: u64,

    /// Minimum success rate for full reliability score
    #[serde(default = "default_min_success_rate")]
    pub min_success_rate: f64,
}

impl Default for ScoringConfigToml {
    fn default() -> Self {
        Self {
            latency_weight: default_latency_weight(),
            cost_weight: default_cost_weight(),
            reliability_weight: default_reliability_weight(),
            quality_weight: default_quality_weight(),
            latency_target_ms: default_latency_target_ms(),
            latency_max_ms: default_latency_max_ms(),
            min_success_rate: default_min_success_rate(),
        }
    }
}

impl ScoringConfigToml {
    /// Convert to ScoringConfig
    pub fn to_scoring_config(&self) -> ScoringConfig {
        ScoringConfig {
            latency_weight: self.latency_weight,
            cost_weight: self.cost_weight,
            reliability_weight: self.reliability_weight,
            quality_weight: self.quality_weight,
            latency_target_ms: self.latency_target_ms as f64,
            latency_max_ms: self.latency_max_ms as f64,
            min_success_rate: self.min_success_rate,
            degradation_threshold: 3, // Default
            min_samples: 10,          // Default
        }
    }

    /// Validate scoring configuration
    pub fn validate(&self) -> Result<(), String> {
        let total = self.latency_weight + self.cost_weight + self.reliability_weight + self.quality_weight;
        if (total - 1.0).abs() > 0.01 {
            tracing::warn!(
                total = total,
                "Scoring weights do not sum to 1.0, they will be normalized"
            );
        }

        if self.latency_target_ms >= self.latency_max_ms {
            return Err(format!(
                "latency_target_ms ({}) must be less than latency_max_ms ({})",
                self.latency_target_ms, self.latency_max_ms
            ));
        }

        if self.min_success_rate < 0.0 || self.min_success_rate > 1.0 {
            return Err(format!(
                "min_success_rate must be between 0.0 and 1.0, got {}",
                self.min_success_rate
            ));
        }

        Ok(())
    }
}

// Metrics default functions
fn default_metrics_enabled() -> bool {
    true
}

fn default_buffer_size() -> usize {
    10000
}

fn default_aggregation_interval() -> u64 {
    60
}

fn default_flush_interval() -> u64 {
    300
}

fn default_exploration_rate() -> f64 {
    0.05
}

fn default_short_term_secs() -> u64 {
    300 // 5 minutes
}

fn default_medium_term_secs() -> u64 {
    3600 // 1 hour
}

fn default_long_term_secs() -> u64 {
    86400 // 24 hours
}

fn default_latency_weight() -> f64 {
    0.25
}

fn default_cost_weight() -> f64 {
    0.25
}

fn default_reliability_weight() -> f64 {
    0.35
}

fn default_quality_weight() -> f64 {
    0.15
}

fn default_latency_target_ms() -> u64 {
    2000
}

fn default_latency_max_ms() -> u64 {
    30000
}

fn default_min_success_rate() -> f64 {
    0.9
}

// =============================================================================
// HealthConfigToml
// =============================================================================

/// Health check system configuration from TOML
///
/// Configures model health monitoring and circuit breaker behavior.
///
/// # Example TOML
/// ```toml
/// [cowork.model_routing.health]
/// enabled = true
/// active_probing = true
/// failure_threshold = 3
/// recovery_successes = 2
/// latency_degradation_threshold_ms = 10000
/// latency_healthy_threshold_ms = 5000
/// rate_limit_warning_threshold = 0.2
///
/// [cowork.model_routing.health.circuit_breaker]
/// failure_threshold = 5
/// window_secs = 60
/// cooldown_secs = 30
/// half_open_successes = 2
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfigToml {
    /// Enable health tracking
    #[serde(default = "default_health_enabled")]
    pub enabled: bool,

    /// Enable active probing of unhealthy models
    #[serde(default = "default_active_probing")]
    pub active_probing: bool,

    /// Number of consecutive failures to mark unhealthy
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,

    /// Number of successes to recover from unhealthy
    #[serde(default = "default_recovery_successes")]
    pub recovery_successes: u32,

    /// Number of successes to recover from degraded
    #[serde(default = "default_degraded_recovery_successes")]
    pub degraded_recovery_successes: u32,

    /// Latency threshold (p95 ms) to mark as degraded
    #[serde(default = "default_latency_degradation_threshold")]
    pub latency_degradation_threshold_ms: u64,

    /// Latency threshold (p95 ms) to recover from degraded
    #[serde(default = "default_latency_healthy_threshold")]
    pub latency_healthy_threshold_ms: u64,

    /// Rate limit remaining percentage to trigger warning
    #[serde(default = "default_rate_limit_warning_threshold")]
    pub rate_limit_warning_threshold: f64,

    /// Circuit breaker configuration
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfigToml,

    /// Probe configuration
    #[serde(default)]
    pub probe: ProbeConfigToml,
}

impl Default for HealthConfigToml {
    fn default() -> Self {
        Self {
            enabled: default_health_enabled(),
            active_probing: default_active_probing(),
            failure_threshold: default_failure_threshold(),
            recovery_successes: default_recovery_successes(),
            degraded_recovery_successes: default_degraded_recovery_successes(),
            latency_degradation_threshold_ms: default_latency_degradation_threshold(),
            latency_healthy_threshold_ms: default_latency_healthy_threshold(),
            rate_limit_warning_threshold: default_rate_limit_warning_threshold(),
            circuit_breaker: CircuitBreakerConfigToml::default(),
            probe: ProbeConfigToml::default(),
        }
    }
}

impl HealthConfigToml {
    /// Convert to HealthConfig for the health manager
    pub fn to_health_config(&self) -> HealthConfig {
        HealthConfig {
            enabled: self.enabled,
            active_probing: self.active_probing,
            failure_threshold: self.failure_threshold,
            recovery_successes: self.recovery_successes,
            degraded_recovery_successes: self.degraded_recovery_successes,
            latency_degradation_threshold_ms: self.latency_degradation_threshold_ms,
            latency_healthy_threshold_ms: self.latency_healthy_threshold_ms,
            rate_limit_warning_threshold: self.rate_limit_warning_threshold,
            circuit_breaker: self.circuit_breaker.to_circuit_breaker_config(),
            probe: self.probe.to_probe_config(),
        }
    }

    /// Validate health configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.failure_threshold == 0 {
            return Err(
                "cowork.model_routing.health.failure_threshold must be greater than 0".to_string(),
            );
        }

        if self.recovery_successes == 0 {
            return Err(
                "cowork.model_routing.health.recovery_successes must be greater than 0".to_string(),
            );
        }

        if self.latency_healthy_threshold_ms >= self.latency_degradation_threshold_ms {
            return Err(format!(
                "latency_healthy_threshold_ms ({}) must be less than latency_degradation_threshold_ms ({})",
                self.latency_healthy_threshold_ms, self.latency_degradation_threshold_ms
            ));
        }

        if self.rate_limit_warning_threshold < 0.0 || self.rate_limit_warning_threshold > 1.0 {
            return Err(format!(
                "rate_limit_warning_threshold must be between 0.0 and 1.0, got {}",
                self.rate_limit_warning_threshold
            ));
        }

        self.circuit_breaker.validate()?;

        Ok(())
    }
}

/// Circuit breaker configuration from TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfigToml {
    /// Number of failures to open circuit
    #[serde(default = "default_cb_failure_threshold")]
    pub failure_threshold: u32,

    /// Window in seconds for counting failures
    #[serde(default = "default_cb_window_secs")]
    pub window_secs: u64,

    /// Base cooldown in seconds before half-open
    #[serde(default = "default_cb_cooldown_secs")]
    pub cooldown_secs: u64,

    /// Number of successes in half-open to close circuit
    #[serde(default = "default_cb_half_open_successes")]
    pub half_open_successes: u32,
}

impl Default for CircuitBreakerConfigToml {
    fn default() -> Self {
        Self {
            failure_threshold: default_cb_failure_threshold(),
            window_secs: default_cb_window_secs(),
            cooldown_secs: default_cb_cooldown_secs(),
            half_open_successes: default_cb_half_open_successes(),
        }
    }
}

impl CircuitBreakerConfigToml {
    /// Convert to CircuitBreakerConfig
    pub fn to_circuit_breaker_config(&self) -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: self.failure_threshold,
            window_secs: self.window_secs,
            cooldown_secs: self.cooldown_secs,
            half_open_successes: self.half_open_successes,
        }
    }

    /// Validate circuit breaker configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.failure_threshold == 0 {
            return Err(
                "circuit_breaker.failure_threshold must be greater than 0".to_string(),
            );
        }

        if self.cooldown_secs == 0 {
            return Err("circuit_breaker.cooldown_secs must be greater than 0".to_string());
        }

        if self.half_open_successes == 0 {
            return Err(
                "circuit_breaker.half_open_successes must be greater than 0".to_string(),
            );
        }

        Ok(())
    }
}

/// Probe configuration from TOML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeConfigToml {
    /// Interval between probes in seconds
    #[serde(default = "default_probe_interval_secs")]
    pub interval_secs: u64,

    /// Timeout for probe requests in seconds
    #[serde(default = "default_probe_timeout_secs")]
    pub timeout_secs: u64,

    /// Minimal test prompt for probing
    #[serde(default = "default_probe_test_prompt")]
    pub test_prompt: String,
}

impl Default for ProbeConfigToml {
    fn default() -> Self {
        Self {
            interval_secs: default_probe_interval_secs(),
            timeout_secs: default_probe_timeout_secs(),
            test_prompt: default_probe_test_prompt(),
        }
    }
}

impl ProbeConfigToml {
    /// Convert to ProbeConfig
    pub fn to_probe_config(&self) -> ProbeConfig {
        ProbeConfig {
            interval_secs: self.interval_secs,
            timeout_secs: self.timeout_secs,
            test_prompt: self.test_prompt.clone(),
        }
    }
}

// Health default functions
fn default_health_enabled() -> bool {
    true
}

fn default_active_probing() -> bool {
    false
}

fn default_failure_threshold() -> u32 {
    3
}

fn default_recovery_successes() -> u32 {
    2
}

fn default_degraded_recovery_successes() -> u32 {
    3
}

fn default_latency_degradation_threshold() -> u64 {
    10000
}

fn default_latency_healthy_threshold() -> u64 {
    5000
}

fn default_rate_limit_warning_threshold() -> f64 {
    0.2
}

fn default_cb_failure_threshold() -> u32 {
    5
}

fn default_cb_window_secs() -> u64 {
    60
}

fn default_cb_cooldown_secs() -> u64 {
    30
}

fn default_cb_half_open_successes() -> u32 {
    2
}

fn default_probe_interval_secs() -> u64 {
    30
}

fn default_probe_timeout_secs() -> u64 {
    10
}

fn default_probe_test_prompt() -> String {
    "Hi".to_string()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CoworkConfigToml::default();
        assert!(config.enabled);
        assert!(config.require_confirmation);
        assert_eq!(config.max_parallelism, 4);
        assert!(!config.dry_run);
        assert!(config.planner_provider.is_none());
    }

    #[test]
    fn test_validation() {
        let mut config = CoworkConfigToml::default();

        // Valid config should pass
        assert!(config.validate().is_ok());

        // Invalid max_parallelism
        config.max_parallelism = 0;
        assert!(config.validate().is_err());
        config.max_parallelism = 4;

        // Invalid auto_execute_threshold
        config.auto_execute_threshold = 1.5;
        assert!(config.validate().is_err());
        config.auto_execute_threshold = 0.95;

        // Invalid category
        config.allowed_categories = vec!["invalid_category".to_string()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_category_filtering() {
        let mut config = CoworkConfigToml::default();

        // All allowed by default
        assert!(config.is_category_allowed("file_operation"));
        assert!(config.is_category_allowed("code_execution"));

        // Block a category
        config.blocked_categories = vec!["code_execution".to_string()];
        assert!(config.is_category_allowed("file_operation"));
        assert!(!config.is_category_allowed("code_execution"));

        // Allow list
        config.blocked_categories.clear();
        config.allowed_categories = vec!["file_operation".to_string()];
        assert!(config.is_category_allowed("file_operation"));
        assert!(!config.is_category_allowed("code_execution"));

        // Blocked takes precedence
        config.blocked_categories = vec!["file_operation".to_string()];
        assert!(!config.is_category_allowed("file_operation"));
    }

    #[test]
    fn test_to_engine_config() {
        let config = CoworkConfigToml {
            enabled: true,
            require_confirmation: false,
            max_parallelism: 8,
            dry_run: true,
            ..Default::default()
        };

        let engine_config = config.to_engine_config();
        assert!(engine_config.enabled);
        assert!(!engine_config.require_confirmation);
        assert_eq!(engine_config.max_parallelism, 8);
        assert!(engine_config.dry_run);
    }

    // =========================================================================
    // FileOpsConfigToml Tests
    // =========================================================================

    #[test]
    fn test_file_ops_default_config() {
        let config = FileOpsConfigToml::default();
        assert!(config.enabled);
        assert!(config.allowed_paths.is_empty());
        assert!(config.denied_paths.is_empty());
        assert_eq!(config.max_file_size, 100 * 1024 * 1024); // 100MB
        assert!(config.require_confirmation_for_write);
        assert!(config.require_confirmation_for_delete);
    }

    #[test]
    fn test_file_ops_validation() {
        let mut config = FileOpsConfigToml::default();

        // Valid config should pass
        assert!(config.validate().is_ok());

        // Valid glob patterns
        config.allowed_paths = vec!["~/Documents/**".to_string()];
        assert!(config.validate().is_ok());

        // Invalid glob pattern
        config.allowed_paths = vec!["[invalid".to_string()];
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_parse_file_size() {
        assert_eq!(parse_file_size("100").unwrap(), 100);
        assert_eq!(parse_file_size("1KB").unwrap(), 1024);
        assert_eq!(parse_file_size("1MB").unwrap(), 1024 * 1024);
        assert_eq!(parse_file_size("1GB").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_file_size("100MB").unwrap(), 100 * 1024 * 1024);
        assert_eq!(parse_file_size("  50 MB  ").unwrap(), 50 * 1024 * 1024);

        // Case insensitive
        assert_eq!(parse_file_size("1mb").unwrap(), 1024 * 1024);
        assert_eq!(parse_file_size("1Mb").unwrap(), 1024 * 1024);

        // Invalid formats
        assert!(parse_file_size("invalid").is_err());
        assert!(parse_file_size("100TB").is_err()); // TB not supported
    }

    #[test]
    fn test_cowork_config_includes_file_ops() {
        let config = CoworkConfigToml::default();
        assert!(config.file_ops.enabled);
        assert!(config.file_ops.require_confirmation_for_write);
    }

    // =========================================================================
    // ModelProfileConfigToml Tests
    // =========================================================================

    #[test]
    fn test_model_profile_config_to_model_profile() {
        let config = ModelProfileConfigToml {
            provider: "anthropic".to_string(),
            model: "claude-opus-4".to_string(),
            capabilities: vec![Capability::Reasoning, Capability::CodeGeneration],
            cost_tier: CostTier::High,
            latency_tier: LatencyTier::Slow,
            max_context: Some(200_000),
            local: false,
            parameters: None,
        };

        let profile = config.to_model_profile("claude-opus".to_string());
        assert_eq!(profile.id, "claude-opus");
        assert_eq!(profile.provider, "anthropic");
        assert_eq!(profile.model, "claude-opus-4");
        assert!(profile.has_capability(Capability::Reasoning));
        assert!(profile.has_capability(Capability::CodeGeneration));
        assert_eq!(profile.cost_tier, CostTier::High);
        assert_eq!(profile.latency_tier, LatencyTier::Slow);
        assert_eq!(profile.max_context, Some(200_000));
        assert!(!profile.local);
    }

    #[test]
    fn test_model_profile_config_validation() {
        // Valid config
        let valid = ModelProfileConfigToml {
            provider: "anthropic".to_string(),
            model: "claude-opus-4".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::default(),
            latency_tier: LatencyTier::default(),
            max_context: None,
            local: false,
            parameters: None,
        };
        assert!(valid.validate("test").is_ok());

        // Empty provider
        let empty_provider = ModelProfileConfigToml {
            provider: "".to_string(),
            model: "claude-opus-4".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::default(),
            latency_tier: LatencyTier::default(),
            max_context: None,
            local: false,
            parameters: None,
        };
        assert!(empty_provider.validate("test").is_err());

        // Empty model
        let empty_model = ModelProfileConfigToml {
            provider: "anthropic".to_string(),
            model: "".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::default(),
            latency_tier: LatencyTier::default(),
            max_context: None,
            local: false,
            parameters: None,
        };
        assert!(empty_model.validate("test").is_err());

        // Zero max_context
        let zero_context = ModelProfileConfigToml {
            provider: "anthropic".to_string(),
            model: "claude".to_string(),
            capabilities: vec![],
            cost_tier: CostTier::default(),
            latency_tier: LatencyTier::default(),
            max_context: Some(0),
            local: false,
            parameters: None,
        };
        assert!(zero_context.validate("test").is_err());
    }

    // =========================================================================
    // ModelRoutingConfigToml Tests
    // =========================================================================

    #[test]
    fn test_model_routing_config_default() {
        let config = ModelRoutingConfigToml::default();
        assert!(config.code_generation.is_none());
        assert!(config.default_model.is_none());
        assert_eq!(config.cost_strategy, CostStrategy::Balanced);
        assert!(config.enable_pipelines);
        assert!(config.overrides.is_empty());
    }

    #[test]
    fn test_model_routing_config_to_rules() {
        let config = ModelRoutingConfigToml {
            code_generation: Some("claude-opus".to_string()),
            code_review: Some("claude-sonnet".to_string()),
            image_analysis: Some("gpt-4o".to_string()),
            video_understanding: None,
            long_document: None,
            quick_tasks: Some("claude-haiku".to_string()),
            privacy_sensitive: Some("ollama-llama".to_string()),
            reasoning: None,
            cost_strategy: CostStrategy::Balanced,
            enable_pipelines: true,
            default_model: Some("claude-sonnet".to_string()),
            overrides: HashMap::new(),
            ..Default::default()
        };

        let rules = config.to_routing_rules();
        assert_eq!(
            rules.get_for_task_type("code_generation"),
            Some("claude-opus")
        );
        assert_eq!(
            rules.get_for_task_type("code_review"),
            Some("claude-sonnet")
        );
        assert_eq!(rules.get_for_task_type("image_analysis"), Some("gpt-4o"));
        assert_eq!(rules.get_for_task_type("quick_tasks"), Some("claude-haiku"));
        assert_eq!(rules.get_default(), Some("claude-sonnet"));
        assert_eq!(rules.cost_strategy, CostStrategy::Balanced);
        assert!(rules.enable_pipelines);
    }

    #[test]
    fn test_model_routing_config_with_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("code_generation".to_string(), "gpt-4-turbo".to_string());

        let config = ModelRoutingConfigToml {
            code_generation: Some("claude-opus".to_string()),
            overrides,
            ..Default::default()
        };

        let rules = config.to_routing_rules();
        // Override should win
        assert_eq!(
            rules.get_for_task_type("code_generation"),
            Some("gpt-4-turbo")
        );
    }

    #[test]
    fn test_model_routing_config_validation() {
        let available = ["claude-opus", "claude-sonnet", "gpt-4o"];

        // Valid config
        let valid = ModelRoutingConfigToml {
            code_generation: Some("claude-opus".to_string()),
            default_model: Some("claude-sonnet".to_string()),
            ..Default::default()
        };
        assert!(valid.validate(&available).is_ok());

        // Invalid profile reference
        let invalid = ModelRoutingConfigToml {
            code_generation: Some("nonexistent-model".to_string()),
            ..Default::default()
        };
        assert!(invalid.validate(&available).is_err());

        // Invalid default model
        let invalid_default = ModelRoutingConfigToml {
            default_model: Some("nonexistent-model".to_string()),
            ..Default::default()
        };
        assert!(invalid_default.validate(&available).is_err());
    }

    #[test]
    fn test_model_routing_referenced_ids() {
        let config = ModelRoutingConfigToml {
            code_generation: Some("claude-opus".to_string()),
            image_analysis: Some("gpt-4o".to_string()),
            default_model: Some("claude-sonnet".to_string()),
            ..Default::default()
        };

        let ids = config.referenced_model_ids();
        assert!(ids.contains(&"claude-opus"));
        assert!(ids.contains(&"gpt-4o"));
        assert!(ids.contains(&"claude-sonnet"));
    }

    // =========================================================================
    // CoworkConfigToml Model Integration Tests
    // =========================================================================

    #[test]
    fn test_cowork_config_model_profiles() {
        let mut config = CoworkConfigToml::default();

        // Add model profiles
        config.model_profiles.insert(
            "claude-opus".to_string(),
            ModelProfileConfigToml {
                provider: "anthropic".to_string(),
                model: "claude-opus-4".to_string(),
                capabilities: vec![Capability::Reasoning],
                cost_tier: CostTier::High,
                latency_tier: LatencyTier::Slow,
                max_context: Some(200_000),
                local: false,
                parameters: None,
            },
        );

        config.model_profiles.insert(
            "claude-sonnet".to_string(),
            ModelProfileConfigToml {
                provider: "anthropic".to_string(),
                model: "claude-sonnet-4".to_string(),
                capabilities: vec![Capability::CodeGeneration],
                cost_tier: CostTier::Medium,
                latency_tier: LatencyTier::Medium,
                max_context: Some(200_000),
                local: false,
                parameters: None,
            },
        );

        // Get profiles
        let profiles = config.get_model_profiles();
        assert_eq!(profiles.len(), 2);

        // Get specific profile
        let opus = config.get_model_profile("claude-opus").unwrap();
        assert_eq!(opus.provider, "anthropic");
        assert_eq!(opus.model, "claude-opus-4");

        // Non-existent profile
        assert!(config.get_model_profile("nonexistent").is_none());
    }

    #[test]
    fn test_cowork_config_model_routing_validation() {
        let mut config = CoworkConfigToml::default();

        // Add a model profile
        config.model_profiles.insert(
            "claude-opus".to_string(),
            ModelProfileConfigToml {
                provider: "anthropic".to_string(),
                model: "claude-opus-4".to_string(),
                capabilities: vec![],
                cost_tier: CostTier::High,
                latency_tier: LatencyTier::Slow,
                max_context: None,
                local: false,
                parameters: None,
            },
        );

        // Valid routing reference
        config.model_routing.code_generation = Some("claude-opus".to_string());
        assert!(config.validate().is_ok());

        // Invalid routing reference
        config.model_routing.code_review = Some("nonexistent".to_string());
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_cowork_config_get_routing_rules() {
        let mut config = CoworkConfigToml::default();

        config.model_routing = ModelRoutingConfigToml {
            code_generation: Some("claude-opus".to_string()),
            cost_strategy: CostStrategy::BestQuality,
            enable_pipelines: false,
            default_model: Some("claude-sonnet".to_string()),
            ..Default::default()
        };

        let rules = config.get_routing_rules();
        assert_eq!(
            rules.get_for_task_type("code_generation"),
            Some("claude-opus")
        );
        assert_eq!(rules.cost_strategy, CostStrategy::BestQuality);
        assert!(!rules.enable_pipelines);
        assert_eq!(rules.get_default(), Some("claude-sonnet"));
    }

    #[test]
    fn test_model_profile_toml_deserialization() {
        let toml_str = r#"
            provider = "anthropic"
            model = "claude-opus-4"
            capabilities = ["reasoning", "code_generation"]
            cost_tier = "high"
            latency_tier = "slow"
            max_context = 200000
            local = false
        "#;

        let config: ModelProfileConfigToml = toml::from_str(toml_str).unwrap();
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.model, "claude-opus-4");
        assert!(config.capabilities.contains(&Capability::Reasoning));
        assert!(config.capabilities.contains(&Capability::CodeGeneration));
        assert_eq!(config.cost_tier, CostTier::High);
        assert_eq!(config.latency_tier, LatencyTier::Slow);
        assert_eq!(config.max_context, Some(200_000));
        assert!(!config.local);
    }

    #[test]
    fn test_model_routing_toml_deserialization() {
        let toml_str = r#"
            code_generation = "claude-opus"
            code_review = "claude-sonnet"
            image_analysis = "gpt-4o"
            cost_strategy = "balanced"
            enable_pipelines = true
            default_model = "claude-sonnet"
        "#;

        let config: ModelRoutingConfigToml = toml::from_str(toml_str).unwrap();
        assert_eq!(config.code_generation, Some("claude-opus".to_string()));
        assert_eq!(config.code_review, Some("claude-sonnet".to_string()));
        assert_eq!(config.image_analysis, Some("gpt-4o".to_string()));
        assert_eq!(config.cost_strategy, CostStrategy::Balanced);
        assert!(config.enable_pipelines);
        assert_eq!(config.default_model, Some("claude-sonnet".to_string()));
    }
}

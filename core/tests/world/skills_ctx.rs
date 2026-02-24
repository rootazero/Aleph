//! Skills context for BDD tests

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use alephcore::gateway::handlers::HandlerRegistry;
use alephcore::gateway::protocol::JsonRpcResponse;
use alephcore::skill_evolution::types::{SkillMetrics, SolidificationSuggestion};
use alephcore::tools::markdown_skill::{
    MarkdownCliTool, MarkdownSkillGenerator, ReloadCallback, SkillWatcherConfig,
};
use alephcore::tools::AlephToolServer;
use tempfile::TempDir;

/// Context for markdown skill tests
pub struct SkillsContext {
    // RPC testing
    pub registry: Option<HandlerRegistry>,
    pub rpc_response: Option<JsonRpcResponse>,

    // Skill loading
    pub loaded_tools: Vec<MarkdownCliTool>,
    pub load_errors: Vec<(PathBuf, anyhow::Error)>,
    pub tool_server: Option<AlephToolServer>,

    // Skill generator
    pub generator: Option<MarkdownSkillGenerator>,
    pub generated_skill_path: Option<PathBuf>,
    pub generated_content: Option<String>,
    pub suggestion: Option<SolidificationSuggestion>,

    // Hot reload
    pub watcher_config: Option<SkillWatcherConfig>,
    pub reloaded_tools: Arc<Mutex<Vec<MarkdownCliTool>>>,
    pub reload_count: Arc<Mutex<usize>>,
    pub watcher_task: Option<tokio::task::JoinHandle<alephcore::Result<()>>>,

    // Temp directories
    pub temp_dir: Option<TempDir>,
    pub skill_dir: Option<PathBuf>,
}

impl Default for SkillsContext {
    fn default() -> Self {
        Self {
            registry: None,
            rpc_response: None,
            loaded_tools: Vec::new(),
            load_errors: Vec::new(),
            tool_server: None,
            generator: None,
            generated_skill_path: None,
            generated_content: None,
            suggestion: None,
            watcher_config: None,
            reloaded_tools: Arc::new(Mutex::new(Vec::new())),
            reload_count: Arc::new(Mutex::new(0)),
            watcher_task: None,
            temp_dir: None,
            skill_dir: None,
        }
    }
}

impl std::fmt::Debug for SkillsContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillsContext")
            .field("registry", &self.registry.as_ref().map(|_| "HandlerRegistry"))
            .field("rpc_response", &self.rpc_response.as_ref().map(|r| r.is_success()))
            .field("loaded_tools_count", &self.loaded_tools.len())
            .field("load_errors_count", &self.load_errors.len())
            .field("tool_server", &self.tool_server.as_ref().map(|_| "AlephToolServer"))
            .field("generator", &self.generator.as_ref().map(|_| "MarkdownSkillGenerator"))
            .field("generated_skill_path", &self.generated_skill_path)
            .field("suggestion", &self.suggestion.as_ref().map(|s| &s.suggested_name))
            .field("watcher_config", &self.watcher_config)
            .field("reload_count", &*self.reload_count.lock().unwrap())
            .field("temp_dir", &self.temp_dir.as_ref().map(|t| t.path()))
            .field("skill_dir", &self.skill_dir)
            .finish()
    }
}

impl Drop for SkillsContext {
    fn drop(&mut self) {
        // Abort any running watcher task
        if let Some(task) = self.watcher_task.take() {
            task.abort();
        }
    }
}

impl SkillsContext {
    /// Create a callback for hot reload tests
    pub fn create_reload_callback(&self) -> ReloadCallback {
        let tools = self.reloaded_tools.clone();
        let count = self.reload_count.clone();
        Arc::new(move |new_tools| {
            let mut reloaded = tools.lock().unwrap();
            reloaded.extend(new_tools);
            *count.lock().unwrap() += 1;
            Ok(())
        })
    }

    /// Create a counting callback for hot reload tests
    pub fn create_counting_callback(&self) -> ReloadCallback {
        let count = self.reload_count.clone();
        Arc::new(move |tools| {
            *count.lock().unwrap() += tools.len();
            Ok(())
        })
    }

    /// Create a suggestion for skill generation tests
    pub fn create_test_suggestion(
        &self,
        pattern_id: &str,
        name: &str,
        description: &str,
        confidence: f32,
    ) -> SolidificationSuggestion {
        let mut metrics = SkillMetrics::new(pattern_id);
        metrics.total_executions = 5;
        metrics.successful_executions = 5;

        SolidificationSuggestion {
            pattern_id: pattern_id.to_string(),
            suggested_name: name.to_string(),
            suggested_description: description.to_string(),
            confidence,
            metrics,
            sample_contexts: vec![],
            instructions_preview: String::new(),
        }
    }
}

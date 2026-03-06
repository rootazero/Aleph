//! Prompt Hook System v2 — 4 types of hooks
//!
//! Extends the original single-trait `PromptHook` with four specialized hook
//! types, each targeting a distinct phase of the prompt pipeline:
//!
//! | Hook | Phase | Mutability |
//! |------|-------|------------|
//! | `BootstrapHook` | Workspace file loading | Full control (replace/reorder/filter) |
//! | `ExtraFilesHook` | Post-bootstrap | Append-only |
//! | `PromptBuildHook` | Prompt assembly | Before/after injection |
//! | `BudgetHook` | Budget resolution | Runtime budget adjustment |
//!
//! All hooks are registered in [`PromptHookRegistry`] which the prompt pipeline
//! queries at each phase.

use std::path::PathBuf;

use crate::error::Result;
use crate::thinker::inbound_context::InboundContext;
use crate::thinker::prompt_budget::TokenBudget;
use crate::thinker::prompt_builder::PromptConfig;
use crate::thinker::workspace_files::WorkspaceFile;

// ---------------------------------------------------------------------------
// Hook 1: BootstrapHook — full control over workspace files
// ---------------------------------------------------------------------------

/// Context passed to [`BootstrapHook::on_bootstrap`].
///
/// The hook may inspect, reorder, filter, or replace the loaded workspace
/// files before they are injected into the system prompt.
pub struct BootstrapHookContext {
    /// Workspace root directory.
    pub workspace_dir: PathBuf,
    /// Session key for the current request.
    pub session_key: String,
    /// Channel kind (e.g. "telegram", "discord", "cli").
    pub channel: String,
    /// Loaded workspace files — hooks may mutate this list freely.
    pub files: Vec<WorkspaceFile>,
}

/// Hook with full control over workspace file loading.
///
/// Runs during the bootstrap phase, before files are injected into the prompt.
/// Implementations may filter, reorder, or replace the loaded workspace files.
pub trait BootstrapHook: Send + Sync {
    /// Human-readable name for logging/debugging.
    fn name(&self) -> &str;

    /// Mutate the bootstrap context (primarily `ctx.files`).
    fn on_bootstrap(&self, ctx: &mut BootstrapHookContext) -> Result<()>;
}

// ---------------------------------------------------------------------------
// Hook 2: ExtraFilesHook — append-only extra files
// ---------------------------------------------------------------------------

/// Read-only context for [`ExtraFilesHook::extra_files`].
pub struct ExtraFilesContext {
    /// Workspace root directory.
    pub workspace_dir: PathBuf,
    /// Session key for the current request.
    pub session_key: String,
    /// Channel kind (e.g. "telegram", "discord", "cli").
    pub channel: String,
}

/// A file to append to the prompt, produced by an [`ExtraFilesHook`].
#[derive(Debug, Clone)]
pub struct ExtraFile {
    /// Logical path or label (e.g. "plugins/greeting.md").
    pub path: String,
    /// File content to inject.
    pub content: String,
}

/// Hook that appends additional files to the prompt.
///
/// Runs after bootstrap. Cannot modify existing files — only append new ones.
pub trait ExtraFilesHook: Send + Sync {
    /// Human-readable name for logging/debugging.
    fn name(&self) -> &str;

    /// Return extra files to append to the prompt.
    fn extra_files(&self, ctx: &ExtraFilesContext) -> Result<Vec<ExtraFile>>;
}

// ---------------------------------------------------------------------------
// Hook 3: PromptBuildHook — before/after prompt assembly
// ---------------------------------------------------------------------------

/// Mutable context passed to [`PromptBuildHook::before_build`].
///
/// Hooks may tweak the prompt config or inject content into the prepend
/// buffer or system prompt override.
pub struct PromptBuildContext {
    /// Prompt configuration — hooks may modify fields (language, persona, etc.).
    pub config: PromptConfig,
    /// Inbound request context (sender, channel, session).
    pub inbound: InboundContext,
    /// Content prepended before the assembled prompt.
    pub prepend_context: String,
    /// When set, replaces the entire assembled system prompt.
    pub system_prompt_override: Option<String>,
}

/// Hook that runs before and/or after prompt assembly.
///
/// - `before_build`: modify config, inject prepend context, or override prompt.
/// - `after_build`: post-process the final prompt string.
///
/// Both methods have default no-op implementations so you can override only
/// the phase you need.
pub trait PromptBuildHook: Send + Sync {
    /// Human-readable name for logging/debugging.
    fn name(&self) -> &str;

    /// Called before prompt assembly. Modify `ctx` to influence the build.
    fn before_build(&self, _ctx: &mut PromptBuildContext) -> Result<()> {
        Ok(())
    }

    /// Called after prompt assembly. Modify the final prompt string in place.
    fn after_build(&self, _prompt: &mut String) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Hook 4: BudgetHook — runtime budget adjustment
// ---------------------------------------------------------------------------

/// Context for [`BudgetHook::adjust_budget`].
pub struct BudgetHookContext {
    /// Session key for the current request.
    pub session_key: String,
    /// Channel kind (e.g. "telegram", "discord", "cli").
    pub channel: String,
    /// Current token budget before adjustment.
    pub current_budget: TokenBudget,
}

/// Overrides returned by a [`BudgetHook`].
///
/// `None` fields leave the current value unchanged.
#[derive(Debug, Clone, Default)]
pub struct BudgetOverride {
    /// Override per-file max chars.
    pub per_file_max_chars: Option<usize>,
    /// Override total max chars for the system prompt.
    pub total_max_chars: Option<usize>,
}

/// Hook for runtime budget adjustment.
///
/// Runs before workspace file loading. Allows per-channel or per-session
/// budget customization (e.g. smaller budget for Telegram, larger for CLI).
pub trait BudgetHook: Send + Sync {
    /// Human-readable name for logging/debugging.
    fn name(&self) -> &str;

    /// Return budget overrides. `None` fields leave the current value unchanged.
    fn adjust_budget(&self, ctx: &BudgetHookContext) -> Result<BudgetOverride>;
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Central registry for all prompt hook types.
///
/// The prompt pipeline queries this registry at each phase to run the
/// registered hooks in insertion order.
pub struct PromptHookRegistry {
    bootstrap_hooks: Vec<Box<dyn BootstrapHook>>,
    extra_files_hooks: Vec<Box<dyn ExtraFilesHook>>,
    prompt_build_hooks: Vec<Box<dyn PromptBuildHook>>,
    budget_hooks: Vec<Box<dyn BudgetHook>>,
}

impl PromptHookRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            bootstrap_hooks: Vec::new(),
            extra_files_hooks: Vec::new(),
            prompt_build_hooks: Vec::new(),
            budget_hooks: Vec::new(),
        }
    }

    /// Register a bootstrap hook.
    pub fn register_bootstrap(&mut self, hook: Box<dyn BootstrapHook>) {
        self.bootstrap_hooks.push(hook);
    }

    /// Register an extra-files hook.
    pub fn register_extra_files(&mut self, hook: Box<dyn ExtraFilesHook>) {
        self.extra_files_hooks.push(hook);
    }

    /// Register a prompt-build hook.
    pub fn register_prompt_build(&mut self, hook: Box<dyn PromptBuildHook>) {
        self.prompt_build_hooks.push(hook);
    }

    /// Register a budget hook.
    pub fn register_budget(&mut self, hook: Box<dyn BudgetHook>) {
        self.budget_hooks.push(hook);
    }

    /// All registered bootstrap hooks (in insertion order).
    pub fn bootstrap_hooks(&self) -> &[Box<dyn BootstrapHook>] {
        &self.bootstrap_hooks
    }

    /// All registered extra-files hooks (in insertion order).
    pub fn extra_files_hooks(&self) -> &[Box<dyn ExtraFilesHook>] {
        &self.extra_files_hooks
    }

    /// All registered prompt-build hooks (in insertion order).
    pub fn prompt_build_hooks(&self) -> &[Box<dyn PromptBuildHook>] {
        &self.prompt_build_hooks
    }

    /// All registered budget hooks (in insertion order).
    pub fn budget_hooks(&self) -> &[Box<dyn BudgetHook>] {
        &self.budget_hooks
    }

    /// Total number of registered hooks across all types.
    pub fn total_count(&self) -> usize {
        self.bootstrap_hooks.len()
            + self.extra_files_hooks.len()
            + self.prompt_build_hooks.len()
            + self.budget_hooks.len()
    }
}

impl Default for PromptHookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Stub implementations ---

    struct StubBootstrapHook;
    impl BootstrapHook for StubBootstrapHook {
        fn name(&self) -> &str {
            "stub_bootstrap"
        }
        fn on_bootstrap(&self, ctx: &mut BootstrapHookContext) -> Result<()> {
            // Remove all files as a test action
            ctx.files.clear();
            Ok(())
        }
    }

    struct StubExtraFilesHook;
    impl ExtraFilesHook for StubExtraFilesHook {
        fn name(&self) -> &str {
            "stub_extra_files"
        }
        fn extra_files(&self, _ctx: &ExtraFilesContext) -> Result<Vec<ExtraFile>> {
            Ok(vec![ExtraFile {
                path: "extra/greeting.md".to_string(),
                content: "Hello from hook!".to_string(),
            }])
        }
    }

    struct StubPromptBuildHook;
    impl PromptBuildHook for StubPromptBuildHook {
        fn name(&self) -> &str {
            "stub_prompt_build"
        }
        fn before_build(&self, ctx: &mut PromptBuildContext) -> Result<()> {
            ctx.config.language = Some("Japanese".to_string());
            Ok(())
        }
        fn after_build(&self, prompt: &mut String) -> Result<()> {
            prompt.push_str("\n<!-- hook footer -->");
            Ok(())
        }
    }

    struct StubBudgetHook;
    impl BudgetHook for StubBudgetHook {
        fn name(&self) -> &str {
            "stub_budget"
        }
        fn adjust_budget(&self, _ctx: &BudgetHookContext) -> Result<BudgetOverride> {
            Ok(BudgetOverride {
                per_file_max_chars: Some(5_000),
                total_max_chars: Some(50_000),
            })
        }
    }

    // --- Registry tests ---

    #[test]
    fn registry_starts_empty() {
        let reg = PromptHookRegistry::new();
        assert_eq!(reg.total_count(), 0);
        assert!(reg.bootstrap_hooks().is_empty());
        assert!(reg.extra_files_hooks().is_empty());
        assert!(reg.prompt_build_hooks().is_empty());
        assert!(reg.budget_hooks().is_empty());
    }

    #[test]
    fn registry_default_is_empty() {
        let reg = PromptHookRegistry::default();
        assert_eq!(reg.total_count(), 0);
    }

    #[test]
    fn register_and_count_all_types() {
        let mut reg = PromptHookRegistry::new();
        reg.register_bootstrap(Box::new(StubBootstrapHook));
        reg.register_extra_files(Box::new(StubExtraFilesHook));
        reg.register_prompt_build(Box::new(StubPromptBuildHook));
        reg.register_budget(Box::new(StubBudgetHook));

        assert_eq!(reg.bootstrap_hooks().len(), 1);
        assert_eq!(reg.extra_files_hooks().len(), 1);
        assert_eq!(reg.prompt_build_hooks().len(), 1);
        assert_eq!(reg.budget_hooks().len(), 1);
        assert_eq!(reg.total_count(), 4);
    }

    #[test]
    fn register_multiple_hooks_per_type() {
        let mut reg = PromptHookRegistry::new();
        reg.register_bootstrap(Box::new(StubBootstrapHook));
        reg.register_bootstrap(Box::new(StubBootstrapHook));
        reg.register_bootstrap(Box::new(StubBootstrapHook));

        assert_eq!(reg.bootstrap_hooks().len(), 3);
        assert_eq!(reg.total_count(), 3);
    }

    // --- Hook behavior tests ---

    #[test]
    fn bootstrap_hook_clears_files() {
        let hook = StubBootstrapHook;
        assert_eq!(hook.name(), "stub_bootstrap");

        let mut ctx = BootstrapHookContext {
            workspace_dir: PathBuf::from("/tmp/ws"),
            session_key: "test:session".to_string(),
            channel: "cli".to_string(),
            files: vec![WorkspaceFile {
                name: "SOUL.md",
                content: Some("soul content".to_string()),
                truncated: false,
                original_size: 12,
            }],
        };

        hook.on_bootstrap(&mut ctx).unwrap();
        assert!(ctx.files.is_empty());
    }

    #[test]
    fn extra_files_hook_returns_files() {
        let hook = StubExtraFilesHook;
        assert_eq!(hook.name(), "stub_extra_files");

        let ctx = ExtraFilesContext {
            workspace_dir: PathBuf::from("/tmp/ws"),
            session_key: "test:session".to_string(),
            channel: "telegram".to_string(),
        };

        let files = hook.extra_files(&ctx).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "extra/greeting.md");
        assert_eq!(files[0].content, "Hello from hook!");
    }

    #[test]
    fn prompt_build_hook_before_and_after() {
        let hook = StubPromptBuildHook;
        assert_eq!(hook.name(), "stub_prompt_build");

        let mut ctx = PromptBuildContext {
            config: PromptConfig::default(),
            inbound: InboundContext::default(),
            prepend_context: String::new(),
            system_prompt_override: None,
        };

        // before_build modifies config
        hook.before_build(&mut ctx).unwrap();
        assert_eq!(ctx.config.language.as_deref(), Some("Japanese"));

        // after_build appends to prompt
        let mut prompt = "base prompt".to_string();
        hook.after_build(&mut prompt).unwrap();
        assert!(prompt.ends_with("<!-- hook footer -->"));
    }

    #[test]
    fn prompt_build_hook_defaults_are_noop() {
        struct NoOpBuildHook;
        impl PromptBuildHook for NoOpBuildHook {
            fn name(&self) -> &str {
                "noop"
            }
        }

        let hook = NoOpBuildHook;
        let mut ctx = PromptBuildContext {
            config: PromptConfig::default(),
            inbound: InboundContext::default(),
            prepend_context: String::new(),
            system_prompt_override: None,
        };
        hook.before_build(&mut ctx).unwrap();
        assert!(ctx.config.language.is_none()); // unchanged

        let mut prompt = "test".to_string();
        hook.after_build(&mut prompt).unwrap();
        assert_eq!(prompt, "test"); // unchanged
    }

    #[test]
    fn budget_hook_returns_overrides() {
        let hook = StubBudgetHook;
        assert_eq!(hook.name(), "stub_budget");

        let ctx = BudgetHookContext {
            session_key: "test:session".to_string(),
            channel: "cli".to_string(),
            current_budget: TokenBudget::default(),
        };

        let overrides = hook.adjust_budget(&ctx).unwrap();
        assert_eq!(overrides.per_file_max_chars, Some(5_000));
        assert_eq!(overrides.total_max_chars, Some(50_000));
    }

    #[test]
    fn budget_override_default_is_all_none() {
        let bo = BudgetOverride::default();
        assert!(bo.per_file_max_chars.is_none());
        assert!(bo.total_max_chars.is_none());
    }
}

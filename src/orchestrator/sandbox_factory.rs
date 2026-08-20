//! Per-session Sandbox allocator. See design §6.
//!
//! `build_sandbox()` (Phase 3) hands out one shared `Arc<dyn Sandbox>`; the
//! Orchestrator wants a per-session handle, so the workspace builder is
//! wrapped in a closure keyed by `session_key`.
//!
//! # Why there is no per-flow sandbox *kind*
//!
//! `FlowSpec` used to carry a `sandbox_kind` axis (`none` | `workspace`) whose
//! `none` arm produced a `DenyAllSandbox` refusing every `execute()`. It never
//! denied anything: the sandbox picked here reaches exactly one consumer —
//! `prompt_build::build_system_prompt`, which calls `.summary()` on it for the
//! `<environment>` block (`runner_impl.rs`). `HarnessDeps` has no sandbox
//! field, so tools always ran under the boot sandbox
//! (`builder/constructor/mod.rs`). The axis was worse than inert: `DenyAllSandbox`
//! did not override `summary()`, so the one flow declaring `none` was also the
//! only flow that told the model *nothing* about its execution envelope.
//!
//! It is deliberately not reconnected. "Which tools may execute" already has
//! three live answers — the boot `[sandbox]` block, `exec_tier`, and
//! `[policies.tool_permissions]` — and CLAUDE.md fixes the enforcement point:
//! **`src/tools/scoped/` is the only one**. A fourth answer here would be a
//! bypass by construction.

use crate::sync_primitives::Arc;

use crate::orchestrator::errors::FlowError;
use crate::sandbox::Sandbox;

pub type WorkspaceBuilder = Arc<dyn Fn(&str) -> Result<Arc<dyn Sandbox>, String> + Send + Sync>;

pub type SandboxFactory = Arc<dyn Fn(&str) -> Result<Arc<dyn Sandbox>, FlowError> + Send + Sync>;

pub fn build_sandbox_factory(workspace: WorkspaceBuilder) -> SandboxFactory {
    Arc::new(move |session_key| workspace(session_key).map_err(FlowError::SandboxProvisionFailed))
}

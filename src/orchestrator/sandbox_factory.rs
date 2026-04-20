//! Per-session Sandbox allocator. See design §6.
//!
//! Phase 3 exposes `build_sandbox()` returning a shared `Arc<dyn Sandbox>`.
//! Orchestrator needs per-session provisioning, so we wrap the Workspace
//! builder in a closure that also knows how to produce `DenyAllSandbox`
//! for `SandboxKind::None`.

use std::sync::Arc;

use async_trait::async_trait;

use crate::orchestrator::errors::FlowError;
use crate::orchestrator::flow_spec::SandboxKind;
use crate::sandbox::{Sandbox, SandboxCommand, SandboxError, SandboxOutput};

pub type WorkspaceBuilder =
    Arc<dyn Fn(&str) -> Result<Arc<dyn Sandbox>, String> + Send + Sync>;

pub type SandboxFactory =
    Arc<dyn Fn(SandboxKind, &str) -> Result<Arc<dyn Sandbox>, FlowError> + Send + Sync>;

pub fn build_sandbox_factory(workspace: WorkspaceBuilder) -> SandboxFactory {
    Arc::new(move |kind, session_key| match kind {
        SandboxKind::None => Ok(Arc::new(DenyAllSandbox::new()) as Arc<dyn Sandbox>),
        SandboxKind::Workspace => {
            workspace(session_key).map_err(FlowError::SandboxProvisionFailed)
        }
    })
}

/// Sandbox that denies every `execute()` call. Used for flows declared
/// `sandbox_kind = "none"` — exec-class tools must not run at all.
///
/// Distinct from `crate::sandbox::NoopSandbox`, which signals a
/// misconfigured sandbox subsystem. This one signals intentional denial.
#[derive(Debug, Default)]
pub struct DenyAllSandbox;

impl DenyAllSandbox {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Sandbox for DenyAllSandbox {
    async fn execute(&self, _cmd: SandboxCommand) -> Result<SandboxOutput, SandboxError> {
        // Phase 5: adapted to existing SandboxError::CapabilityDenied variant.
        Err(SandboxError::CapabilityDenied {
            reason: "SandboxKind::None flow denied exec-class tool".into(),
        })
    }
}

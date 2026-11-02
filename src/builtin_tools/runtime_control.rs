//! Memory lifecycle controls deliberately require no Memory admission.

use crate::error::AlephError;
use crate::memory::runtime::{MemoryRuntime, MemoryStatus};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeControlArgs {}

#[derive(Clone)]
pub struct MemoryRuntimeStatusTool {
    runtime: Arc<MemoryRuntime>,
}

impl MemoryRuntimeStatusTool {
    pub fn new(runtime: Arc<MemoryRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl AlephTool for MemoryRuntimeStatusTool {
    const NAME: &'static str = "memory_runtime_status";
    const DESCRIPTION: &'static str = "Report the Memory runtime state without starting it.";
    type Args = RuntimeControlArgs;
    type Output = MemoryStatus;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, AlephError> {
        Ok(self.runtime.status())
    }
}

#[derive(Clone)]
pub struct MemoryRuntimeStartTool {
    runtime: Arc<MemoryRuntime>,
}

impl MemoryRuntimeStartTool {
    pub fn new(runtime: Arc<MemoryRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl AlephTool for MemoryRuntimeStartTool {
    const NAME: &'static str = "memory_runtime_start";
    const DESCRIPTION: &'static str =
        "Start Memory indexing and retrieval using the current configuration.";
    type Args = RuntimeControlArgs;
    type Output = MemoryStatus;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, AlephError> {
        self.runtime.start().await
    }
}

#[derive(Clone)]
pub struct MemoryRuntimeStopTool {
    runtime: Arc<MemoryRuntime>,
}

impl MemoryRuntimeStopTool {
    pub fn new(runtime: Arc<MemoryRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl AlephTool for MemoryRuntimeStopTool {
    const NAME: &'static str = "memory_runtime_stop";
    const DESCRIPTION: &'static str = "Stop new Memory work, drain admitted work, and close derived resources. Chat and raw sessions continue.";
    type Args = RuntimeControlArgs;
    type Output = MemoryStatus;

    async fn call(&self, _args: Self::Args) -> Result<Self::Output, AlephError> {
        self.runtime.stop().await
    }
}

//! Tool progress callback adapter for FFI

use crate::ffi::AetherEventHandler;
use crate::rig_tools::ToolProgressCallback;
use std::sync::Arc;

/// Adapter to bridge tool progress callbacks to the FFI event handler
///
/// This enables real-time streaming of tool execution progress during agent operations.
pub struct FfiToolProgressAdapter {
    handler: Arc<dyn AetherEventHandler>,
}

impl FfiToolProgressAdapter {
    pub fn new(handler: Arc<dyn AetherEventHandler>) -> Self {
        Self { handler }
    }
}

impl ToolProgressCallback for FfiToolProgressAdapter {
    fn on_tool_start(&self, tool_name: &str, args_summary: &str) {
        // Stream tool start as a formatted message
        let message = format!("\n**[工具]** {} - {}\n", tool_name, args_summary);
        self.handler.on_stream_chunk(message);
        // Also notify via dedicated tool callback
        self.handler.on_tool_start(tool_name.to_string());
    }

    fn on_tool_result(&self, tool_name: &str, result_summary: &str, success: bool) {
        // Stream result as a formatted message
        let status = if success { "✓" } else { "✗" };
        let message = format!("**[{}]** {}: {}\n", status, tool_name, result_summary);
        self.handler.on_stream_chunk(message);
        // Also notify via dedicated tool callback
        self.handler
            .on_tool_result(tool_name.to_string(), result_summary.to_string());
    }
}

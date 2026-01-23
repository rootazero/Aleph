//! Agent state management FFI methods
//!
//! Contains state control and subscription:
//! - agent_get_state
//! - agent_pause, agent_resume, agent_cancel
//! - agent_is_paused, agent_is_cancelled
//! - agent_subscribe

use crate::ffi::AetherCore;
use std::sync::Arc;
use tracing::info;

impl AetherCore {
    /// Get current execution state
    pub fn agent_get_state(&self) -> crate::ffi::dispatcher_types::AgentExecutionState {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            self.runtime.block_on(async {
                crate::ffi::dispatcher_types::AgentExecutionState::from(engine.state().await)
            })
        } else {
            crate::ffi::dispatcher_types::AgentExecutionState::Idle
        }
    }

    /// Pause execution
    pub fn agent_pause(&self) {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.pause();
            info!("Cowork execution paused");
        }
    }

    /// Resume execution
    pub fn agent_resume(&self) {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.resume();
            info!("Cowork execution resumed");
        }
    }

    /// Cancel execution
    pub fn agent_cancel(&self) {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.cancel();
            info!("Cowork execution cancelled");
        }
    }

    /// Check if execution is paused
    pub fn agent_is_paused(&self) -> bool {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.is_paused()
        } else {
            false
        }
    }

    /// Check if execution is cancelled
    pub fn agent_is_cancelled(&self) -> bool {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            engine.is_cancelled()
        } else {
            false
        }
    }

    /// Subscribe to progress events
    pub fn agent_subscribe(&self, handler: Box<dyn crate::ffi::dispatcher_types::AgentProgressHandler>) {
        if let Ok(engine) = self.get_or_create_agent_engine() {
            // Convert Box to Arc for internal use
            let handler_arc: Arc<dyn crate::ffi::dispatcher_types::AgentProgressHandler> = Arc::from(handler);
            let subscriber = Arc::new(crate::ffi::dispatcher_types::FfiProgressSubscriber::new(handler_arc));
            engine.subscribe(subscriber);
            info!("Cowork progress subscriber added");
        }
    }
}

//! Command Dispatcher
//!
//! Executes Direct-mode commands without going through Agent Loop.

// Not yet wired into the dispatch path; retained as the intended handler abstraction.
#![allow(dead_code)]

use async_trait::async_trait;
use std::collections::HashMap;

use super::types::CommandExecutionResult;

/// Handler for a direct-mode command
#[async_trait]
pub(crate) trait DirectHandler: Send + Sync {
    /// Execute the command with optional arguments
    async fn execute(&self, args: Option<&str>) -> CommandExecutionResult;
}

/// Dispatches Direct-mode commands to their handlers
///
/// **Note**: This dispatcher uses a standard `HashMap` and is **not** designed
/// for concurrent registration. `register` requires `&mut self`, while `execute`
/// only needs `&self`. If you need runtime handler registration from multiple
/// threads/tasks, wrap this dispatcher in an `RwLock` or `Mutex`.
pub(crate) struct CommandDispatcher {
    handlers: HashMap<String, Box<dyn DirectHandler>>,
}

impl CommandDispatcher {
    /// Create a new empty dispatcher
    pub(crate) fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a command name
    pub(crate) fn register(
        &mut self,
        name: impl Into<String>,
        handler: impl DirectHandler + 'static,
    ) {
        self.handlers.insert(name.into(), Box::new(handler));
    }

    /// Execute a direct command by name
    pub(crate) async fn execute(
        &self,
        command_name: &str,
        args: Option<&str>,
    ) -> CommandExecutionResult {
        match self.handlers.get(command_name) {
            Some(handler) => handler.execute(args).await,
            None => CommandExecutionResult::error(format!(
                "No direct handler registered for '{}'",
                command_name
            )),
        }
    }

    /// Check if a handler exists for a command
    pub(crate) fn has_handler(&self, command_name: &str) -> bool {
        self.handlers.contains_key(command_name)
    }
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockHandler {
        response: String,
    }

    #[async_trait]
    impl DirectHandler for MockHandler {
        async fn execute(&self, args: Option<&str>) -> CommandExecutionResult {
            let msg = match args {
                Some(a) => format!("{}: {}", self.response, a),
                None => self.response.clone(),
            };
            CommandExecutionResult::success(msg)
        }
    }

    #[tokio::test]
    async fn test_dispatch_registered_handler() {
        let mut dispatcher = CommandDispatcher::new();
        dispatcher.register(
            "help",
            MockHandler {
                response: "Help output".to_string(),
            },
        );

        let result = dispatcher.execute("help", None).await;
        assert!(result.success);
        assert_eq!(result.message, "Help output");
    }

    #[tokio::test]
    async fn test_dispatch_with_args() {
        let mut dispatcher = CommandDispatcher::new();
        dispatcher.register(
            "echo",
            MockHandler {
                response: "Echo".to_string(),
            },
        );

        let result = dispatcher.execute("echo", Some("hello")).await;
        assert!(result.success);
        assert_eq!(result.message, "Echo: hello");
    }

    #[tokio::test]
    async fn test_dispatch_unknown_command() {
        let dispatcher = CommandDispatcher::new();
        let result = dispatcher.execute("nonexistent", None).await;
        assert!(!result.success);
        assert!(result.message.contains("No direct handler"));
    }

    #[tokio::test]
    async fn test_has_handler() {
        let mut dispatcher = CommandDispatcher::new();
        dispatcher.register(
            "help",
            MockHandler {
                response: "Help".to_string(),
            },
        );

        assert!(dispatcher.has_handler("help"));
        assert!(!dispatcher.has_handler("unknown"));
    }
}

//! Request types for the orchestrator

use crate::dispatcher::UnifiedTool;

/// Context for the request
#[derive(Debug, Clone, Default)]
pub struct RequestContext {
    /// Selected file path (if any)
    pub selected_file: Option<String>,
    /// Active application name
    pub active_app: Option<String>,
    /// Current UI mode
    pub ui_mode: Option<String>,
    /// Clipboard content type
    pub clipboard_type: Option<String>,
}

impl RequestContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self::default()
    }

    /// Create context with a selected file
    pub fn with_file(file_path: impl Into<String>) -> Self {
        Self {
            selected_file: Some(file_path.into()),
            ..Default::default()
        }
    }

    /// Create context with an active app
    pub fn with_app(app_name: impl Into<String>) -> Self {
        Self {
            active_app: Some(app_name.into()),
            ..Default::default()
        }
    }

    /// Builder: set selected file
    pub fn selected_file(mut self, path: impl Into<String>) -> Self {
        self.selected_file = Some(path.into());
        self
    }

    /// Builder: set active app
    pub fn active_app(mut self, app: impl Into<String>) -> Self {
        self.active_app = Some(app.into());
        self
    }

    /// Builder: set UI mode
    pub fn ui_mode(mut self, mode: impl Into<String>) -> Self {
        self.ui_mode = Some(mode.into());
        self
    }

    /// Builder: set clipboard type
    pub fn clipboard_type(mut self, clip_type: impl Into<String>) -> Self {
        self.clipboard_type = Some(clip_type.into());
        self
    }

    /// Create context from FFI options (app_context and window_title)
    ///
    /// This factory method creates a RequestContext from the fields
    /// typically available in ProcessOptions without creating a dependency.
    pub fn from_ffi_options(
        app_context: Option<String>,
        window_title: Option<String>,
    ) -> Option<Self> {
        // Only create context if we have meaningful data
        if app_context.is_none() && window_title.is_none() {
            return None;
        }

        Some(Self {
            selected_file: None,
            active_app: app_context,
            ui_mode: None,
            clipboard_type: None,
        })
    }
}

/// Request to the orchestrator
#[derive(Debug, Clone)]
pub struct OrchestratorRequest {
    /// User input text
    pub input: String,
    /// Optional context signals
    pub context: Option<RequestContext>,
    /// Available tools for execution
    pub tools: Vec<UnifiedTool>,
}

impl OrchestratorRequest {
    /// Create a new request with just input
    pub fn new(input: impl Into<String>) -> Self {
        Self {
            input: input.into(),
            context: None,
            tools: Vec::new(),
        }
    }

    /// Builder: set context
    pub fn with_context(mut self, context: RequestContext) -> Self {
        self.context = Some(context);
        self
    }

    /// Builder: set tools
    pub fn with_tools(mut self, tools: Vec<UnifiedTool>) -> Self {
        self.tools = tools;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_context_builder() {
        let ctx = RequestContext::new()
            .selected_file("/path/to/file.txt")
            .active_app("Finder")
            .ui_mode("agent")
            .clipboard_type("text");

        assert_eq!(ctx.selected_file, Some("/path/to/file.txt".to_string()));
        assert_eq!(ctx.active_app, Some("Finder".to_string()));
        assert_eq!(ctx.ui_mode, Some("agent".to_string()));
        assert_eq!(ctx.clipboard_type, Some("text".to_string()));
    }

    #[test]
    fn test_orchestrator_request_builder() {
        let request = OrchestratorRequest::new("test input")
            .with_context(RequestContext::with_file("/test.jpg"));

        assert_eq!(request.input, "test input");
        assert!(request.context.is_some());
        assert_eq!(
            request.context.unwrap().selected_file,
            Some("/test.jpg".to_string())
        );
    }
}

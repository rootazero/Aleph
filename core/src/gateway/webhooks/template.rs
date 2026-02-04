//! Session Key Template Renderer
//!
//! Renders session key templates with variable substitution for webhook routing.

use std::collections::HashMap;

/// Template context for session key rendering
#[derive(Debug, Clone, Default)]
pub struct TemplateContext {
    variables: HashMap<String, String>,
}

impl TemplateContext {
    /// Create a new empty context
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a context with webhook-specific variables
    pub fn for_webhook(
        webhook_id: &str,
        event_type: Option<&str>,
        source_id: Option<&str>,
    ) -> Self {
        let mut ctx = Self::new();
        ctx.set("webhook_id", webhook_id);
        if let Some(event) = event_type {
            ctx.set("event_type", event);
        }
        if let Some(source) = source_id {
            ctx.set("source_id", source);
        }
        ctx
    }

    /// Set a variable
    pub fn set(&mut self, key: &str, value: &str) -> &mut Self {
        self.variables.insert(key.to_string(), value.to_string());
        self
    }

    /// Get a variable
    pub fn get(&self, key: &str) -> Option<&str> {
        self.variables.get(key).map(|s| s.as_str())
    }

    /// Add a variable (chainable)
    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.set(key, value);
        self
    }
}

/// Render a session key template
///
/// Substitutes `{variable}` patterns with values from the context.
/// Unknown variables are replaced with empty strings.
///
/// # Examples
///
/// ```ignore
/// let ctx = TemplateContext::for_webhook("github", Some("push"), None);
/// let key = render_template("task:webhook:{webhook_id}:{event_type}", &ctx);
/// assert_eq!(key, "task:webhook:github:push");
/// ```
pub fn render_template(template: &str, context: &TemplateContext) -> String {
    let mut result = template.to_string();
    let mut start = 0;

    while let Some(open) = result[start..].find('{') {
        let open_pos = start + open;
        if let Some(close) = result[open_pos..].find('}') {
            let close_pos = open_pos + close;
            let var_name = &result[open_pos + 1..close_pos];

            let value = context.get(var_name).unwrap_or("");
            result = format!(
                "{}{}{}",
                &result[..open_pos],
                value,
                &result[close_pos + 1..]
            );

            // Move start past the substituted value
            start = open_pos + value.len();
        } else {
            // No closing brace, skip this open brace
            start = open_pos + 1;
        }
    }

    result
}

/// Validate a template string
///
/// Returns a list of variable names found in the template.
pub fn extract_variables(template: &str) -> Vec<String> {
    let mut variables = Vec::new();
    let chars = template.chars().peekable();
    let mut current_var = String::new();
    let mut in_var = false;

    for c in chars {
        if c == '{' {
            in_var = true;
            current_var.clear();
        } else if c == '}' && in_var {
            if !current_var.is_empty() {
                variables.push(current_var.clone());
            }
            in_var = false;
        } else if in_var {
            current_var.push(c);
        }
    }

    variables
}

/// Standard template variables for webhooks
pub mod vars {
    /// The webhook endpoint ID
    pub const WEBHOOK_ID: &str = "webhook_id";
    /// The event type (e.g., "push", "payment.succeeded")
    pub const EVENT_TYPE: &str = "event_type";
    /// The source identifier (e.g., repo name, customer ID)
    pub const SOURCE_ID: &str = "source_id";
    /// Unix timestamp of the webhook
    pub const TIMESTAMP: &str = "timestamp";
    /// Unique delivery ID
    pub const DELIVERY_ID: &str = "delivery_id";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_simple_template() {
        let ctx = TemplateContext::new().with("name", "test");
        let result = render_template("hello-{name}", &ctx);
        assert_eq!(result, "hello-test");
    }

    #[test]
    fn test_render_multiple_variables() {
        let ctx = TemplateContext::new()
            .with("a", "1")
            .with("b", "2")
            .with("c", "3");
        let result = render_template("{a}-{b}-{c}", &ctx);
        assert_eq!(result, "1-2-3");
    }

    #[test]
    fn test_render_webhook_template() {
        let ctx = TemplateContext::for_webhook("github", Some("push"), Some("myrepo"));
        let result = render_template("task:webhook:{webhook_id}:{event_type}", &ctx);
        assert_eq!(result, "task:webhook:github:push");
    }

    #[test]
    fn test_render_missing_variable() {
        let ctx = TemplateContext::new();
        let result = render_template("task:{missing}", &ctx);
        assert_eq!(result, "task:");
    }

    #[test]
    fn test_render_no_variables() {
        let ctx = TemplateContext::new();
        let result = render_template("static-key", &ctx);
        assert_eq!(result, "static-key");
    }

    #[test]
    fn test_render_repeated_variable() {
        let ctx = TemplateContext::new().with("x", "val");
        let result = render_template("{x}-{x}-{x}", &ctx);
        assert_eq!(result, "val-val-val");
    }

    #[test]
    fn test_extract_variables() {
        let vars = extract_variables("task:{webhook_id}:{event_type}");
        assert_eq!(vars, vec!["webhook_id", "event_type"]);
    }

    #[test]
    fn test_extract_no_variables() {
        let vars = extract_variables("static-key");
        assert!(vars.is_empty());
    }

    #[test]
    fn test_context_for_webhook() {
        let ctx = TemplateContext::for_webhook("test", Some("event"), Some("source"));
        assert_eq!(ctx.get("webhook_id"), Some("test"));
        assert_eq!(ctx.get("event_type"), Some("event"));
        assert_eq!(ctx.get("source_id"), Some("source"));
    }

    #[test]
    fn test_unclosed_brace() {
        let ctx = TemplateContext::new().with("x", "val");
        let result = render_template("test{x", &ctx);
        assert_eq!(result, "test{x");
    }
}

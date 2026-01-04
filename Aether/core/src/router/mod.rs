/// Smart Routing System for Aether
///
/// This module implements regex-based routing that matches user input against
/// configured rules to select the appropriate AI provider.
///
/// # Architecture
///
/// ```
/// User Input → Router → Matching Rule → Provider + System Prompt
/// ```
///
/// # Design Principles
///
/// - **First-match wins**: Rules are evaluated in order, first match is used
/// - **Regex-based**: Flexible pattern matching for complex routing logic
/// - **Fallback support**: Default provider used when no rule matches
/// - **System prompt override**: Each rule can customize the AI's behavior
///
/// # Example
///
/// ```rust,no_run
/// use aethecore::router::Router;
/// use aethecore::config::Config;
///
/// # fn example() -> aethecore::error::Result<()> {
/// let config = Config::default();
/// let router = Router::new(&config)?;
///
/// // Route input to appropriate provider
/// if let Some((provider, system_prompt)) = router.route("/code write a function") {
///     println!("Using provider: {}", provider.name());
///     println!("System prompt: {:?}", system_prompt);
/// }
/// # Ok(())
/// # }
/// ```
pub mod decision;

use crate::config::Config;
use crate::error::{AetherError, Result};
use crate::providers::{create_provider, AiProvider};
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

// Re-export
pub use decision::RoutingDecision;

// Import for tests
#[cfg(test)]
use crate::config::{ProviderConfig, RoutingRuleConfig};

// Type aliases for complex return types
/// Primary provider with system prompt, and optional fallback provider
pub type ProviderWithFallback<'a> = (
    (&'a dyn AiProvider, Option<&'a str>),
    Option<&'a dyn AiProvider>,
);

/// A routing rule that matches input patterns to AI providers
///
/// Each rule consists of:
/// - A compiled regex pattern to match against user input
/// - The name of the provider to use when matched
/// - An optional system prompt to override the default behavior
///
/// # Example
///
/// ```rust,no_run
/// use aethecore::router::RoutingRule;
///
/// # fn example() -> aethecore::error::Result<()> {
/// // Route code-related requests to Claude
/// let rule = RoutingRule::new(
///     r"^/(code|rust|python)",
///     "claude",
///     Some("You are a senior software engineer.")
/// )?;
///
/// assert!(rule.matches("/code write a function"));
/// assert!(rule.matches("/rust implement a trait"));
/// assert!(!rule.matches("Hello, how are you?"));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RoutingRule {
    /// Compiled regex pattern for matching input
    regex: Regex,
    /// Original pattern string (for prefix stripping)
    pattern: String,
    /// Name of the provider to use when this rule matches
    provider_name: String,
    /// Optional system prompt to guide AI behavior
    system_prompt: Option<String>,
    /// Whether to strip the matched prefix from input before sending to AI
    strip_prefix: bool,
}

impl RoutingRule {
    /// Create a new routing rule
    ///
    /// # Arguments
    ///
    /// * `pattern` - Regex pattern string (compiled at creation time)
    /// * `provider_name` - Name of the provider to use
    /// * `system_prompt` - Optional system prompt override
    /// * `strip_prefix` - Whether to strip the matched prefix (None = auto-detect for ^/ patterns)
    ///
    /// # Returns
    ///
    /// * `Ok(RoutingRule)` - Successfully created rule
    /// * `Err(AetherError::InvalidConfig)` - Invalid regex syntax
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::router::RoutingRule;
    /// # fn example() -> aethecore::error::Result<()> {
    /// let rule = RoutingRule::new(
    ///     r"^/draw",
    ///     "openai",
    ///     Some("You are DALL-E. Generate images."),
    ///     None  // Auto-detect: true for ^/ patterns
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(
        pattern: &str,
        provider_name: &str,
        system_prompt: Option<&str>,
        strip_prefix: Option<bool>,
    ) -> Result<Self> {
        let regex = Regex::new(pattern).map_err(|e| {
            AetherError::invalid_config(format!("Invalid regex pattern '{}': {}", pattern, e))
        })?;

        // Auto-detect strip_prefix for command patterns (^/xxx)
        let should_strip = strip_prefix.unwrap_or_else(|| pattern.starts_with("^/"));

        Ok(Self {
            regex,
            pattern: pattern.to_string(),
            provider_name: provider_name.to_string(),
            system_prompt: system_prompt.map(|s| s.to_string()),
            strip_prefix: should_strip,
        })
    }

    /// Check if this rule matches the given input
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text to match against
    ///
    /// # Returns
    ///
    /// `true` if the regex pattern matches the input, `false` otherwise
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::router::RoutingRule;
    /// # fn example() -> aethecore::error::Result<()> {
    /// let rule = RoutingRule::new(r"^/code", "claude", None, None)?;
    ///
    /// assert!(rule.matches("/code write a function"));
    /// assert!(!rule.matches("Hello world"));
    /// # Ok(())
    /// # }
    /// ```
    pub fn matches(&self, input: &str) -> bool {
        self.regex.is_match(input)
    }

    /// Get the provider name for this rule
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Get the system prompt override (if any)
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt.as_deref()
    }

    /// Check if this rule should strip the matched prefix
    pub fn should_strip_prefix(&self) -> bool {
        self.strip_prefix
    }

    /// Strip the matched prefix from input if strip_prefix is enabled
    ///
    /// For command patterns like `^/en`, this removes the `/en` prefix from the input.
    /// The result is trimmed of leading whitespace after stripping.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::router::RoutingRule;
    /// # fn example() -> aethecore::error::Result<()> {
    /// let rule = RoutingRule::new(r"^/en", "openai", None, Some(true))?;
    /// let input = "/en Hello world";
    /// let stripped = rule.strip_matched_prefix(input);
    /// assert_eq!(stripped, "Hello world");
    /// # Ok(())
    /// # }
    /// ```
    pub fn strip_matched_prefix(&self, input: &str) -> String {
        if !self.strip_prefix {
            return input.to_string();
        }

        // Find the match and remove it
        if let Some(mat) = self.regex.find(input) {
            // Remove the matched portion and trim leading whitespace
            let stripped = &input[mat.end()..];
            stripped.trim_start().to_string()
        } else {
            input.to_string()
        }
    }
}

/// Smart router that selects AI providers based on input patterns
///
/// The router maintains:
/// - A list of routing rules (evaluated in order)
/// - A registry of available providers
/// - A fallback default provider
///
/// # Routing Algorithm
///
/// 1. Iterate through rules in order
/// 2. Return the first rule that matches the input
/// 3. If no rule matches, use the default provider
/// 4. If no default provider, return None
///
/// # Example
///
/// ```rust,no_run
/// use aethecore::router::Router;
/// use aethecore::config::Config;
///
/// # async fn example() -> aethecore::error::Result<()> {
/// let config = Config::default();
/// let router = Router::new(&config)?;
///
/// // Route based on prefix
/// if let Some((provider, sys_prompt)) = router.route("/code write a function") {
///     let response = provider.process("write a function", sys_prompt).await?;
///     println!("Response: {}", response);
/// }
/// # Ok(())
/// # }
/// ```
pub struct Router {
    /// Ordered list of routing rules (first match wins)
    rules: Vec<RoutingRule>,
    /// Registry of available providers (name → provider instance)
    providers: HashMap<String, Arc<dyn AiProvider>>,
    /// Optional default provider name (fallback when no rule matches)
    default_provider: Option<String>,
}

impl Router {
    /// Create a new router from configuration
    ///
    /// # Arguments
    ///
    /// * `config` - Application configuration containing providers and rules
    ///
    /// # Returns
    ///
    /// * `Ok(Router)` - Successfully created router
    /// * `Err(AetherError)` - Configuration errors:
    ///   - Invalid provider configuration
    ///   - Invalid regex patterns
    ///   - Missing provider references
    ///
    /// # Configuration Requirements
    ///
    /// - At least one provider must be configured
    /// - All provider references in rules must exist
    /// - Default provider (if specified) must exist
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use aethecore::router::Router;
    /// use aethecore::config::Config;
    ///
    /// # fn example() -> aethecore::error::Result<()> {
    /// let config = Config::default();
    /// let router = Router::new(&config)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(config: &Config) -> Result<Self> {
        // Initialize provider registry
        let mut providers = HashMap::new();

        // Create provider instances from config
        for (name, provider_config) in &config.providers {
            let provider = create_provider(name, provider_config.clone())?;
            providers.insert(name.clone(), provider);
        }

        // Validate at least one provider exists
        if providers.is_empty() {
            return Err(AetherError::invalid_config(
                "No providers configured. At least one provider is required.",
            ));
        }

        // Validate default provider exists (if specified)
        if let Some(ref default_name) = config.general.default_provider {
            if !providers.contains_key(default_name) {
                return Err(AetherError::invalid_config(format!(
                    "Default provider '{}' not found in configured providers",
                    default_name
                )));
            }
        }

        // Load routing rules from config
        let mut rules = Vec::new();
        for rule_config in &config.rules {
            // Create RoutingRule from config
            let rule = RoutingRule::new(
                &rule_config.regex,
                &rule_config.provider,
                rule_config.system_prompt.as_deref(),
                rule_config.strip_prefix,
            )?;

            // Validate that provider exists
            if !providers.contains_key(&rule_config.provider) {
                return Err(AetherError::invalid_config(format!(
                    "Rule references unknown provider '{}'. Available providers: {}",
                    rule_config.provider,
                    providers.keys().cloned().collect::<Vec<_>>().join(", ")
                )));
            }

            rules.push(rule);
        }

        Ok(Self {
            rules,
            providers,
            default_provider: config.general.default_provider.clone(),
        })
    }

    /// Route context to the appropriate AI provider
    ///
    /// # Arguments
    ///
    /// * `context` - The full context string (window context + clipboard content)
    ///   Format: `[AppName] WindowTitle\nClipboardContent`
    ///
    /// # Returns
    ///
    /// * `Some((provider, system_prompt))` - Matched provider and optional system prompt
    /// * `None` - No provider available (no match and no default)
    ///
    /// # Routing Logic (First-Match-Stops)
    ///
    /// 1. Iterate through rules in order (first match wins)
    /// 2. Return first matching rule's provider + system prompt override
    /// 3. If no match, return default provider with no system prompt override
    /// 4. If no default, return None
    ///
    /// # System Prompt Priority
    ///
    /// 1. Rule's `system_prompt` (highest priority) - returned as `Some(&str)`
    /// 2. Provider's default prompt (if rule has no prompt) - returned as `None`, provider uses its default
    /// 3. No prompt at all - returned as `None`
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::router::Router;
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// # let config = Config::default();
    /// let router = Router::new(&config)?;
    ///
    /// // Route with window context
    /// let context = "[VSCode] main.rs\nfn main() {}";
    /// if let Some((provider, sys_prompt)) = router.route(context) {
    ///     println!("Matched provider: {}", provider.name());
    ///     println!("Custom system prompt: {:?}", sys_prompt);
    /// }
    ///
    /// // Route with default fallback
    /// let context = "[Notes] Document.txt\nHello world";
    /// if let Some((provider, _)) = router.route(context) {
    ///     println!("Using default provider: {}", provider.name());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn route(&self, context: &str) -> Option<(&dyn AiProvider, Option<&str>)> {
        info!(
            context_length = context.len(),
            rules_count = self.rules.len(),
            "Starting route decision with full context"
        );

        // Iterate through rules in order (first match wins, subsequent rules ignored)
        for (index, rule) in self.rules.iter().enumerate() {
            debug!(
                rule_index = index,
                pattern = %self.get_rule_pattern(index).unwrap_or("unknown"),
                "Testing rule"
            );

            if rule.matches(context) {
                // Get provider by name
                if let Some(provider) = self.providers.get(rule.provider_name()) {
                    info!(
                        rule_index = index,
                        pattern = %self.get_rule_pattern(index).unwrap_or("unknown"),
                        provider = %rule.provider_name(),
                        has_custom_prompt = rule.system_prompt().is_some(),
                        custom_prompt_preview = ?rule.system_prompt().map(|s| s.chars().take(50).collect::<String>()),
                        "Rule matched (first-match-stops), routing to provider"
                    );
                    // Return provider with rule's system prompt (if specified)
                    // This overrides the provider's default system prompt
                    return Some((provider.as_ref(), rule.system_prompt()));
                }
                // Rule matched but provider not found (should not happen due to validation)
                warn!(
                    provider = %rule.provider_name(),
                    "Rule matched but provider not found in registry"
                );
            }
        }

        // No rule matched, fall back to default provider (if configured)
        info!(
            rules_tested = self.rules.len(),
            has_default = self.default_provider.is_some(),
            "No rule matched, attempting default provider fallback"
        );
        let result = self
            .default_provider
            .as_ref()
            .and_then(|name| self.providers.get(name))
            .map(|provider| (provider.as_ref(), None)); // None = use provider's default prompt

        if let Some((provider, _)) = &result {
            info!(
                provider = %provider.name(),
                "Using default provider (no rule match)"
            );
        } else {
            warn!("No provider available: no rule match and no default provider");
        }

        result
    }

    /// Get the regex pattern for a rule by index (for logging purposes)
    fn get_rule_pattern(&self, index: usize) -> Option<&str> {
        self.rules.get(index).map(|r| r.regex.as_str())
    }

    /// Get the number of configured routing rules
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Get the number of configured providers
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Check if a provider with the given name exists
    pub fn has_provider(&self, name: &str) -> bool {
        self.providers.contains_key(name)
    }

    /// Get the default provider name (if configured)
    pub fn default_provider_name(&self) -> Option<&str> {
        self.default_provider.as_deref()
    }

    /// Route context and provide fallback provider if requested provider fails
    ///
    /// This method returns both the primary provider and a fallback provider
    /// (if different from primary). The fallback is the default provider.
    ///
    /// # Arguments
    ///
    /// * `context` - The full context string (window context + clipboard content)
    ///   Format: `[AppName] WindowTitle\nClipboardContent`
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// * Primary provider and system prompt
    /// * Optional fallback provider (None if same as primary or no default configured)
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::router::Router;
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// # let config = Config::default();
    /// let router = Router::new(&config)?;
    ///
    /// let context = "[VSCode] main.rs\nfn main() {}";
    /// if let Some(((provider, sys_prompt), fallback)) = router.route_with_fallback(context) {
    ///     // Try primary provider
    ///     match try_process(provider, "input", sys_prompt) {
    ///         Ok(response) => println!("Success: {}", response),
    ///         Err(_) => {
    ///             // Try fallback if available
    ///             if let Some(fallback_provider) = fallback {
    ///                 let response = try_process(fallback_provider, "input", None);
    ///                 println!("Fallback success: {:?}", response);
    ///             }
    ///         }
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// # fn try_process(p: &dyn aethecore::providers::AiProvider, i: &str, s: Option<&str>) -> Result<String, ()> { Ok("".into()) }
    /// ```
    pub fn route_with_fallback(&self, context: &str) -> Option<ProviderWithFallback<'_>> {
        // Get primary routing result
        let (primary_provider, system_prompt) = self.route(context)?;
        let primary_name = primary_provider.name();

        // Determine fallback provider
        let fallback = self.default_provider.as_ref().and_then(|default_name| {
            // Only use fallback if it's different from primary
            if default_name != primary_name {
                self.providers
                    .get(default_name)
                    .map(|p| p.as_ref() as &dyn AiProvider)
            } else {
                None
            }
        });

        Some(((primary_provider, system_prompt), fallback))
    }

    /// Get a provider by name for explicit fallback scenarios
    ///
    /// This is useful when you want to manually specify a fallback provider
    /// rather than using the automatic default fallback.
    ///
    /// # Arguments
    ///
    /// * `name` - Provider name to retrieve
    ///
    /// # Returns
    ///
    /// * `Some(&dyn AiProvider)` - Provider reference if found
    /// * `None` - Provider not found
    pub fn get_provider(&self, name: &str) -> Option<&dyn AiProvider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    /// Route with extended decision information (new API)
    ///
    /// This is the enhanced routing method that returns a `RoutingDecision` with
    /// additional information about capabilities, intent, and context format.
    ///
    /// # Arguments
    ///
    /// * `context` - The routing context (clipboard content + window context)
    /// * `rule_config` - The matched routing rule configuration
    ///
    /// # Returns
    ///
    /// * `Some(RoutingDecision)` - Extended routing decision with full context
    /// * `None` - No provider found
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::router::Router;
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// # let config = Config::default();
    /// let router = Router::new(&config)?;
    ///
    /// // Get routing decision with extended info
    /// if let Some(decision) = router.route_with_extended_info("[VSCode] main.rs") {
    ///     println!("Provider: {}", decision.provider_name);
    ///     println!("Intent: {:?}", decision.intent);
    ///     println!("Capabilities: {:?}", decision.capabilities);
    ///     println!("Format: {:?}", decision.context_format);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn route_with_extended_info<'a>(&'a self, context: &str) -> Option<RoutingDecision<'a>> {
        use crate::config::RoutingRuleConfig;

        info!(
            context_length = context.len(),
            rules_count = self.rules.len(),
            "Starting extended route decision"
        );

        // Find the first matching rule
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.matches(context) {
                // Get provider by name
                if let Some(provider) = self.providers.get(rule.provider_name()) {
                    // Get fallback provider (if different from primary)
                    let fallback = self.default_provider.as_ref().and_then(|default_name| {
                        if default_name != rule.provider_name() {
                            self.providers
                                .get(default_name)
                                .map(|p| p.as_ref() as &dyn AiProvider)
                        } else {
                            None
                        }
                    });

                    // Convert RoutingRule to RoutingRuleConfig for decision
                    let rule_config = RoutingRuleConfig {
                        regex: rule.pattern.clone(),
                        provider: rule.provider_name.clone(),
                        system_prompt: rule.system_prompt.clone(),
                        strip_prefix: Some(rule.strip_prefix),
                        capabilities: None,   // Rules don't have this yet
                        intent_type: None,    // Rules don't have this yet
                        context_format: None, // Rules don't have this yet
                        skill_id: None,
                        skill_version: None,
                        workflow: None,
                        tools: None,
                        knowledge_base: None,
                    };

                    let decision = RoutingDecision::from_rule(
                        provider.as_ref(),
                        rule.provider_name.clone(),
                        &rule_config,
                        fallback,
                    );

                    info!(
                        rule_index = index,
                        provider = %decision.provider_name,
                        intent = %decision.intent,
                        capabilities_count = decision.capabilities.len(),
                        "Extended routing decision created"
                    );

                    return Some(decision);
                }
            }
        }

        // Fallback to default provider if available
        if let Some(default_name) = &self.default_provider {
            if let Some(provider) = self.providers.get(default_name) {
                info!(
                    provider = %default_name,
                    "Using default provider (no rules matched)"
                );

                return Some(RoutingDecision::basic(
                    provider.as_ref(),
                    default_name.clone(),
                    "You are a helpful AI assistant.".to_string(),
                ));
            }
        }

        warn!("No provider found (no rules matched and no default configured)");
        None
    }

    /// Strip command prefix from input based on matched routing rule
    ///
    /// This method finds the first matching rule and strips its matched prefix
    /// if the rule has `strip_prefix` enabled. For command patterns like `^/en`,
    /// this removes the `/en` prefix from the input.
    ///
    /// # Arguments
    ///
    /// * `context` - The routing context (clipboard content + window context)
    /// * `input` - The original user input (clipboard content only)
    ///
    /// # Returns
    ///
    /// The input with command prefix stripped if applicable, or original input
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use aethecore::router::Router;
    /// # use aethecore::config::Config;
    /// # fn example() -> aethecore::error::Result<()> {
    /// # let config = Config::default();
    /// # let router = Router::new(&config)?;
    /// let input = "/en Hello world";
    /// let context = format!("{}\n---\n[App] Window", input);
    /// let stripped = router.strip_command_prefix(&context, input);
    /// // If a rule ^/en matched, stripped would be "Hello world"
    /// # Ok(())
    /// # }
    /// ```
    pub fn strip_command_prefix(&self, context: &str, input: &str) -> String {
        // Find the first matching rule
        for rule in &self.rules {
            if rule.matches(context) {
                if rule.should_strip_prefix() {
                    let stripped = rule.strip_matched_prefix(input);
                    info!(
                        original_length = input.len(),
                        stripped_length = stripped.len(),
                        pattern = %rule.pattern,
                        "Stripped command prefix from input"
                    );
                    return stripped;
                }
                // Rule matched but strip_prefix is false
                return input.to_string();
            }
        }
        // No rule matched, return original
        input.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_rule_creation() {
        let rule = RoutingRule::new(r"^/code", "claude", Some("You are a coder"), None);
        assert!(rule.is_ok());

        let rule = rule.unwrap();
        assert_eq!(rule.provider_name(), "claude");
        assert_eq!(rule.system_prompt(), Some("You are a coder"));
    }

    #[test]
    fn test_routing_rule_invalid_regex() {
        let rule = RoutingRule::new(r"[invalid(regex", "claude", None, None);
        assert!(rule.is_err());
        assert!(matches!(rule, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_routing_rule_matching() {
        let rule = RoutingRule::new(r"^/code", "claude", None, None).unwrap();

        assert!(rule.matches("/code write a function"));
        assert!(rule.matches("/code"));
        assert!(!rule.matches("code"));
        assert!(!rule.matches("Hello /code"));
    }

    #[test]
    fn test_routing_rule_complex_regex() {
        let rule = RoutingRule::new(r"^/(code|rust|python)", "claude", None, None).unwrap();

        assert!(rule.matches("/code something"));
        assert!(rule.matches("/rust something"));
        assert!(rule.matches("/python something"));
        assert!(!rule.matches("/java something"));
    }

    #[test]
    fn test_routing_rule_case_sensitive() {
        let rule = RoutingRule::new(r"^/Code", "claude", None, None).unwrap();

        assert!(rule.matches("/Code"));
        assert!(!rule.matches("/code")); // Case sensitive by default
    }

    #[test]
    fn test_routing_rule_catch_all() {
        let rule = RoutingRule::new(r".*", "openai", None, None).unwrap();

        assert!(rule.matches("anything"));
        assert!(rule.matches(""));
        assert!(rule.matches("/code"));
    }

    #[test]
    fn test_router_creation_with_providers() {
        let mut config = Config::default();

        // Add OpenAI provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        let router = Router::new(&config);
        assert!(router.is_ok());

        let router = router.unwrap();
        assert_eq!(router.provider_count(), 1);
        assert!(router.has_provider("openai"));
    }

    #[test]
    fn test_router_creation_without_providers() {
        let config = Config::default();
        let router = Router::new(&config);

        assert!(router.is_err());
        assert!(matches!(router, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_router_default_provider_validation() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        // Set default to non-existent provider
        config.general.default_provider = Some("nonexistent".to_string());

        let router = Router::new(&config);
        assert!(router.is_err());
        assert!(matches!(router, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_router_default_provider_fallback() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        // Set valid default provider
        config.general.default_provider = Some("openai".to_string());

        let router = Router::new(&config).unwrap();

        // Route with no rules should use default
        let result = router.route("Hello world");
        assert!(result.is_some());

        let (provider, system_prompt) = result.unwrap();
        assert_eq!(provider.name(), "openai");
        assert!(system_prompt.is_none());
    }

    #[test]
    fn test_router_no_match_no_default() {
        let mut config = Config::default();

        // Add provider but no default
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        let router = Router::new(&config).unwrap();

        // Route with no rules and no default should return None
        let result = router.route("Hello world");
        assert!(result.is_none());
    }

    #[test]
    fn test_router_metadata() {
        let mut config = Config::default();

        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        config.general.default_provider = Some("openai".to_string());

        let router = Router::new(&config).unwrap();

        assert_eq!(router.provider_count(), 1);
        assert_eq!(router.rule_count(), 0);
        assert!(router.has_provider("openai"));
        assert!(!router.has_provider("claude"));
        assert_eq!(router.default_provider_name(), Some("openai"));
    }

    #[test]
    fn test_router_multiple_providers() {
        let mut config = Config::default();

        // Add multiple providers
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        config.providers.insert("claude".to_string(), {
            let mut config = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
            config.provider_type = Some("claude".to_string());
            config
        });

        let router = Router::new(&config).unwrap();

        assert_eq!(router.provider_count(), 2);
        assert!(router.has_provider("openai"));
        assert!(router.has_provider("claude"));
    }

    #[test]
    fn test_router_with_rules() {
        let mut config = Config::default();

        // Add providers
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        config.providers.insert("claude".to_string(), {
            let mut config = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
            config.provider_type = Some("claude".to_string());
            config
        });

        // Add routing rules
        config.rules.push(RoutingRuleConfig {
            regex: r"^/code".to_string(),
            provider: "claude".to_string(),
            system_prompt: Some("You are a coder".to_string()),
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            context_format: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        });

        config.rules.push(RoutingRuleConfig {
            regex: r".*".to_string(),
            provider: "openai".to_string(),
            system_prompt: None,
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            context_format: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        });

        let router = Router::new(&config).unwrap();

        assert_eq!(router.rule_count(), 2);
    }

    #[test]
    fn test_router_rule_matching_priority() {
        let mut config = Config::default();

        // Add providers
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        config.providers.insert("claude".to_string(), {
            let mut config = ProviderConfig::test_config("claude-3-5-sonnet-20241022");
            config.provider_type = Some("claude".to_string());
            config
        });

        // Add routing rules (first match wins)
        config.rules.push(RoutingRuleConfig {
            regex: r"^/code".to_string(),
            provider: "claude".to_string(),
            system_prompt: Some("You are a coder".to_string()),
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            context_format: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        });

        config.rules.push(RoutingRuleConfig {
            regex: r".*".to_string(),
            provider: "openai".to_string(),
            system_prompt: None,
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            context_format: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        });

        let router = Router::new(&config).unwrap();

        // Test code request routes to Claude
        let result = router.route("/code write a function");
        assert!(result.is_some());
        let (provider, sys_prompt) = result.unwrap();
        assert_eq!(provider.name(), "claude"); // Claude provider
        assert_eq!(sys_prompt, Some("You are a coder"));

        // Test generic request routes to OpenAI (fallback)
        let result = router.route("Hello world");
        assert!(result.is_some());
        let (provider, sys_prompt) = result.unwrap();
        assert_eq!(provider.name(), "openai");
        assert!(sys_prompt.is_none());
    }

    #[test]
    fn test_router_invalid_provider_reference() {
        let mut config = Config::default();

        // Add only one provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        // Add rule referencing non-existent provider
        config.rules.push(RoutingRuleConfig {
            regex: r"^/code".to_string(),
            provider: "nonexistent".to_string(),
            system_prompt: None,
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            context_format: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        });

        let result = Router::new(&config);
        assert!(result.is_err());
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_router_invalid_regex_in_rule() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        // Add rule with invalid regex
        config.rules.push(RoutingRuleConfig {
            regex: r"[invalid(regex".to_string(),
            provider: "openai".to_string(),
            system_prompt: None,
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            context_format: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        });

        let result = Router::new(&config);
        assert!(result.is_err());
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_routing_rule_config_serialization() {
        let rule = RoutingRuleConfig {
            regex: r"^/code".to_string(),
            provider: "claude".to_string(),
            system_prompt: Some("You are a coder".to_string()),
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            context_format: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        };

        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("^/code"));
        assert!(json.contains("claude"));
        assert!(json.contains("You are a coder"));
    }

    #[test]
    fn test_routing_rule_config_deserialization() {
        let json = r#"{
            "regex": "^/code",
            "provider": "claude",
            "system_prompt": "You are a coder"
        }"#;

        let rule: RoutingRuleConfig = serde_json::from_str(json).unwrap();
        assert_eq!(rule.regex, "^/code");
        assert_eq!(rule.provider, "claude");
        assert_eq!(rule.system_prompt, Some("You are a coder".to_string()));
    }

    #[test]
    fn test_config_with_rules_serialization() {
        let mut config = Config::default();

        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        config.rules.push(RoutingRuleConfig {
            regex: r"^/code".to_string(),
            provider: "openai".to_string(),
            system_prompt: Some("You are a coder".to_string()),
            strip_prefix: None,
            capabilities: None,
            intent_type: None,
            context_format: None,
            skill_id: None,
            skill_version: None,
            workflow: None,
            tools: None,
            knowledge_base: None,
        });

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("rules"));
        assert!(json.contains("^/code"));
    }
}

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
use crate::config::Config;
use crate::error::{AetherError, Result};
use crate::providers::{create_provider, AiProvider};
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

// Import for tests
#[cfg(test)]
use crate::config::{ProviderConfig, RoutingRuleConfig};

// Type aliases for complex return types
/// Primary provider with system prompt, and optional fallback provider
pub type ProviderWithFallback<'a> = ((&'a dyn AiProvider, Option<&'a str>), Option<&'a dyn AiProvider>);

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
    /// Name of the provider to use when this rule matches
    provider_name: String,
    /// Optional system prompt to guide AI behavior
    system_prompt: Option<String>,
}

impl RoutingRule {
    /// Create a new routing rule
    ///
    /// # Arguments
    ///
    /// * `pattern` - Regex pattern string (compiled at creation time)
    /// * `provider_name` - Name of the provider to use
    /// * `system_prompt` - Optional system prompt override
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
    ///     Some("You are DALL-E. Generate images.")
    /// )?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(pattern: &str, provider_name: &str, system_prompt: Option<&str>) -> Result<Self> {
        let regex = Regex::new(pattern).map_err(|e| {
            AetherError::invalid_config(format!("Invalid regex pattern '{}': {}", pattern, e))
        })?;

        Ok(Self {
            regex,
            provider_name: provider_name.to_string(),
            system_prompt: system_prompt.map(|s| s.to_string()),
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
    /// let rule = RoutingRule::new(r"^/code", "claude", None)?;
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

    /// Route input to the appropriate AI provider
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text to route
    ///
    /// # Returns
    ///
    /// * `Some((provider, system_prompt))` - Matched provider and optional system prompt
    /// * `None` - No provider available (no match and no default)
    ///
    /// # Routing Logic
    ///
    /// 1. Check each rule in order
    /// 2. Return first matching rule's provider + system prompt
    /// 3. If no match, return default provider (if configured)
    /// 4. If no default, return None
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
    /// // Route with rule match
    /// if let Some((provider, sys_prompt)) = router.route("/code write a function") {
    ///     println!("Matched provider: {}", provider.name());
    /// }
    ///
    /// // Route with default fallback
    /// if let Some((provider, _)) = router.route("Hello") {
    ///     println!("Using default provider: {}", provider.name());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn route(&self, input: &str) -> Option<(&dyn AiProvider, Option<&str>)> {
        debug!(input_length = input.len(), "Starting route decision");

        // Find first matching rule
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.matches(input) {
                // Get provider by name
                if let Some(provider) = self.providers.get(rule.provider_name()) {
                    info!(
                        rule_index = index,
                        provider = %rule.provider_name(),
                        has_system_prompt = rule.system_prompt().is_some(),
                        "Rule matched, routing to provider"
                    );
                    return Some((provider.as_ref(), rule.system_prompt()));
                }
                // Rule matched but provider not found (should not happen due to validation)
                warn!(
                    provider = %rule.provider_name(),
                    "Rule matched but provider not found in registry"
                );
            }
        }

        // No rule matched, fall back to default provider
        debug!("No rule matched, attempting default provider fallback");
        let result = self
            .default_provider
            .as_ref()
            .and_then(|name| self.providers.get(name))
            .map(|provider| (provider.as_ref(), None));

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

    /// Route input and provide fallback provider if requested provider fails
    ///
    /// This method returns both the primary provider and a fallback provider
    /// (if different from primary). The fallback is the default provider.
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text to route
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
    /// if let Some(((provider, sys_prompt), fallback)) = router.route_with_fallback("/code test") {
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
    pub fn route_with_fallback(
        &self,
        input: &str,
    ) -> Option<ProviderWithFallback<'_>> {
        // Get primary routing result
        let (primary_provider, system_prompt) = self.route(input)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_routing_rule_creation() {
        let rule = RoutingRule::new(r"^/code", "claude", Some("You are a coder"));
        assert!(rule.is_ok());

        let rule = rule.unwrap();
        assert_eq!(rule.provider_name(), "claude");
        assert_eq!(rule.system_prompt(), Some("You are a coder"));
    }

    #[test]
    fn test_routing_rule_invalid_regex() {
        let rule = RoutingRule::new(r"[invalid(regex", "claude", None);
        assert!(rule.is_err());
        assert!(matches!(rule, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_routing_rule_matching() {
        let rule = RoutingRule::new(r"^/code", "claude", None).unwrap();

        assert!(rule.matches("/code write a function"));
        assert!(rule.matches("/code"));
        assert!(!rule.matches("code"));
        assert!(!rule.matches("Hello /code"));
    }

    #[test]
    fn test_routing_rule_complex_regex() {
        let rule = RoutingRule::new(r"^/(code|rust|python)", "claude", None).unwrap();

        assert!(rule.matches("/code something"));
        assert!(rule.matches("/rust something"));
        assert!(rule.matches("/python something"));
        assert!(!rule.matches("/java something"));
    }

    #[test]
    fn test_routing_rule_case_sensitive() {
        let rule = RoutingRule::new(r"^/Code", "claude", None).unwrap();

        assert!(rule.matches("/Code"));
        assert!(!rule.matches("/code")); // Case sensitive by default
    }

    #[test]
    fn test_routing_rule_catch_all() {
        let rule = RoutingRule::new(r".*", "openai", None).unwrap();

        assert!(rule.matches("anything"));
        assert!(rule.matches(""));
        assert!(rule.matches("/code"));
    }

    #[test]
    fn test_router_creation_with_providers() {
        let mut config = Config::default();

        // Add OpenAI provider
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

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
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

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
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

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
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        let router = Router::new(&config).unwrap();

        // Route with no rules and no default should return None
        let result = router.route("Hello world");
        assert!(result.is_none());
    }

    #[test]
    fn test_router_metadata() {
        let mut config = Config::default();

        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

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
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        config.providers.insert(
            "claude".to_string(),
            ProviderConfig {
                provider_type: Some("claude".to_string()),
                api_key: Some("sk-ant-test".to_string()),
                model: "claude-3-5-sonnet-20241022".to_string(),
                base_url: None,
                color: "#d97757".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        let router = Router::new(&config).unwrap();

        assert_eq!(router.provider_count(), 2);
        assert!(router.has_provider("openai"));
        assert!(router.has_provider("claude"));
    }

    #[test]
    fn test_router_with_rules() {
        let mut config = Config::default();

        // Add providers
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        config.providers.insert(
            "claude".to_string(),
            ProviderConfig {
                provider_type: Some("claude".to_string()),
                api_key: Some("sk-ant-test".to_string()),
                model: "claude-3-5-sonnet-20241022".to_string(),
                base_url: None,
                color: "#d97757".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        // Add routing rules
        config.rules.push(RoutingRuleConfig {
            regex: r"^/code".to_string(),
            provider: "claude".to_string(),
            system_prompt: Some("You are a coder".to_string()),
        });

        config.rules.push(RoutingRuleConfig {
            regex: r".*".to_string(),
            provider: "openai".to_string(),
            system_prompt: None,
        });

        let router = Router::new(&config).unwrap();

        assert_eq!(router.rule_count(), 2);
    }

    #[test]
    fn test_router_rule_matching_priority() {
        let mut config = Config::default();

        // Add providers
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        config.providers.insert(
            "claude".to_string(),
            ProviderConfig {
                provider_type: Some("claude".to_string()),
                api_key: Some("sk-ant-test".to_string()),
                model: "claude-3-5-sonnet-20241022".to_string(),
                base_url: None,
                color: "#d97757".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        // Add routing rules (first match wins)
        config.rules.push(RoutingRuleConfig {
            regex: r"^/code".to_string(),
            provider: "claude".to_string(),
            system_prompt: Some("You are a coder".to_string()),
        });

        config.rules.push(RoutingRuleConfig {
            regex: r".*".to_string(),
            provider: "openai".to_string(),
            system_prompt: None,
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
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        // Add rule referencing non-existent provider
        config.rules.push(RoutingRuleConfig {
            regex: r"^/code".to_string(),
            provider: "nonexistent".to_string(),
            system_prompt: None,
        });

        let result = Router::new(&config);
        assert!(result.is_err());
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_router_invalid_regex_in_rule() {
        let mut config = Config::default();

        // Add provider
        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        // Add rule with invalid regex
        config.rules.push(RoutingRuleConfig {
            regex: r"[invalid(regex".to_string(),
            provider: "openai".to_string(),
            system_prompt: None,
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

        config.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                provider_type: Some("openai".to_string()),
                api_key: Some("sk-test".to_string()),
                model: "gpt-4o".to_string(),
                base_url: None,
                color: "#10a37f".to_string(),
                timeout_seconds: 30,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
        );

        config.rules.push(RoutingRuleConfig {
            regex: r"^/code".to_string(),
            provider: "openai".to_string(),
            system_prompt: Some("You are a coder".to_string()),
        });

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("rules"));
        assert!(json.contains("^/code"));
    }
}

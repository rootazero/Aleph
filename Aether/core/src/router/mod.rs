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

use crate::config::{Config, KeywordRuleConfig, SmartMatchingConfig};
use crate::error::{AetherError, Result};
use crate::payload::{Capability, Intent};
use crate::providers::{create_provider, AiProvider};
use crate::semantic::{
    KeywordIndex, KeywordMatch, MatchResult, MatcherConfig, MatchingContext, SemanticMatcher,
};
use crate::semantic::keyword::{KeywordMatchMode, KeywordRule};
use crate::video::extract_youtube_url;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

// Re-export
pub use decision::RoutingDecision;

// Import for tests
#[cfg(test)]
use crate::config::{ProviderConfig, RoutingRuleConfig};

// ============================================================================
// Skill Command Parsing
// ============================================================================

/// Extract skill_id and remaining input from a `/skill <name> <input>` command.
///
/// # Format
///
/// `/skill <skill_name> <remaining_input>`
///
/// The skill_name must be a valid identifier (alphanumeric, hyphens, underscores).
///
/// # Returns
///
/// - `Some((skill_id, remaining_input))` if the input matches the `/skill <name>` pattern
/// - `None` if the input doesn't match the pattern
///
/// # Examples
///
/// ```rust,ignore
/// let (skill_id, remaining) = extract_skill_command("/skill refine-text Fix this text").unwrap();
/// assert_eq!(skill_id, "refine-text");
/// assert_eq!(remaining, "Fix this text");
/// ```
pub fn extract_skill_command(input: &str) -> Option<(String, String)> {
    // Match /skill followed by whitespace, then skill name (alphanumeric, hyphens, underscores)
    // Regex: ^/skill\s+([a-zA-Z0-9_-]+)\s*(.*)$
    static SKILL_COMMAND_REGEX: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"^/skill\s+([a-zA-Z0-9_-]+)\s*(.*)$").expect("Invalid skill command regex")
    });

    SKILL_COMMAND_REGEX.captures(input).map(|caps| {
        let skill_id = caps.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
        let remaining = caps.get(2).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        (skill_id, remaining)
    })
}

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

// ============================================================================
// Routing Match Types (Command + Keyword rule matching result)
// ============================================================================

/// Result of routing rule matching
///
/// Contains the matched command rule (if any) and all matched keyword rules.
/// Command rules use first-match-stops semantics, keyword rules use all-match.
///
/// # Example
///
/// ```rust,ignore
/// let result = router.match_rules("/draw 一幅山水画");
///
/// // Command rule matched: provider + cleaned input
/// if let Some(cmd) = &result.command_rule {
///     println!("Provider: {}", cmd.provider_name);
///     println!("Cleaned input: {}", cmd.cleaned_input);
/// }
///
/// // Multiple keyword rules can match
/// for keyword in &result.keyword_rules {
///     println!("Keyword prompt: {}", keyword.system_prompt);
/// }
///
/// // Get combined system prompt
/// let final_prompt = result.assemble_prompt();
/// ```
#[derive(Debug, Clone, Default)]
pub struct RoutingMatch {
    /// Matched command rule (if any) - first-match-stops
    pub command_rule: Option<MatchedCommandRule>,
    /// All matched keyword rules - all-match semantics
    pub keyword_rules: Vec<MatchedKeywordRule>,
}

/// A matched command rule with provider and cleaned input
#[derive(Debug, Clone)]
pub struct MatchedCommandRule {
    /// Name of the provider to use
    pub provider_name: String,
    /// System prompt from the rule (if any)
    pub system_prompt: Option<String>,
    /// Input with command prefix stripped (e.g., "/draw cat" → "cat")
    pub cleaned_input: String,
    /// Capabilities enabled for this rule (Memory, Search, etc.)
    pub capabilities: Vec<Capability>,
    /// Index of the matched rule (for debugging/logging)
    pub rule_index: usize,
    /// Skill ID extracted from /skill <name> command (None for non-skill commands)
    pub skill_id: Option<String>,
}

/// A matched keyword rule with system prompt
#[derive(Debug, Clone)]
pub struct MatchedKeywordRule {
    /// System prompt to add to the final prompt
    pub system_prompt: String,
    /// Index of the matched rule (for debugging/logging)
    pub rule_index: usize,
}

impl RoutingMatch {
    /// Check if any rule was matched
    pub fn has_match(&self) -> bool {
        self.command_rule.is_some() || !self.keyword_rules.is_empty()
    }

    /// Get the provider name (from command rule, or None for keyword-only)
    pub fn provider_name(&self) -> Option<&str> {
        self.command_rule.as_ref().map(|c| c.provider_name.as_str())
    }

    /// Get the cleaned input (command prefix stripped if applicable)
    pub fn cleaned_input(&self) -> Option<&str> {
        self.command_rule.as_ref().map(|c| c.cleaned_input.as_str())
    }

    /// Combine all prompts into final system prompt
    ///
    /// Order: command rule prompt first, then keyword rule prompts
    /// Separator: double newline (\n\n) for clear separation
    pub fn assemble_prompt(&self) -> Option<String> {
        let mut prompts = Vec::new();

        // Add command rule prompt first
        if let Some(ref cmd) = self.command_rule {
            if let Some(ref prompt) = cmd.system_prompt {
                if !prompt.is_empty() {
                    prompts.push(prompt.as_str());
                }
            }
        }

        // Add all keyword rule prompts
        for keyword in &self.keyword_rules {
            if !keyword.system_prompt.is_empty() {
                prompts.push(&keyword.system_prompt);
            }
        }

        if prompts.is_empty() {
            None
        } else {
            // Join with double newline for clear separation
            Some(prompts.join("\n\n"))
        }
    }

    /// Get capabilities from the matched command rule
    ///
    /// Returns an empty vec if no command rule matched.
    pub fn get_capabilities(&self) -> Vec<Capability> {
        self.command_rule
            .as_ref()
            .map(|c| c.capabilities.clone())
            .unwrap_or_default()
    }

    /// Get skill ID from the matched command rule (for /skill commands)
    ///
    /// Returns None if no skill command matched or skill_id was not extracted.
    pub fn get_skill_id(&self) -> Option<&str> {
        self.command_rule
            .as_ref()
            .and_then(|c| c.skill_id.as_deref())
    }

    /// Create an Intent based on the matched rule
    ///
    /// For /skill commands, creates Intent::Skills with the extracted skill_id.
    /// For other commands, returns the intent from the rule config.
    /// For no match, returns Intent::GeneralChat.
    ///
    /// # Arguments
    ///
    /// * `rule_configs` - Reference to rule configurations for intent lookup
    pub fn to_intent(&self, rule_configs: &[crate::config::RoutingRuleConfig]) -> Intent {
        if let Some(ref cmd) = self.command_rule {
            // If skill_id was extracted, create Intent::Skills
            if let Some(ref skill_id) = cmd.skill_id {
                return Intent::Skills(skill_id.clone());
            }

            // Otherwise, get intent from rule config
            if let Some(rule_config) = rule_configs.get(cmd.rule_index) {
                return Intent::from_rule(rule_config);
            }
        }

        Intent::GeneralChat
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
    /// Ordered list of routing rules (first match wins) - legacy, kept for compatibility
    rules: Vec<RoutingRule>,
    /// Rule configurations (parallel to rules, for accessing capabilities/intent)
    rule_configs: Vec<crate::config::RoutingRuleConfig>,
    /// Command rules (first-match-stops, requires provider, strips prefix)
    command_rules: Vec<(usize, RoutingRule)>, // (index, rule)
    /// Keyword rules (all-match, prompt only)
    keyword_rules: Vec<(usize, RoutingRule)>, // (index, rule)
    /// Registry of available providers (name → provider instance)
    providers: HashMap<String, Arc<dyn AiProvider>>,
    /// Optional default provider name (fallback when no rule matches)
    default_provider: Option<String>,
    /// Semantic matcher for enhanced multi-layer matching (optional)
    semantic_matcher: Option<SemanticMatcher>,
    /// Smart matching configuration
    smart_matching_config: SmartMatchingConfig,
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

        // Create provider instances from config (only enabled providers)
        for (name, provider_config) in &config.providers {
            // Skip disabled providers
            if !provider_config.enabled {
                info!(
                    provider = %name,
                    "Skipping disabled provider"
                );
                continue;
            }

            let provider = create_provider(name, provider_config.clone())?;
            providers.insert(name.clone(), provider);
        }

        // Validate at least one provider exists
        if providers.is_empty() {
            return Err(AetherError::invalid_config(
                "No enabled providers configured. At least one enabled provider is required.",
            ));
        }

        // Validate default provider exists AND is enabled (if specified)
        if let Some(ref default_name) = config.general.default_provider {
            if !providers.contains_key(default_name) {
                // Check if it exists but is disabled
                if config.providers.contains_key(default_name) {
                    return Err(AetherError::invalid_config(format!(
                        "Default provider '{}' is disabled. Enable it or choose a different default provider.",
                        default_name
                    )));
                }
                return Err(AetherError::invalid_config(format!(
                    "Default provider '{}' not found in configured providers",
                    default_name
                )));
            }
        }

        // Load routing rules from config
        // Note: Rules are now split into command rules and keyword rules
        // - Command rules: first-match-stops, requires provider
        // - Keyword rules: all-match, prompt only (uses default_provider)
        let mut rules = Vec::new();
        let mut rule_configs = Vec::new();
        for rule_config in &config.rules {
            // Determine effective provider for this rule
            let effective_provider = if rule_config.is_command_rule() {
                // Command rules require explicit provider
                match &rule_config.provider {
                    Some(p) => {
                        // For builtin rules, try specified provider first, then fallback to default
                        if rule_config.is_builtin && !providers.contains_key(p) {
                            // Builtin rule: fallback to default_provider
                            config
                                .general
                                .default_provider
                                .clone()
                                .unwrap_or_else(|| "openai".to_string())
                        } else {
                            p.clone()
                        }
                    }
                    None => {
                        // Builtin rules without provider use default_provider
                        if rule_config.is_builtin {
                            config
                                .general
                                .default_provider
                                .clone()
                                .unwrap_or_else(|| "openai".to_string())
                        } else {
                            return Err(AetherError::invalid_config(format!(
                                "Command rule (regex: '{}') requires a provider",
                                rule_config.regex
                            )));
                        }
                    }
                }
            } else {
                // Keyword rules use default provider
                config
                    .general
                    .default_provider
                    .clone()
                    .unwrap_or_else(|| "openai".to_string())
            };

            // Skip builtin rules if no valid provider available
            if rule_config.is_builtin && !providers.contains_key(&effective_provider) {
                debug!(
                    regex = %rule_config.regex,
                    provider = %effective_provider,
                    "Skipping builtin rule: no matching provider available"
                );
                continue;
            }

            // Create RoutingRule from config
            let rule = RoutingRule::new(
                &rule_config.regex,
                &effective_provider,
                rule_config.system_prompt.as_deref(),
                Some(rule_config.should_strip_prefix()),
            )?;

            // Validate that provider exists (only for non-builtin command rules)
            if rule_config.is_command_rule()
                && !rule_config.is_builtin
                && !providers.contains_key(&effective_provider)
            {
                return Err(AetherError::invalid_config(format!(
                    "Rule references unknown provider '{}'. Available providers: {}",
                    effective_provider,
                    providers.keys().cloned().collect::<Vec<_>>().join(", ")
                )));
            }

            rules.push(rule);
            rule_configs.push(rule_config.clone());
        }

        // Split rules into command and keyword categories
        let mut command_rules = Vec::new();
        let mut keyword_rules = Vec::new();

        for (index, (rule, config)) in rules.iter().zip(rule_configs.iter()).enumerate() {
            if config.is_command_rule() {
                command_rules.push((index, rule.clone()));
            } else {
                keyword_rules.push((index, rule.clone()));
            }
        }

        debug!(
            command_count = command_rules.len(),
            keyword_count = keyword_rules.len(),
            "Split rules into command and keyword categories"
        );

        // Build semantic matcher if smart matching is enabled
        let semantic_matcher = if config.smart_matching.enabled {
            let matcher_config = MatcherConfig {
                enabled: config.smart_matching.enabled,
                regex_threshold: config.smart_matching.regex_threshold,
                keyword_threshold: config.smart_matching.keyword_threshold as f32,
                ai_threshold: config.smart_matching.ai_threshold,
                enable_context_inference: config.smart_matching.enable_context_inference,
            };

            // Build keyword index from config's keyword rules
            let mut keyword_index = KeywordIndex::new();
            for keyword_rule_config in &config.smart_matching.keyword_rules {
                let rule = Self::convert_keyword_rule_config(keyword_rule_config);
                keyword_index.add_rule(rule);
            }

            Some(SemanticMatcher::with_keyword_index(&matcher_config, keyword_index))
        } else {
            None
        };

        Ok(Self {
            rules,
            rule_configs,
            command_rules,
            keyword_rules,
            providers,
            default_provider: config.general.default_provider.clone(),
            semantic_matcher,
            smart_matching_config: config.smart_matching.clone(),
        })
    }

    /// Convert a KeywordRuleConfig from config to a KeywordRule for semantic matching
    fn convert_keyword_rule_config(config: &KeywordRuleConfig) -> KeywordRule {
        let keywords = config.parse_keywords();
        let match_mode = match config.match_mode.as_str() {
            "all" => KeywordMatchMode::All,
            "weighted" => KeywordMatchMode::Weighted,
            _ => KeywordMatchMode::Any,
        };

        let mut rule = KeywordRule::with_weights(&config.id, &config.intent_type, keywords)
            .with_mode(match_mode);

        if let Some(ref name) = config.name {
            rule = rule.with_name(name);
        }
        if let Some(ref prompt) = config.system_prompt {
            rule = rule.with_prompt(prompt);
        }
        if let Some(score) = config.min_score {
            rule = rule.with_min_score(score);
        }
        if !config.capabilities.is_empty() {
            rule = rule.with_capabilities(config.capabilities.clone());
        }

        rule
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

    /// Match input against all rules and return combined result
    ///
    /// This is the new two-phase matching system:
    /// - Phase 1: Command rules (first-match-stops) - determines provider
    /// - Phase 2: Keyword rules (all-match) - adds additional prompts
    ///
    /// # Arguments
    ///
    /// * `input` - The user input text to match against rules
    ///
    /// # Returns
    ///
    /// A `RoutingMatch` containing:
    /// - `command_rule`: The first matched command rule (if any)
    /// - `keyword_rules`: All matched keyword rules
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let result = router.match_rules("/draw 一幅山水画");
    ///
    /// // Get provider from command rule
    /// let provider_name = result.provider_name().unwrap_or("default");
    ///
    /// // Get cleaned input (command prefix stripped)
    /// let cleaned = result.cleaned_input().unwrap_or(input);
    ///
    /// // Get combined system prompt
    /// let prompt = result.assemble_prompt();
    /// ```
    pub fn match_rules(&self, input: &str) -> RoutingMatch {
        let mut result = RoutingMatch::default();

        info!(
            input_length = input.len(),
            command_rules = self.command_rules.len(),
            keyword_rules = self.keyword_rules.len(),
            "Starting match_rules with two-phase matching"
        );

        // Phase 1: Find command rule (first-match-stops)
        for (index, rule) in &self.command_rules {
            if rule.matches(input) {
                let rule_config = &self.rule_configs[*index];

                // Check if this is a /skill command (intent_type = "skills")
                let is_skill_command = rule_config.intent_type.as_deref() == Some("skills");

                // For /skill command, extract skill_id from input
                let (skill_id, cleaned_input) = if is_skill_command {
                    if let Some((skill_id, remaining)) = extract_skill_command(input) {
                        info!(
                            skill_id = %skill_id,
                            remaining_input_length = remaining.len(),
                            "Extracted skill_id from /skill command"
                        );
                        (Some(skill_id), remaining)
                    } else {
                        // /skill without skill name - use normal prefix stripping
                        (None, rule.strip_matched_prefix(input))
                    }
                } else {
                    // Normal command - just strip prefix
                    (None, rule.strip_matched_prefix(input))
                };

                info!(
                    rule_index = index,
                    provider = %rule.provider_name(),
                    cleaned_input_length = cleaned_input.len(),
                    skill_id = ?skill_id,
                    "Command rule matched (first-match-stops)"
                );

                // Get capabilities from the rule config
                let capabilities = rule_config.get_capabilities();

                result.command_rule = Some(MatchedCommandRule {
                    provider_name: rule.provider_name().to_string(),
                    system_prompt: rule.system_prompt().map(|s| s.to_string()),
                    cleaned_input,
                    capabilities,
                    rule_index: *index,
                    skill_id,
                });

                break; // First match stops for command rules
            }
        }

        // Phase 2: Find all matching keyword rules (all-match)
        for (index, rule) in &self.keyword_rules {
            if rule.matches(input) {
                if let Some(prompt) = rule.system_prompt() {
                    debug!(
                        rule_index = index,
                        prompt_preview = %prompt.chars().take(50).collect::<String>(),
                        "Keyword rule matched (all-match)"
                    );

                    result.keyword_rules.push(MatchedKeywordRule {
                        system_prompt: prompt.to_string(),
                        rule_index: *index,
                    });
                }
            }
        }

        debug!(
            has_command = result.command_rule.is_some(),
            keyword_count = result.keyword_rules.len(),
            "Match result summary"
        );

        result
    }

    /// Match rules and get provider with combined prompt
    ///
    /// This is a convenience method that combines `match_rules()` with provider lookup.
    /// It returns the provider instance and the assembled system prompt.
    ///
    /// # Returns
    ///
    /// - `Some((provider, cleaned_input, system_prompt))` if a provider was determined
    /// - `None` if no command rule matched and no default provider is configured
    pub fn match_and_get_provider(
        &self,
        input: &str,
    ) -> Option<(&dyn AiProvider, String, Option<String>)> {
        let routing_match = self.match_rules(input);
        let assembled_prompt = routing_match.assemble_prompt();

        // Determine provider: command rule provider or default
        let provider_name = routing_match
            .provider_name()
            .map(|s| s.to_string())
            .or_else(|| self.default_provider.clone())?;

        let provider = self.providers.get(&provider_name)?;

        // Determine cleaned input: from command rule or original
        let cleaned_input = routing_match
            .cleaned_input()
            .map(|s| s.to_string())
            .unwrap_or_else(|| input.to_string());

        Some((provider.as_ref(), cleaned_input, assembled_prompt))
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

    /// Get the number of command rules
    pub fn command_rule_count(&self) -> usize {
        self.command_rules.len()
    }

    /// Get the number of keyword rules
    pub fn keyword_rule_count(&self) -> usize {
        self.keyword_rules.len()
    }

    /// Get the rule configurations for intent resolution
    ///
    /// Used by RoutingMatch::to_intent() to create proper Intent from matched rules.
    pub fn rule_configs(&self) -> &[crate::config::RoutingRuleConfig] {
        &self.rule_configs
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

    /// Get a provider by name (Arc version for shared ownership).
    ///
    /// # Returns
    ///
    /// * `Some(Arc<dyn AiProvider>)` - Provider Arc if found
    /// * `None` - Provider not found
    pub fn get_provider_arc(&self, name: &str) -> Option<Arc<dyn AiProvider>> {
        self.providers.get(name).cloned()
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
        info!(
            context_length = context.len(),
            rules_count = self.rules.len(),
            "Starting extended route decision"
        );

        // Check for YouTube URL in context (for auto-enabling Video capability)
        let has_youtube_url = extract_youtube_url(context).is_some();
        if has_youtube_url {
            debug!("YouTube URL detected in context, Video capability will be auto-enabled");
        }

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

                    // Use the actual rule configuration (has capabilities, intent_type, etc.)
                    let rule_config = &self.rule_configs[index];

                    let mut decision = RoutingDecision::from_rule(
                        provider.as_ref(),
                        rule.provider_name.clone(),
                        rule_config,
                        fallback,
                    );

                    // Auto-add Video capability if YouTube URL detected and not already present
                    if has_youtube_url && !decision.capabilities.contains(&Capability::Video) {
                        decision.capabilities.push(Capability::Video);
                        info!(
                            "Auto-enabled Video capability due to YouTube URL in context"
                        );
                    }

                    info!(
                        rule_index = index,
                        provider = %decision.provider_name,
                        intent = %decision.intent,
                        capabilities_count = decision.capabilities.len(),
                        has_youtube_url = has_youtube_url,
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

                // For default routing, also auto-add Video capability if YouTube URL detected
                let capabilities = if has_youtube_url {
                    info!("Auto-enabled Video capability for default route due to YouTube URL");
                    vec![Capability::Video]
                } else {
                    vec![]
                };

                return Some(RoutingDecision {
                    provider: provider.as_ref(),
                    provider_name: default_name.clone(),
                    system_prompt: "You are a helpful AI assistant.".to_string(),
                    capabilities,
                    intent: crate::payload::Intent::GeneralChat,
                    context_format: crate::payload::ContextFormat::Markdown,
                    fallback: None,
                });
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

    // ============================================================================
    // Semantic Matching Methods (New Semantic Detection System)
    // ============================================================================

    /// Check if semantic matching is enabled
    pub fn is_semantic_matching_enabled(&self) -> bool {
        self.smart_matching_config.enabled && self.semantic_matcher.is_some()
    }

    /// Get the semantic matcher (if enabled)
    pub fn semantic_matcher(&self) -> Option<&SemanticMatcher> {
        self.semantic_matcher.as_ref()
    }

    /// Get the smart matching configuration
    pub fn smart_matching_config(&self) -> &SmartMatchingConfig {
        &self.smart_matching_config
    }

    /// Route using semantic matching with full context
    ///
    /// This method uses the new multi-layer semantic detection system:
    /// - Layer 1: Fast path (command/regex matching)
    /// - Layer 2: Keyword index matching
    /// - Layer 3: Context-aware inference
    /// - Layer 4: AI detection fallback
    ///
    /// # Arguments
    ///
    /// * `context` - Full matching context with conversation, app, and time info
    ///
    /// # Returns
    ///
    /// * `Some(MatchResult)` - Semantic match result with intent and confidence
    /// * `None` - Semantic matching is disabled
    pub async fn route_semantic(&self, context: &MatchingContext) -> Option<MatchResult> {
        let matcher = self.semantic_matcher.as_ref()?;
        Some(matcher.match_input(context).await)
    }

    /// Perform keyword matching only (synchronous)
    ///
    /// This is useful when you only want to check keyword matches without
    /// going through the full semantic detection pipeline.
    ///
    /// # Arguments
    ///
    /// * `input` - Raw user input text
    ///
    /// # Returns
    ///
    /// * `Vec<KeywordMatch>` - List of keyword matches sorted by score
    pub fn match_keywords(&self, input: &str) -> Vec<KeywordMatch> {
        if let Some(ref matcher) = self.semantic_matcher {
            matcher.match_keywords_only(input)
        } else {
            Vec::new()
        }
    }

    /// Get keyword matches above a confidence threshold
    pub fn match_keywords_with_threshold(&self, input: &str, min_score: f32) -> Vec<KeywordMatch> {
        self.match_keywords(input)
            .into_iter()
            .filter(|m| m.score >= min_score)
            .collect()
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
        // Config::default() includes 5 preset rules (/search, /mcp, /skill, /video, /chat)
        assert_eq!(router.rule_count(), 5);
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

        // Add routing rules (using command factory method)
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/code", "claude", Some("You are a coder")));
        config.rules.push(RoutingRuleConfig::command(r".*", "openai", None));

        let router = Router::new(&config).unwrap();

        // 5 preset rules + 2 custom rules = 7 total
        assert_eq!(router.rule_count(), 7);
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

        // Add routing rules (first match wins, using command factory method)
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/code", "claude", Some("You are a coder")));
        config.rules.push(RoutingRuleConfig::command(r".*", "openai", None));

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

        // Add command rule referencing non-existent provider
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/code", "nonexistent", None));

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

        // Add rule with invalid regex - need to create manually for invalid regex test
        let mut invalid_rule = RoutingRuleConfig::command(r"[invalid(regex", "openai", None);
        invalid_rule.regex = r"[invalid(regex".to_string();
        config.rules.push(invalid_rule);

        let result = Router::new(&config);
        assert!(result.is_err());
        assert!(matches!(result, Err(AetherError::InvalidConfig { .. })));
    }

    #[test]
    fn test_routing_rule_config_serialization() {
        let rule = RoutingRuleConfig::command(r"^/code", "claude", Some("You are a coder"));

        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains("^/code"));
        assert!(json.contains("claude"));
        assert!(json.contains("You are a coder"));
    }

    #[test]
    fn test_routing_rule_config_deserialization() {
        // Test backward compatibility: old JSON without rule_type/is_builtin should still work
        let json = r#"{
            "regex": "^/code",
            "provider": "claude",
            "system_prompt": "You are a coder"
        }"#;

        let rule: RoutingRuleConfig = serde_json::from_str(json).unwrap();
        assert_eq!(rule.regex, "^/code");
        assert_eq!(rule.provider, Some("claude".to_string()));
        assert_eq!(rule.system_prompt, Some("You are a coder".to_string()));
        // Auto-detected rule type should be "command" because regex starts with ^/
        assert_eq!(rule.get_rule_type(), "command");
        assert!(!rule.is_builtin);
    }

    #[test]
    fn test_config_with_rules_serialization() {
        let mut config = Config::default();

        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));

        config
            .rules
            .push(RoutingRuleConfig::command(r"^/code", "openai", Some("You are a coder")));

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("rules"));
        assert!(json.contains("^/code"));
    }

    #[test]
    fn test_rule_type_auto_detection() {
        // Command rule pattern (starts with ^/)
        let command_rule = RoutingRuleConfig::test_config(r"^/draw", "openai");
        assert_eq!(command_rule.get_rule_type(), "command");
        assert!(command_rule.is_command_rule());
        assert!(!command_rule.is_keyword_rule());

        // Keyword rule pattern (does not start with ^/)
        let keyword_rule = RoutingRuleConfig::keyword("翻译成英文", "翻译目标语言为英文");
        assert_eq!(keyword_rule.get_rule_type(), "keyword");
        assert!(keyword_rule.is_keyword_rule());
        assert!(!keyword_rule.is_command_rule());
    }

    // ============================================================================
    // Tests for match_rules (two-phase command/keyword matching)
    // ============================================================================

    #[test]
    fn test_match_rules_command_only() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        // Add command rule
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/draw", "openai", Some("You are an artist.")));

        let router = Router::new(&config).unwrap();

        // Match command rule
        let result = router.match_rules("/draw a beautiful sunset");

        // Should have command rule matched
        assert!(result.command_rule.is_some());
        assert!(result.keyword_rules.is_empty());

        let cmd = result.command_rule.unwrap();
        assert_eq!(cmd.provider_name, "openai");
        assert_eq!(cmd.system_prompt, Some("You are an artist.".to_string()));
        assert_eq!(cmd.cleaned_input, "a beautiful sunset"); // prefix stripped

        // Check assembled prompt
        let result = router.match_rules("/draw a cat");
        assert_eq!(result.assemble_prompt(), Some("You are an artist.".to_string()));
    }

    #[test]
    fn test_match_rules_keyword_only() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        // Add keyword rules
        config
            .rules
            .push(RoutingRuleConfig::keyword("翻译成英文", "翻译目标语言为英文"));
        config
            .rules
            .push(RoutingRuleConfig::keyword("请帮我", "语气友好礼貌"));

        let router = Router::new(&config).unwrap();

        // Match keyword rules
        let result = router.match_rules("请帮我翻译成英文：你好世界");

        // Should have no command rule but multiple keyword rules
        assert!(result.command_rule.is_none());
        assert_eq!(result.keyword_rules.len(), 2);

        // Check prompts
        let prompts: Vec<_> = result.keyword_rules.iter().map(|k| k.system_prompt.as_str()).collect();
        assert!(prompts.contains(&"翻译目标语言为英文"));
        assert!(prompts.contains(&"语气友好礼貌"));

        // Check assembled prompt (joined with \n\n)
        let assembled = result.assemble_prompt().unwrap();
        assert!(assembled.contains("翻译目标语言为英文"));
        assert!(assembled.contains("语气友好礼貌"));
        assert!(assembled.contains("\n\n")); // Double newline separator
    }

    #[test]
    fn test_match_rules_command_and_keyword() {
        let mut config = Config::default();

        // Add provider (use mock type)
        config.providers.insert("artist".to_string(), {
            let mut c = ProviderConfig::test_config("test-model");
            c.provider_type = Some("mock".to_string());
            c
        });
        config.general.default_provider = Some("artist".to_string());

        // Add command rule
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/draw", "artist", Some("You are an AI artist.")));

        // Add keyword rule
        config
            .rules
            .push(RoutingRuleConfig::keyword("山水画", "中国传统山水画风格"));

        let router = Router::new(&config).unwrap();

        // Match both command and keyword rules
        let result = router.match_rules("/draw 一幅山水画");

        // Should have both
        assert!(result.command_rule.is_some());
        assert_eq!(result.keyword_rules.len(), 1);

        let cmd = result.command_rule.as_ref().unwrap();
        assert_eq!(cmd.provider_name, "artist");
        assert_eq!(cmd.cleaned_input, "一幅山水画");

        // Check assembled prompt (command first, then keywords)
        let assembled = result.assemble_prompt().unwrap();
        assert!(assembled.starts_with("You are an AI artist."));
        assert!(assembled.contains("中国传统山水画风格"));
    }

    #[test]
    fn test_match_rules_no_match() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        // Add rules that won't match
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/draw", "openai", Some("You are an artist.")));

        let router = Router::new(&config).unwrap();

        // No match
        let result = router.match_rules("Hello world");

        assert!(result.command_rule.is_none());
        assert!(result.keyword_rules.is_empty());
        assert!(!result.has_match());
        assert!(result.assemble_prompt().is_none());
    }

    #[test]
    fn test_match_rules_command_first_match_stops() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config
            .providers
            .insert("claude".to_string(), {
                let mut c = ProviderConfig::test_config("claude-3-5-sonnet");
                c.provider_type = Some("claude".to_string());
                c
            });
        config.general.default_provider = Some("openai".to_string());

        // Add two command rules that could both match
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/code", "openai", Some("First rule")));
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/code", "claude", Some("Second rule")));

        let router = Router::new(&config).unwrap();

        // First match should win for command rules
        let result = router.match_rules("/code write a function");

        assert!(result.command_rule.is_some());
        let cmd = result.command_rule.unwrap();
        assert_eq!(cmd.provider_name, "openai"); // First rule's provider
        assert_eq!(cmd.system_prompt, Some("First rule".to_string()));
    }

    #[test]
    fn test_match_rules_keyword_all_match() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        // Add multiple keyword rules
        config
            .rules
            .push(RoutingRuleConfig::keyword("test", "Prompt A"));
        config
            .rules
            .push(RoutingRuleConfig::keyword("test", "Prompt B"));
        config
            .rules
            .push(RoutingRuleConfig::keyword("test", "Prompt C"));

        let router = Router::new(&config).unwrap();

        // All keyword rules should match (all-match semantics)
        let result = router.match_rules("this is a test input");

        assert!(result.command_rule.is_none());
        assert_eq!(result.keyword_rules.len(), 3);

        // All prompts should be included
        let prompts: Vec<_> = result.keyword_rules.iter().map(|k| k.system_prompt.as_str()).collect();
        assert!(prompts.contains(&"Prompt A"));
        assert!(prompts.contains(&"Prompt B"));
        assert!(prompts.contains(&"Prompt C"));
    }

    #[test]
    fn test_match_and_get_provider() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        // Add command rule
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/test", "openai", Some("Test prompt")));

        let router = Router::new(&config).unwrap();

        // Match and get provider
        let result = router.match_and_get_provider("/test hello world");
        assert!(result.is_some());

        let (provider, cleaned_input, prompt) = result.unwrap();
        assert_eq!(provider.name(), "openai");
        assert_eq!(cleaned_input, "hello world");
        assert_eq!(prompt, Some("Test prompt".to_string()));
    }

    #[test]
    fn test_routing_match_provider_name() {
        let mut routing_match = RoutingMatch::default();
        assert!(routing_match.provider_name().is_none());

        routing_match.command_rule = Some(MatchedCommandRule {
            provider_name: "gemini".to_string(),
            system_prompt: None,
            cleaned_input: "test".to_string(),
            capabilities: vec![],
            rule_index: 0,
            skill_id: None,
        });

        assert_eq!(routing_match.provider_name(), Some("gemini"));
    }

    #[test]
    fn test_command_and_keyword_rule_counts() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        // Add 2 command rules
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/draw", "openai", Some("Artist")));
        config
            .rules
            .push(RoutingRuleConfig::command(r"^/code", "openai", Some("Coder")));

        // Add 3 keyword rules
        config
            .rules
            .push(RoutingRuleConfig::keyword("polite", "Be polite"));
        config
            .rules
            .push(RoutingRuleConfig::keyword("formal", "Be formal"));
        config
            .rules
            .push(RoutingRuleConfig::keyword("brief", "Be brief"));

        let router = Router::new(&config).unwrap();

        // Check counts (builtin rules + custom rules)
        assert!(router.command_rule_count() >= 2); // At least 2 custom + builtin
        assert_eq!(router.keyword_rule_count(), 3); // Exactly 3 keyword rules
    }

    // ============================================================================
    // Tests for skill command extraction
    // ============================================================================

    #[test]
    fn test_extract_skill_command_basic() {
        let result = extract_skill_command("/skill refine-text Fix this text");
        assert!(result.is_some());

        let (skill_id, remaining) = result.unwrap();
        assert_eq!(skill_id, "refine-text");
        assert_eq!(remaining, "Fix this text");
    }

    #[test]
    fn test_extract_skill_command_with_underscore() {
        let result = extract_skill_command("/skill build_macos_apps Create an app");
        assert!(result.is_some());

        let (skill_id, remaining) = result.unwrap();
        assert_eq!(skill_id, "build_macos_apps");
        assert_eq!(remaining, "Create an app");
    }

    #[test]
    fn test_extract_skill_command_no_remaining_input() {
        let result = extract_skill_command("/skill pdf");
        assert!(result.is_some());

        let (skill_id, remaining) = result.unwrap();
        assert_eq!(skill_id, "pdf");
        assert_eq!(remaining, "");
    }

    #[test]
    fn test_extract_skill_command_extra_whitespace() {
        let result = extract_skill_command("/skill   code-review   Please review this code");
        assert!(result.is_some());

        let (skill_id, remaining) = result.unwrap();
        assert_eq!(skill_id, "code-review");
        assert_eq!(remaining, "Please review this code");
    }

    #[test]
    fn test_extract_skill_command_no_match_no_skill_name() {
        // Missing skill name
        let result = extract_skill_command("/skill");
        assert!(result.is_none());

        let result = extract_skill_command("/skill ");
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_skill_command_no_match_wrong_command() {
        let result = extract_skill_command("/draw something");
        assert!(result.is_none());

        let result = extract_skill_command("skill test");
        assert!(result.is_none());
    }

    #[test]
    fn test_match_rules_skill_command() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        let router = Router::new(&config).unwrap();

        // Match /skill command (should use builtin /skill rule)
        let result = router.match_rules("/skill refine-text Improve this text");

        // Should have command rule matched with skill_id extracted
        assert!(result.command_rule.is_some());
        let cmd = result.command_rule.unwrap();

        // Skill ID should be extracted
        assert_eq!(cmd.skill_id, Some("refine-text".to_string()));
        // Cleaned input should have both /skill and skill_name stripped
        assert_eq!(cmd.cleaned_input, "Improve this text");
    }

    #[test]
    fn test_routing_match_get_skill_id() {
        let mut routing_match = RoutingMatch::default();
        assert!(routing_match.get_skill_id().is_none());

        routing_match.command_rule = Some(MatchedCommandRule {
            provider_name: "openai".to_string(),
            system_prompt: None,
            cleaned_input: "test input".to_string(),
            capabilities: vec![],
            rule_index: 0,
            skill_id: Some("build-macos-apps".to_string()),
        });

        assert_eq!(routing_match.get_skill_id(), Some("build-macos-apps"));
    }

    #[test]
    fn test_routing_match_to_intent_with_skill() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        let router = Router::new(&config).unwrap();

        // Match /skill command
        let result = router.match_rules("/skill pdf Extract text from document");

        // Should create Intent::Skills with skill_id
        let intent = result.to_intent(router.rule_configs());
        assert!(intent.is_skills());
        assert_eq!(intent.skills_id(), Some("pdf"));
    }

    #[test]
    fn test_routing_match_to_intent_no_match() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        let router = Router::new(&config).unwrap();

        // No match - just plain text
        let result = router.match_rules("Hello world");

        // Should return GeneralChat
        let intent = result.to_intent(router.rule_configs());
        assert_eq!(intent, Intent::GeneralChat);
    }

    #[test]
    fn test_routing_match_to_intent_search_command() {
        let mut config = Config::default();

        // Add provider
        config
            .providers
            .insert("openai".to_string(), ProviderConfig::test_config("gpt-4o"));
        config.general.default_provider = Some("openai".to_string());

        let router = Router::new(&config).unwrap();

        // Match /search command (builtin rule)
        let result = router.match_rules("/search latest news");

        // Should return BuiltinSearch intent
        let intent = result.to_intent(router.rule_configs());
        // Note: builtin_search is the intent_type for /search
        assert!(matches!(intent, Intent::BuiltinSearch | Intent::Custom(_)));
    }
}

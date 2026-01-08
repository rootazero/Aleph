//! Unified Semantic Detection System
//!
//! Multi-layer matching with context awareness:
//! - Layer 1: Fast path - command/regex matching (existing)
//! - Layer 2: Keyword index - weighted keyword scoring
//! - Layer 3: Context inference - multi-turn, app, time aware
//! - Layer 4: AI fallback - AI-first detection
//!
//! # Architecture
//!
//! ```text
//! User Input
//!     ↓
//! SemanticMatcher (orchestrator)
//!     ├─ [Layer 1] Fast Path: Command/Regex Match
//!     ├─ [Layer 2] Keyword Index Match
//!     ├─ [Layer 3] Context-Aware Inference
//!     └─ [Layer 4] AI-First Detection
//!            ↓
//!     MatchingContext (conversation, app, time)
//!            ↓
//!     SemanticIntent → PromptAssembler → Provider
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! use aethecore::semantic::{SemanticMatcher, MatchingContext, SemanticIntent};
//!
//! async fn example() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = MatcherConfig::default();
//!     let matcher = SemanticMatcher::new(&config);
//!
//!     let context = MatchingContext::builder()
//!         .raw_input("What's the weather in Beijing?")
//!         .conversation(ConversationContext::default())
//!         .app(AppContext::default())
//!         .time(TimeContext::now())
//!         .build();
//!
//!     let result = matcher.match_input(&context).await;
//!     println!("Intent: {:?}, Confidence: {}", result.intent, result.confidence);
//!     Ok(())
//! }
//! ```

pub mod assembler;
pub mod context;
pub mod intent;
pub mod keyword;
pub mod layer;
pub mod layers;
pub mod matcher;
pub mod template;

// Re-exports
pub use assembler::{AssembledPrompt, SmartPromptAssembler, TruncationStrategy};
pub use context::{
    AppContext, ConversationContext, ConversationTurn, InputFeatures, MatchingContext,
    PendingParam, TimeContext,
};
pub use intent::{DetectionMethod, IntentCategory, ParamValue, SemanticIntent};
pub use keyword::{KeywordIndex, KeywordMatch};
pub use layer::{LayerChain, LayerEnabledFlag, MatchingLayer};
pub use layers::{CommandLayer, ContextLayer, KeywordLayer, RegexLayer};
pub use matcher::{MatchResult, MatcherConfig, SemanticMatcher};
pub use template::{ContextSection, PromptTemplate, TemplateVariable};

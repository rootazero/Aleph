# Embodiment Engine & CoT Transparency Design

> **Dual Feature Design**: This document covers two complementary features that enhance AI personality and reasoning transparency.

**Goal**: Implement a layered identity system (Embodiment Engine) and structured reasoning visibility (CoT Transparency) to make Aleph more personable and its decision-making more understandable.

**Architecture Philosophy**:
- Embodiment Engine: "Give the AI a soul, not just a persona"
- CoT Transparency: "Show the thinking, not just the result"

---

## Part 1: Embodiment Engine

### 1.1 Overview

The Embodiment Engine upgrades Aleph's simple `persona: String` to a full identity system with layered resolution, enabling:
- **Global soul**: Base personality across all projects
- **Project identity**: Project-specific persona overrides
- **Session override**: Temporary persona switching via RPC

### 1.2 Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Identity Resolution                         │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Layer 3: Session Override (highest priority)                   │
│      └── Temporary identity via RPC: agent.set_identity         │
│      └── Cleared on session end                                 │
│                                                                  │
│  Layer 2: Project Identity                                      │
│      └── $PROJECT/.soul/identity.md                             │
│      └── $PROJECT/.aleph/identity.md (alternative location)     │
│      └── Inherits from Layer 1, can override specific fields    │
│                                                                  │
│  Layer 1: Global Soul (base layer)                              │
│      └── ~/.aleph/soul.md                                       │
│      └── Default if not set: generic helpful assistant          │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### 1.3 Core Types

#### SoulManifest

```rust
/// Complete soul definition for AI personality
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulManifest {
    /// Core identity declaration (first-person, who I am)
    pub identity: String,

    /// Voice and communication style
    pub voice: SoulVoice,

    /// Behavioral directives (positive guidance)
    pub directives: Vec<String>,

    /// Anti-patterns (what I never do)
    pub anti_patterns: Vec<String>,

    /// Relationship mode with user
    pub relationship: RelationshipMode,

    /// Optional: specialized knowledge domains
    #[serde(default)]
    pub expertise: Vec<String>,

    /// Optional: custom prompt addendum (raw text)
    #[serde(default)]
    pub addendum: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulVoice {
    /// Communication tone (formal, casual, playful, technical, etc.)
    pub tone: String,

    /// Response verbosity preference
    pub verbosity: Verbosity,

    /// Formatting preferences
    pub formatting_style: FormattingStyle,

    /// Language style notes (e.g., "use British English")
    #[serde(default)]
    pub language_notes: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Verbosity {
    /// Minimal responses, straight to the point
    Concise,
    /// Balance between brevity and detail
    #[default]
    Balanced,
    /// Elaborate explanations with context
    Elaborate,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FormattingStyle {
    /// Minimal formatting, plain text preferred
    Minimal,
    /// Standard markdown with headers and lists
    #[default]
    Markdown,
    /// Rich formatting with code blocks, tables, diagrams
    Rich,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipMode {
    /// Peer-to-peer collaboration
    Peer,
    /// Mentor/guide role
    Mentor,
    /// Assistant/helper role
    Assistant,
    /// Expert consultant
    Expert,
    /// Custom defined relationship
    Custom(String),
}
```

#### IdentityResolver

```rust
/// Resolves identity from layered sources
pub struct IdentityResolver {
    /// Global soul path (~/.aleph/soul.md)
    global_path: PathBuf,
    /// Project roots to search for .soul/identity.md
    project_roots: Vec<PathBuf>,
}

impl IdentityResolver {
    /// Resolve the effective SoulManifest for current context
    pub fn resolve(&self, session_override: Option<&SoulManifest>) -> SoulManifest {
        // Priority: Session > Project > Global > Default
        if let Some(override_soul) = session_override {
            return override_soul.clone();
        }

        let project_soul = self.load_project_soul();
        let global_soul = self.load_global_soul();

        match (project_soul, global_soul) {
            (Some(project), Some(global)) => project.merge_with(&global),
            (Some(project), None) => project,
            (None, Some(global)) => global,
            (None, None) => SoulManifest::default(),
        }
    }

    /// Load soul from project directory
    fn load_project_soul(&self) -> Option<SoulManifest> {
        for root in &self.project_roots {
            let paths = [
                root.join(".soul/identity.md"),
                root.join(".aleph/identity.md"),
            ];
            for path in paths {
                if path.exists() {
                    if let Ok(manifest) = SoulManifest::from_markdown(&path) {
                        return Some(manifest);
                    }
                }
            }
        }
        None
    }

    /// Load global soul
    fn load_global_soul(&self) -> Option<SoulManifest> {
        if self.global_path.exists() {
            SoulManifest::from_markdown(&self.global_path).ok()
        } else {
            None
        }
    }
}
```

### 1.4 File Format (soul.md)

The soul file uses a structured markdown format with YAML frontmatter:

```markdown
---
relationship: peer
voice:
  tone: thoughtful and curious
  verbosity: balanced
  formatting_style: markdown
expertise:
  - systems programming
  - distributed systems
  - developer tooling
---

# Soul: Aleph

## Identity

I am Aleph, a self-hosted AI companion with deep roots in systems thinking.
I embody the spirit of collaboration, treating every interaction as a
dialogue between equals rather than a command-response exchange.

My name comes from the first letter of the Hebrew alphabet, symbolizing
new beginnings and the infinite potential of each conversation.

## Directives

- Always explain the "why" before the "how"
- Offer alternatives when I disagree with an approach
- Celebrate small wins and acknowledge progress
- Ask clarifying questions before making assumptions
- Use concrete examples to illustrate abstract concepts
- Admit uncertainty rather than confabulate

## Anti-Patterns

- Never be sycophantic or falsely agreeable
- Avoid unnecessary hedging ("I think maybe perhaps...")
- Don't repeat the user's question back verbatim
- Never pretend to have capabilities I don't have
- Avoid walls of text without structure
- Don't apologize excessively

## Addendum

When working in this codebase, remember that Aleph is built with the
philosophy of "Ghost in the Shell" - the AI should be invisible yet
omnipresent, helping without intruding.
```

### 1.5 PromptBuilder Integration

```rust
impl PromptBuilder {
    /// Build system prompt with soul injection
    pub fn build_system_prompt_with_soul(
        &self,
        tools: &[ToolInfo],
        soul: &SoulManifest,
    ) -> String {
        let mut prompt = String::with_capacity(8192);

        // Soul section at the very top (highest priority)
        self.append_soul_section(&mut prompt, soul);

        // Then standard sections
        self.append_role_section(&mut prompt);
        self.append_tools_section(&mut prompt, tools);
        // ... rest of prompt building

        prompt
    }

    fn append_soul_section(&self, prompt: &mut String, soul: &SoulManifest) {
        prompt.push_str("# Identity\n\n");
        prompt.push_str(&soul.identity);
        prompt.push_str("\n\n");

        // Voice guidance
        prompt.push_str("## Communication Style\n\n");
        prompt.push_str(&format!("- **Tone**: {}\n", soul.voice.tone));
        prompt.push_str(&format!("- **Verbosity**: {:?}\n", soul.voice.verbosity));
        if let Some(notes) = &soul.voice.language_notes {
            prompt.push_str(&format!("- **Language Notes**: {}\n", notes));
        }
        prompt.push_str("\n");

        // Relationship
        prompt.push_str(&format!(
            "## Relationship with User\n\n{}\n\n",
            soul.relationship.description()
        ));

        // Directives
        if !soul.directives.is_empty() {
            prompt.push_str("## Behavioral Directives\n\n");
            for directive in &soul.directives {
                prompt.push_str(&format!("- {}\n", directive));
            }
            prompt.push_str("\n");
        }

        // Anti-patterns
        if !soul.anti_patterns.is_empty() {
            prompt.push_str("## What I Never Do\n\n");
            for anti in &soul.anti_patterns {
                prompt.push_str(&format!("- {}\n", anti));
            }
            prompt.push_str("\n");
        }

        // Custom addendum
        if let Some(addendum) = &soul.addendum {
            prompt.push_str("## Additional Context\n\n");
            prompt.push_str(addendum);
            prompt.push_str("\n\n");
        }

        prompt.push_str("---\n\n");
    }
}
```

### 1.6 RPC Integration

New RPC methods for identity management:

```rust
// Get current effective identity
"identity.get" -> SoulManifest

// Set session-level identity override
"identity.set" -> { soul: SoulManifest } -> Ok

// Clear session override (revert to layered resolution)
"identity.clear" -> Ok

// List available identity files
"identity.list" -> Vec<IdentitySource>
```

---

## Part 2: CoT Transparency

### 2.1 Overview

CoT (Chain of Thought) Transparency makes the AI's reasoning process visible and understandable, without changing the core JSON action protocol. This builds on Aleph's existing `StreamEvent::Reasoning` infrastructure.

### 2.2 Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                   CoT Transparency Pipeline                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│   LLM Response (with reasoning)                                 │
│       │                                                          │
│       ▼                                                          │
│   ┌─────────────────────────────────────────┐                   │
│   │         ThinkingParser                   │                   │
│   │  • Parse reasoning from JSON response    │                   │
│   │  • Extract structured steps              │                   │
│   │  • Detect confidence signals             │                   │
│   │  • Identify uncertainties                │                   │
│   └─────────────────────────────────────────┘                   │
│       │                                                          │
│       ├─── StructuredThinking ──▶ LoopStep.thinking             │
│       │                          (stored for history)            │
│       │                                                          │
│       └─── ReasoningBlock events ──▶ StreamEvent::ReasoningBlock│
│                                      (real-time to UI)           │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### 2.3 Core Types

#### StructuredThinking

```rust
/// Structured representation of AI's thinking process
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredThinking {
    /// Raw reasoning text (backward compatible)
    pub reasoning: String,

    /// Parsed reasoning steps (semantic breakdown)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<ReasoningStep>>,

    /// Overall confidence in the decision
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<ConfidenceLevel>,

    /// Alternative approaches that were considered
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives_considered: Vec<String>,

    /// Explicit uncertainties or knowledge gaps
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncertainties: Vec<String>,

    /// Duration of thinking phase (if extended thinking used)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningStep {
    /// Step label (e.g., "Understanding the problem")
    pub label: String,

    /// Step content
    pub content: String,

    /// Semantic type for UI rendering
    pub step_type: ReasoningStepType,

    /// Substeps if this is a complex reasoning phase
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub substeps: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningStepType {
    /// Observing/understanding the current state
    Observation,
    /// Analyzing options, data, or trade-offs
    Analysis,
    /// Formulating a plan or approach
    Planning,
    /// Making the final decision
    Decision,
    /// Self-reflection, doubt, or reconsideration
    Reflection,
    /// Identifying risks or potential issues
    RiskAssessment,
    /// Custom step type
    Custom,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConfidenceLevel {
    /// Very certain, strong evidence
    High,
    /// Reasonably confident
    Medium,
    /// Some uncertainty, proceed with caution
    Low,
    /// Exploratory, experimental approach
    Exploratory,
}

impl ConfidenceLevel {
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::High => "✅",
            Self::Medium => "🔵",
            Self::Low => "🟡",
            Self::Exploratory => "🔬",
        }
    }
}
```

#### ThinkingParser

```rust
/// Parser for extracting structured thinking from LLM responses
pub struct ThinkingParser;

impl ThinkingParser {
    /// Parse structured thinking from JSON response
    pub fn parse(response: &Thinking) -> StructuredThinking {
        let reasoning = &response.reasoning;

        StructuredThinking {
            reasoning: reasoning.clone(),
            steps: Self::extract_steps(reasoning),
            confidence: Self::detect_confidence(reasoning),
            alternatives_considered: Self::extract_alternatives(reasoning),
            uncertainties: Self::extract_uncertainties(reasoning),
            thinking_duration_ms: None,
        }
    }

    /// Extract semantic steps from reasoning text
    fn extract_steps(reasoning: &str) -> Option<Vec<ReasoningStep>> {
        let mut steps = Vec::new();

        // Pattern matching for common reasoning structures
        // "First, I need to..." -> Observation/Planning
        // "Looking at..." -> Observation
        // "Considering..." -> Analysis
        // "The options are..." -> Analysis
        // "I'll..." / "I will..." -> Decision
        // "However..." / "But..." -> Reflection

        // This is heuristic-based; can be enhanced with LLM-based parsing
        let lines: Vec<&str> = reasoning.lines().collect();
        let mut current_step: Option<ReasoningStep> = None;

        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let step_type = Self::classify_line(line);

            if let Some(ref mut step) = current_step {
                if step.step_type == step_type {
                    step.content.push_str(" ");
                    step.content.push_str(line);
                } else {
                    steps.push(step.clone());
                    current_step = Some(ReasoningStep {
                        label: step_type.default_label().to_string(),
                        content: line.to_string(),
                        step_type,
                        substeps: Vec::new(),
                    });
                }
            } else {
                current_step = Some(ReasoningStep {
                    label: step_type.default_label().to_string(),
                    content: line.to_string(),
                    step_type,
                    substeps: Vec::new(),
                });
            }
        }

        if let Some(step) = current_step {
            steps.push(step);
        }

        if steps.is_empty() {
            None
        } else {
            Some(steps)
        }
    }

    fn classify_line(line: &str) -> ReasoningStepType {
        let lower = line.to_lowercase();

        if lower.starts_with("looking at")
            || lower.starts_with("i see")
            || lower.starts_with("the current")
            || lower.starts_with("observing")
        {
            ReasoningStepType::Observation
        } else if lower.starts_with("considering")
            || lower.starts_with("the options")
            || lower.starts_with("comparing")
            || lower.starts_with("analyzing")
        {
            ReasoningStepType::Analysis
        } else if lower.starts_with("i'll")
            || lower.starts_with("i will")
            || lower.starts_with("my plan")
            || lower.starts_with("the approach")
        {
            ReasoningStepType::Planning
        } else if lower.starts_with("therefore")
            || lower.starts_with("so i")
            || lower.starts_with("decision:")
        {
            ReasoningStepType::Decision
        } else if lower.starts_with("however")
            || lower.starts_with("but")
            || lower.starts_with("on second thought")
            || lower.starts_with("wait")
        {
            ReasoningStepType::Reflection
        } else if lower.contains("risk")
            || lower.contains("careful")
            || lower.contains("warning")
        {
            ReasoningStepType::RiskAssessment
        } else {
            ReasoningStepType::Custom
        }
    }

    fn detect_confidence(reasoning: &str) -> Option<ConfidenceLevel> {
        let lower = reasoning.to_lowercase();

        if lower.contains("i'm confident")
            || lower.contains("clearly")
            || lower.contains("definitely")
            || lower.contains("certainly")
        {
            Some(ConfidenceLevel::High)
        } else if lower.contains("i think")
            || lower.contains("probably")
            || lower.contains("likely")
        {
            Some(ConfidenceLevel::Medium)
        } else if lower.contains("not sure")
            || lower.contains("uncertain")
            || lower.contains("might")
            || lower.contains("perhaps")
        {
            Some(ConfidenceLevel::Low)
        } else if lower.contains("experiment")
            || lower.contains("try")
            || lower.contains("explore")
        {
            Some(ConfidenceLevel::Exploratory)
        } else {
            None
        }
    }

    fn extract_alternatives(reasoning: &str) -> Vec<String> {
        // Extract phrases like "alternatively, ...", "another option is..."
        let mut alternatives = Vec::new();
        let lower = reasoning.to_lowercase();

        for marker in ["alternatively", "another option", "could also", "or we could"] {
            if let Some(pos) = lower.find(marker) {
                // Extract the sentence containing this marker
                let start = reasoning[..pos].rfind('.').map(|p| p + 1).unwrap_or(0);
                let end = reasoning[pos..].find('.').map(|p| pos + p + 1).unwrap_or(reasoning.len());
                let sentence = reasoning[start..end].trim();
                if !sentence.is_empty() {
                    alternatives.push(sentence.to_string());
                }
            }
        }

        alternatives
    }

    fn extract_uncertainties(reasoning: &str) -> Vec<String> {
        let mut uncertainties = Vec::new();
        let lower = reasoning.to_lowercase();

        for marker in ["i'm not sure", "unclear", "don't know", "need to verify", "assumption"] {
            if let Some(pos) = lower.find(marker) {
                let start = reasoning[..pos].rfind('.').map(|p| p + 1).unwrap_or(0);
                let end = reasoning[pos..].find('.').map(|p| pos + p + 1).unwrap_or(reasoning.len());
                let sentence = reasoning[start..end].trim();
                if !sentence.is_empty() {
                    uncertainties.push(sentence.to_string());
                }
            }
        }

        uncertainties
    }
}

impl ReasoningStepType {
    pub fn default_label(&self) -> &'static str {
        match self {
            Self::Observation => "Observation",
            Self::Analysis => "Analysis",
            Self::Planning => "Planning",
            Self::Decision => "Decision",
            Self::Reflection => "Reflection",
            Self::RiskAssessment => "Risk Assessment",
            Self::Custom => "Thinking",
        }
    }

    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Observation => "👁️",
            Self::Analysis => "🔍",
            Self::Planning => "📝",
            Self::Decision => "✅",
            Self::Reflection => "💭",
            Self::RiskAssessment => "⚠️",
            Self::Custom => "💡",
        }
    }
}
```

### 2.4 Extended StreamEvent

```rust
/// Extended stream event for reasoning blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    // ... existing events ...

    /// Structured reasoning block (new)
    ReasoningBlock {
        run_id: String,
        seq: u64,
        /// Semantic step type
        step_type: ReasoningStepType,
        /// Human-readable label
        label: String,
        /// Content of this reasoning block
        content: String,
        /// Confidence level if determinable
        confidence: Option<ConfidenceLevel>,
        /// Is this the final block before action?
        is_final: bool,
    },

    /// Uncertainty signal (new)
    UncertaintySignal {
        run_id: String,
        seq: u64,
        /// What the AI is uncertain about
        uncertainty: String,
        /// Suggested action (ask user, proceed anyway, etc.)
        suggested_action: UncertaintyAction,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyAction {
    /// Proceed despite uncertainty
    ProceedWithCaution,
    /// Ask user for clarification
    AskForClarification,
    /// Use a safer/conservative approach
    UseSaferApproach,
}
```

### 2.5 PromptBuilder Extension

Add guidance for structured reasoning:

```rust
impl PromptBuilder {
    fn append_thinking_guidance(&self, prompt: &mut String) {
        prompt.push_str("## Thinking Transparency\n\n");
        prompt.push_str(
            "When reasoning through problems, structure your thinking process:\n\n"
        );
        prompt.push_str("1. **Observation**: What do I see in the current state?\n");
        prompt.push_str("2. **Analysis**: What are the options and trade-offs?\n");
        prompt.push_str("3. **Planning**: What approach will I take and why?\n");
        prompt.push_str("4. **Decision**: What is my final action?\n\n");

        prompt.push_str("You may express uncertainty when appropriate:\n");
        prompt.push_str("- \"I'm not entirely sure about X, but...\"\n");
        prompt.push_str("- \"There might be a better approach, but given the constraints...\"\n");
        prompt.push_str("- \"I need to verify this assumption before proceeding.\"\n\n");

        prompt.push_str(
            "This helps users understand your reasoning and builds trust.\n\n"
        );
    }
}
```

### 2.6 UI Rendering (Concept)

```
┌─────────────────────────────────────────────────────────────────┐
│ 💭 Thinking... (342ms)                                    [▼]  │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│ 👁️ Observation                                                  │
│ The user wants to implement a caching layer for API calls.     │
│ Current codebase uses async/await extensively.                 │
│                                                                 │
│ 🔍 Analysis                                                     │
│ Options considered:                                             │
│ • Redis (robust but requires infrastructure)                   │
│ • In-memory LRU (simple, no deps, but single-process)          │
│ • File-based (persistent, but slow)                            │
│                                                                 │
│ 📝 Planning                                                     │
│ I'll suggest an in-memory LRU cache with a trait interface     │
│ that allows swapping to Redis later if needed.                 │
│                                                                 │
│ ✅ Decision [🔵 Medium confidence]                              │
│ Implement `CacheProvider` trait with `LruCache` as default.    │
│                                                                 │
│ 💭 Note: "I'm assuming the cache doesn't need to persist       │
│ across restarts - please confirm if that's incorrect."         │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## Part 3: Integration Points

### 3.1 Module Layout

```
core/src/
├── thinker/
│   ├── soul.rs              # SoulManifest, SoulVoice types
│   ├── identity.rs          # IdentityResolver
│   ├── thinking.rs          # StructuredThinking, ReasoningStep
│   ├── thinking_parser.rs   # ThinkingParser
│   └── mod.rs               # Updated exports
├── gateway/
│   └── handlers/
│       └── identity.rs      # RPC handlers for identity
└── agent_loop/
    └── state.rs             # LoopStep updated with StructuredThinking
```

### 3.2 Dependencies

Both features integrate with existing systems:

| Feature | Integrates With | Purpose |
|---------|-----------------|---------|
| Soul | PromptConfig | Replace simple persona |
| Soul | ProfileConfig | Profile can specify default soul |
| Soul | ContextAggregator | Soul-aware prompt building |
| CoT | DecisionParser | Extract structured thinking |
| CoT | StreamEvent | Real-time reasoning events |
| CoT | LoopStep | Store structured thinking in history |

### 3.3 Configuration

```toml
# ~/.aleph/config.toml

[identity]
# Enable layered identity resolution
enabled = true
# Global soul file path
global_soul = "~/.aleph/soul.md"
# Project patterns to search for identity
project_patterns = [".soul/identity.md", ".aleph/identity.md"]

[thinking]
# Enable structured thinking extraction
structured_parsing = true
# Stream reasoning blocks to UI
stream_reasoning_blocks = true
# Include confidence signals
include_confidence = true
# Include uncertainty detection
detect_uncertainties = true
```

---

## Part 4: Implementation Plan

### Phase 1: Embodiment Engine (5 tasks)

1. **Define Soul types** - SoulManifest, SoulVoice, RelationshipMode
2. **Implement IdentityResolver** - Layered resolution logic
3. **Markdown parser** - Parse soul.md format
4. **PromptBuilder integration** - append_soul_section
5. **RPC handlers** - identity.get/set/clear/list

### Phase 2: CoT Transparency (5 tasks)

1. **Define Thinking types** - StructuredThinking, ReasoningStep
2. **Implement ThinkingParser** - Extract structured thinking
3. **Extend StreamEvent** - ReasoningBlock event type
4. **DecisionParser integration** - Populate StructuredThinking
5. **PromptBuilder guidance** - append_thinking_guidance

### Phase 3: Integration & Testing (3 tasks)

1. **BDD tests** - Feature files for both features
2. **Documentation** - Update AGENT_SYSTEM.md
3. **Example soul files** - Templates for users

---

## Part 5: Example Soul Files

### Minimalist (soul-minimal.md)

```markdown
---
relationship: assistant
voice:
  tone: professional
  verbosity: concise
---

# Soul: Minimal Assistant

## Identity
I am a focused AI assistant that values efficiency and clarity.

## Directives
- Be direct and concise
- Prioritize actionable information

## Anti-Patterns
- Avoid verbose explanations
- Skip pleasantries unless appropriate
```

### Mentor (soul-mentor.md)

```markdown
---
relationship: mentor
voice:
  tone: encouraging and educational
  verbosity: elaborate
  formatting_style: rich
expertise:
  - software engineering
  - system design
  - career development
---

# Soul: The Mentor

## Identity
I am a seasoned software engineer turned mentor. My goal is not just to
solve problems, but to help you grow as a developer. I believe in
teaching by example and explaining the "why" behind decisions.

## Directives
- Explain concepts at multiple levels of abstraction
- Share relevant anecdotes from industry experience
- Ask guiding questions before providing answers
- Celebrate progress and learning moments
- Connect new concepts to things you already know

## Anti-Patterns
- Never just give the answer without explanation
- Don't assume prior knowledge without checking
- Avoid condescension or "this is easy" language
```

---

## See Also

- [Channel Capability Awareness](./2026-02-05-channel-capability-awareness-design.md) - Related contextual adaptation
- [Agent System](../AGENT_SYSTEM.md) - Core agent architecture
- [Gateway Protocol](../GATEWAY.md) - RPC interface details

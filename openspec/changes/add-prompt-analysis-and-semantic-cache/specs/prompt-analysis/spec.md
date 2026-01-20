# Spec: prompt-analysis

Prompt analysis capability for intelligent model routing based on prompt content features.

## ADDED Requirements

### Requirement: Token Estimation
The system MUST provide accurate token count estimation for prompts.

#### Scenario: Estimate tokens for English text
- Given a prompt "Hello, how are you today?"
- When the system estimates tokens
- Then the estimated count is within 5% of actual tiktoken (cl100k_base) output

#### Scenario: Estimate tokens for Chinese text
- Given a prompt "你好，今天天气怎么样？"
- When the system estimates tokens
- Then the estimated count is within 10% of actual tiktoken output

#### Scenario: Estimate tokens for mixed content
- Given a prompt containing both English and code blocks
- When the system estimates tokens
- Then the estimated count accounts for both text and code tokenization

### Requirement: Complexity Scoring
The system MUST calculate a complexity score (0.0-1.0) for prompts.

#### Scenario: Simple prompt scores low
- Given a simple prompt "What is the capital of France?"
- When the system calculates complexity
- Then the complexity score is less than 0.3

#### Scenario: Complex prompt scores high
- Given a complex prompt requiring multi-step reasoning with technical terms
- When the system calculates complexity
- Then the complexity score is greater than 0.7

#### Scenario: Complexity factors are weighted
- Given configured complexity weights for length, structure, technical terms, and multi-step indicators
- When the system calculates complexity
- Then all factors contribute according to their configured weights

### Requirement: Language Detection
The system MUST detect the primary language of prompts.

#### Scenario: Detect English text
- Given a prompt "Please explain machine learning algorithms"
- When the system detects language
- Then the primary language is English with confidence > 0.9

#### Scenario: Detect Chinese text
- Given a prompt "请用Rust实现快速排序算法"
- When the system detects language
- Then the primary language is Chinese with confidence > 0.8

#### Scenario: Detect mixed language content
- Given a prompt with substantial content in multiple languages
- When the system detects language
- Then the primary language is Mixed when no single language exceeds 70%

### Requirement: Code Detection
The system MUST detect code content within prompts.

#### Scenario: Detect markdown code blocks
- Given a prompt containing ```rust\nfn main() {}\n```
- When the system calculates code ratio
- Then the code ratio is greater than 0.0

#### Scenario: Pure text has zero code ratio
- Given a prompt with only natural language text
- When the system calculates code ratio
- Then the code ratio is 0.0

#### Scenario: Code-heavy prompt has high ratio
- Given a prompt that is primarily code with minimal description
- When the system calculates code ratio
- Then the code ratio is greater than 0.8

### Requirement: Reasoning Level Detection
The system MUST detect the level of reasoning required for prompts.

#### Scenario: Factual query is low reasoning
- Given a prompt "What year was Python released?"
- When the system detects reasoning level
- Then the reasoning level is Low

#### Scenario: Explanation request is medium reasoning
- Given a prompt "Explain how HTTP caching works"
- When the system detects reasoning level
- Then the reasoning level is Medium

#### Scenario: Analysis request is high reasoning
- Given a prompt containing "analyze", "compare", "step by step", or chain-of-thought markers
- When the system detects reasoning level
- Then the reasoning level is High

### Requirement: Domain Classification
The system MUST classify prompts into domains.

#### Scenario: Programming domain detected
- Given a prompt about Rust, Python, or other programming topics
- When the system detects domain
- Then the domain is Technical(Programming)

#### Scenario: General domain for conversational prompts
- Given a casual conversational prompt without technical content
- When the system detects domain
- Then the domain is General

### Requirement: Analysis Performance
The system MUST complete prompt analysis within latency targets.

#### Scenario: Fast analysis for typical prompts
- Given a prompt of typical length (< 1000 characters)
- When the system performs full analysis
- Then the analysis completes within 5ms

#### Scenario: Analysis time is tracked
- Given any prompt analysis operation
- When the analysis completes
- Then the PromptFeatures includes analysis_time_us field

### Requirement: Prompt Features Output
The system MUST produce a complete PromptFeatures struct.

#### Scenario: All fields populated
- Given any non-empty prompt
- When the system analyzes the prompt
- Then the PromptFeatures contains:
  - estimated_tokens (u32)
  - complexity_score (f64, 0.0-1.0)
  - primary_language (Language enum)
  - code_ratio (f64, 0.0-1.0)
  - reasoning_indicators (ReasoningLevel enum)
  - domain (Domain enum)
  - suggested_context_size (ContextSize enum)
  - analysis_time_us (u64)

#### Scenario: Context size suggestion
- Given a prompt with estimated tokens
- When the system suggests context size
- Then Small is suggested for < 4K tokens, Medium for 4K-32K, Large for > 32K

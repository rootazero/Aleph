# model-ensemble Specification

## Purpose

Provides multi-model ensemble execution capability, enabling higher reliability and quality through parallel model execution, response aggregation, and consensus detection for critical tasks.

## ADDED Requirements

### Requirement: Ensemble Configuration

The system SHALL support configuring multi-model ensemble execution strategies.

#### Scenario: Configure ensemble mode
- **GIVEN** the system is initializing
- **WHEN** ensemble configuration is loaded
- **THEN** default mode is set (disabled, best_of_n, voting, consensus, cascade)
- **AND** per-intent strategies override the default
- **AND** complexity threshold triggers high-complexity strategy

#### Scenario: Configure models per strategy
- **GIVEN** ensemble strategy is defined
- **WHEN** `models` array is specified
- **THEN** only listed models participate in ensemble
- **AND** models must exist in configured profiles
- **AND** at least 2 models required for meaningful ensemble

#### Scenario: Configure timeout
- **GIVEN** ensemble strategy is defined
- **WHEN** `timeout_ms` is specified
- **THEN** entire ensemble execution completes within timeout
- **AND** slower models are abandoned if timeout reached
- **AND** at least one successful response is required

#### Scenario: Configure budget constraints
- **GIVEN** ensemble is enabled
- **WHEN** `max_cost_multiplier` is configured (e.g., 3.0)
- **THEN** ensemble cost is limited to multiplier × single model cost
- **AND** models are selected to stay within budget
- **AND** budget check occurs before execution

### Requirement: Parallel Execution

The system SHALL execute multiple models concurrently.

#### Scenario: Execute models in parallel
- **GIVEN** ensemble strategy selects N models
- **WHEN** ensemble execution starts
- **THEN** all N model requests are initiated concurrently
- **AND** execution uses tokio::join_all or equivalent
- **AND** total latency is max(individual latencies) not sum

#### Scenario: Handle partial success
- **GIVEN** N models are executed in parallel
- **WHEN** some models fail and some succeed
- **THEN** successful responses are collected
- **AND** failed responses are logged with error details
- **AND** aggregation proceeds with successful responses only

#### Scenario: Handle timeout
- **GIVEN** ensemble timeout is 30000ms
- **WHEN** a model exceeds timeout
- **THEN** that model's execution is cancelled
- **AND** other models continue until their response or timeout
- **AND** timeout is recorded in ModelExecutionResult

#### Scenario: Limit concurrency
- **GIVEN** `max_concurrency` is configured
- **WHEN** ensemble selects more models than limit
- **THEN** only first N models are executed
- **AND** excess models are not queued
- **AND** logged as skipped due to concurrency limit

### Requirement: Best-of-N Aggregation

The system SHALL select the best response from multiple models using quality scoring.

#### Scenario: Score responses by quality
- **GIVEN** ensemble mode is `best_of_n`
- **WHEN** multiple responses are received
- **THEN** each response is scored by configured quality metric
- **AND** score is normalized to 0.0 - 1.0 range
- **AND** highest scoring response is selected

#### Scenario: Length and structure scoring
- **GIVEN** quality metric is `length_and_structure`
- **WHEN** response is scored
- **THEN** length score considers response length (longer often better for explanations)
- **AND** structure score detects code blocks, lists, headers, paragraphs
- **AND** combined score weights structure higher (0.6) than length (0.4)

#### Scenario: Return ensemble result
- **GIVEN** best response is selected
- **WHEN** ensemble result is returned
- **THEN** result includes selected response and model_id
- **AND** result includes confidence (quality score)
- **AND** result includes all_results for transparency
- **AND** result includes total_cost_usd (sum of all models)

### Requirement: Voting Aggregation

The system SHALL aggregate responses using majority voting.

#### Scenario: Majority voting
- **GIVEN** ensemble mode is `voting`
- **WHEN** 3+ responses are received
- **THEN** responses are grouped by semantic similarity
- **AND** largest group's representative is selected
- **AND** confidence is based on group size / total responses

#### Scenario: No clear majority
- **GIVEN** voting produces tied groups
- **WHEN** no single majority exists
- **THEN** fallback to best_of_n among tied groups
- **AND** confidence is reduced to reflect uncertainty
- **AND** result includes note about tie-breaking

### Requirement: Consensus Detection

The system SHALL detect agreement between model responses.

#### Scenario: Calculate consensus level
- **GIVEN** ensemble mode is `consensus`
- **WHEN** responses are received
- **THEN** pairwise similarity is calculated
- **AND** average similarity is the consensus level (0.0 - 1.0)
- **AND** consensus level is included in result

#### Scenario: High consensus
- **GIVEN** `min_agreement = 0.7`
- **WHEN** consensus level >= 0.7
- **THEN** confidence is high
- **AND** any similar response can be selected
- **AND** result indicates strong agreement

#### Scenario: Low consensus
- **GIVEN** `min_agreement = 0.7`
- **WHEN** consensus level < 0.7
- **THEN** confidence is reduced (multiplied by 0.7)
- **AND** best quality response is selected
- **AND** result includes warning about disagreement

#### Scenario: Similarity calculation
- **GIVEN** two responses to compare
- **WHEN** similarity is calculated
- **THEN** Jaccard similarity of word sets is used
- **AND** common words / total unique words
- **AND** normalized to 0.0 - 1.0 range

### Requirement: Cascade Mode

The system SHALL execute models in priority order until quality threshold is met.

#### Scenario: Sequential execution
- **GIVEN** ensemble mode is `cascade`
- **WHEN** models are ordered by priority
- **THEN** first model is executed
- **AND** response is scored against quality_threshold
- **AND** if threshold met, return immediately

#### Scenario: Fallback on low quality
- **GIVEN** first model response scores below threshold
- **WHEN** cascade continues
- **THEN** next priority model is executed
- **AND** process repeats until threshold met or models exhausted
- **AND** if exhausted, return best scoring response

#### Scenario: Cost efficiency
- **GIVEN** cascade mode is used
- **WHEN** first model meets threshold
- **THEN** only one model is charged
- **AND** average cost is lower than parallel modes
- **AND** latency is sequential but often single-model

### Requirement: Quality Scoring

The system SHALL provide configurable quality scoring.

#### Scenario: Built-in scorers
- **GIVEN** quality metric is configured
- **WHEN** metric is `length`, `structure`, `length_and_structure`, `confidence_markers`, or `relevance`
- **THEN** corresponding built-in scorer is used
- **AND** no external dependencies required
- **AND** fast execution (<1ms per response)

#### Scenario: Custom scorer
- **GIVEN** quality metric is `custom(name)`
- **WHEN** custom scorer is registered
- **THEN** registered scorer is used for scoring
- **AND** scorer receives response and prompt
- **AND** returns f64 score (0.0 - 1.0)

#### Scenario: Confidence markers scoring
- **GIVEN** quality metric is `confidence_markers`
- **WHEN** response is scored
- **THEN** positive markers increase score ("I'm confident", "certainly", etc.)
- **AND** hedge markers decrease score ("I think", "might be", etc.)
- **AND** balanced to avoid gaming

### Requirement: Intent-Based Strategy Selection

The system SHALL select ensemble strategy based on task intent.

#### Scenario: Map intent to strategy
- **GIVEN** intent strategies are configured
- **WHEN** request has `TaskIntent::Reasoning`
- **THEN** reasoning-specific ensemble strategy is used
- **AND** includes models suited for reasoning
- **AND** may use higher timeout for complex tasks

#### Scenario: Fallback to default
- **GIVEN** no strategy for current intent
- **WHEN** ensemble is enabled
- **THEN** default ensemble mode is used
- **AND** if default is `disabled`, single model routing
- **AND** intent-specific config is optional

### Requirement: Complexity-Based Triggering

The system SHALL trigger ensemble based on prompt complexity.

#### Scenario: High complexity threshold
- **GIVEN** `complexity_threshold = 0.8`
- **WHEN** prompt complexity score >= 0.8
- **THEN** high_complexity ensemble config is used
- **AND** overrides intent-based strategy
- **AND** designed for challenging prompts

#### Scenario: Below threshold
- **GIVEN** `complexity_threshold = 0.8`
- **WHEN** prompt complexity score < 0.8
- **THEN** normal routing or intent-based ensemble
- **AND** complexity check is fast (<1ms)
- **AND** uses PromptAnalyzer from P2

### Requirement: Budget Integration

The system SHALL integrate with budget management.

#### Scenario: Pre-execution budget check
- **GIVEN** ensemble requires N models
- **WHEN** estimated total cost exceeds remaining budget
- **THEN** reduce N or fall back to single model
- **AND** never exceed max_cost_multiplier
- **AND** log budget constraint warning

#### Scenario: Cost tracking
- **GIVEN** ensemble execution completes
- **WHEN** costs are recorded
- **THEN** each model's cost is tracked individually
- **AND** total_cost_usd is reported in result
- **AND** budget manager is updated

### Requirement: Result Metadata

The system SHALL provide detailed ensemble result metadata.

#### Scenario: Include all model results
- **GIVEN** ensemble execution completes
- **WHEN** result is returned
- **THEN** all_results includes every model's outcome
- **AND** each outcome has model_id, response/error, latency, cost
- **AND** useful for debugging and analysis

#### Scenario: Include aggregation method
- **GIVEN** result is returned
- **WHEN** aggregation was performed
- **THEN** aggregation_method is included (best_of_n, voting, consensus)
- **AND** method-specific metadata included
- **AND** consensus_level for consensus mode

#### Scenario: Attribution
- **GIVEN** result is returned
- **WHEN** response is selected
- **THEN** selected_model indicates which model produced response
- **AND** important for user transparency
- **AND** useful for future model evaluation

### Requirement: FFI Export

The system SHALL expose ensemble status via UniFFI.

#### Scenario: Get ensemble configuration
- **GIVEN** ensemble is configured
- **WHEN** `get_ensemble_config()` is called via FFI
- **THEN** returns EnsembleConfigSummary record
- **AND** includes enabled status, default mode, configured strategies
- **AND** useful for settings UI display

#### Scenario: Get ensemble statistics
- **GIVEN** ensemble has been used
- **WHEN** `get_ensemble_stats()` is called via FFI
- **THEN** returns EnsembleSummaryStats record
- **AND** includes total_ensemble_calls, avg_models_per_call
- **AND** includes avg_cost_multiplier, consensus_rate

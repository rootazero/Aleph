# budget-manager Specification

## Purpose

Provides cost control and budget enforcement for AI model usage. The system tracks spending in real-time, enforces configurable limits at multiple scopes (global, project, session), and provides visibility into budget status for UI display. Users can set daily/weekly/monthly limits with soft or hard enforcement.

## ADDED Requirements

### Requirement: Budget Scope Hierarchy
The system SHALL support hierarchical budget scopes for flexible cost control.

#### Scenario: Global budget scope
- **WHEN** BudgetScope::Global is configured
- **THEN** limit applies to all API usage regardless of project or session
- **AND** all costs count against this single pool
- **AND** this is the broadest scope

#### Scenario: Project budget scope
- **WHEN** BudgetScope::Project(id) is configured
- **THEN** limit applies only to usage within that project
- **AND** different projects have independent budgets
- **AND** project scope is checked after global scope

#### Scenario: Session budget scope
- **WHEN** BudgetScope::Session(id) is configured
- **THEN** limit applies only to current conversation/session
- **AND** new sessions start with fresh budget
- **AND** session scope is most granular

#### Scenario: Model-specific budget scope
- **WHEN** BudgetScope::Model(id) is configured
- **THEN** limit applies only to usage of that specific model
- **AND** useful for capping expensive model usage
- **AND** other models are unaffected by this limit

#### Scenario: Multiple scopes apply simultaneously
- **WHEN** request matches multiple budget scopes
- **THEN** all applicable limits are checked
- **AND** most restrictive limit takes precedence
- **AND** global scope always applies if configured

### Requirement: Budget Period Configuration
The system SHALL support time-based budget periods with automatic reset.

#### Scenario: Lifetime budget (no reset)
- **WHEN** BudgetPeriod::Lifetime is configured
- **THEN** budget never automatically resets
- **AND** spending accumulates indefinitely
- **AND** only manual reset clears the budget

#### Scenario: Daily budget reset
- **WHEN** BudgetPeriod::Daily is configured
- **THEN** budget resets every 24 hours
- **AND** reset_hour specifies the hour (0-23)
- **AND** timezone is respected for reset timing

#### Scenario: Weekly budget reset
- **WHEN** BudgetPeriod::Weekly is configured
- **THEN** budget resets every 7 days
- **AND** reset_day specifies day of week (Monday=0)
- **AND** reset_hour specifies the hour
- **AND** timezone is respected

#### Scenario: Monthly budget reset
- **WHEN** BudgetPeriod::Monthly is configured
- **THEN** budget resets on specified day of month
- **AND** if day > days_in_month, reset on last day
- **AND** reset_hour specifies the hour
- **AND** timezone is respected

### Requirement: Cost Estimation
The system SHALL estimate costs before API calls to enable pre-emptive budget checks.

#### Scenario: Estimate cost from token counts
- **WHEN** CostEstimator.estimate is called with model_id and token counts
- **THEN** cost is calculated using model's pricing from profile
- **AND** input and output tokens use their respective prices
- **AND** safety_margin is applied (default: 1.2x)
- **AND** result includes base_cost_usd and with_margin_usd

#### Scenario: Load pricing from model profiles
- **WHEN** CostEstimator is initialized
- **THEN** pricing is loaded from ModelProfile.input_price_per_1m_usd
- **AND** ModelProfile.output_price_per_1m_usd
- **AND** missing prices default to Medium tier estimates
- **AND** pricing can be updated dynamically

#### Scenario: Learn pricing from actual costs
- **WHEN** actual cost is known from API response
- **THEN** CostEstimator can update its pricing model
- **AND** exponential moving average smooths updates
- **AND** learned pricing improves future estimates

### Requirement: Budget Check Before Execution
The system SHALL check budget availability before making API calls.

#### Scenario: Budget check passes (allowed)
- **WHEN** check_budget is called
- **AND** estimated_cost + spent_usd < limit_usd for all applicable limits
- **THEN** return BudgetCheckResult::Allowed
- **AND** remaining_usd reflects lowest remaining across limits

#### Scenario: Budget check triggers warning
- **WHEN** check_budget is called
- **AND** new usage would cross a warning_threshold
- **AND** that threshold hasn't been fired yet this period
- **THEN** return BudgetCheckResult::Warning
- **AND** threshold is marked as fired
- **AND** message describes which threshold was crossed

#### Scenario: Budget check blocked by soft limit
- **WHEN** check_budget is called
- **AND** estimated_cost + spent_usd > limit_usd
- **AND** enforcement is SoftBlock
- **THEN** return BudgetCheckResult::SoftBlocked
- **AND** include limit_id, spent_usd, limit_usd
- **AND** caller should not proceed with API call

#### Scenario: Budget check blocked by hard limit
- **WHEN** check_budget is called
- **AND** estimated_cost + spent_usd > limit_usd
- **AND** enforcement is HardBlock
- **THEN** return BudgetCheckResult::HardBlocked
- **AND** this is the strictest enforcement level
- **AND** no override is possible

#### Scenario: Budget check with WarnOnly enforcement
- **WHEN** check_budget is called
- **AND** estimated_cost + spent_usd > limit_usd
- **AND** enforcement is WarnOnly
- **THEN** return BudgetCheckResult::Warning (not blocked)
- **AND** warning message indicates limit exceeded
- **AND** caller may proceed at their discretion

### Requirement: Cost Recording After Execution
The system SHALL record actual costs after API calls complete.

#### Scenario: Record cost from successful call
- **WHEN** record_cost is called with CallRecord
- **AND** CallRecord.cost_usd is Some
- **THEN** add cost to all applicable BudgetState.spent_usd
- **AND** update remaining_usd and used_percent
- **AND** persist state if storage is configured

#### Scenario: Record cost triggers warning
- **WHEN** recording cost causes used_percent to cross warning_threshold
- **AND** that threshold hasn't been fired
- **THEN** emit SpendingAlert event
- **AND** mark threshold as fired in BudgetState
- **AND** alert_handler callback is invoked if configured

#### Scenario: Record cost with unknown actual cost
- **WHEN** record_cost is called
- **AND** CallRecord.cost_usd is None
- **THEN** use estimated cost as fallback
- **AND** log warning about missing actual cost
- **AND** state is still updated

### Requirement: Budget State Management
The system SHALL maintain current budget state for all configured limits.

#### Scenario: Initialize budget state
- **WHEN** BudgetManager is created with limits
- **THEN** BudgetState is created for each limit
- **AND** spent_usd starts at 0 (or loaded from storage)
- **AND** period_start is set to current period start
- **AND** next_reset is calculated from period config

#### Scenario: Get budget status for display
- **WHEN** get_status is called with scope
- **THEN** return all BudgetState matching that scope
- **AND** include global scope states
- **AND** states are sorted by used_percent descending

#### Scenario: Manual budget reset
- **WHEN** reset_limit is called with limit_id
- **THEN** BudgetState.spent_usd is set to 0
- **AND** period_start is updated to now
- **AND** next_reset is recalculated
- **AND** warnings_fired is cleared

### Requirement: Automatic Budget Reset
The system SHALL automatically reset budgets on schedule.

#### Scenario: Schedule reset timers
- **WHEN** ResetScheduler is started
- **THEN** background task is spawned for each limit with periodic reset
- **AND** task calculates sleep duration until next_reset
- **AND** task wakes and calls reset_limit at scheduled time

#### Scenario: Execute scheduled reset
- **WHEN** scheduled reset time arrives
- **THEN** BudgetState is reset (spent=0, warnings cleared)
- **AND** RouterEvent::BudgetReset is emitted
- **AND** persistence is updated if configured
- **AND** next reset time is scheduled

#### Scenario: Handle timezone correctly
- **WHEN** reset is scheduled in specific timezone
- **THEN** reset occurs at configured local time
- **AND** DST transitions are handled correctly
- **AND** "daily at midnight UTC" resets at 00:00 UTC

### Requirement: Budget Persistence
The system SHALL optionally persist budget state across restarts.

#### Scenario: Save state to storage
- **WHEN** record_cost or reset_limit is called
- **AND** BudgetStorage is configured
- **THEN** updated state is saved to storage
- **AND** save is async and non-blocking
- **AND** save failures are logged but don't block operation

#### Scenario: Load state on startup
- **WHEN** BudgetManager is created
- **AND** BudgetStorage is configured
- **THEN** previous state is loaded from storage
- **AND** stale periods are reset (if past next_reset)
- **AND** missing limits get fresh state

### Requirement: Budget-Aware Routing
The system SHALL influence model selection based on budget status.

#### Scenario: Bias toward cheaper models when budget is low
- **WHEN** route_with_budget is called
- **AND** budget_state.used_percent > 0.8
- **THEN** CostStrategy is biased toward Cheapest
- **AND** more expensive models are deprioritized
- **AND** this is a soft preference, not a hard block

#### Scenario: Normal routing when budget is healthy
- **WHEN** route_with_budget is called
- **AND** budget_state.used_percent < 0.5
- **THEN** normal routing strategy applies
- **AND** no cost bias is added
- **AND** request's cost_strategy is respected

### Requirement: Budget Events and Alerts
The system SHALL emit events for budget-related state changes.

#### Scenario: Emit warning threshold event
- **WHEN** spending crosses a warning_threshold
- **THEN** RouterEvent::BudgetWarning is emitted
- **AND** event includes threshold percent and remaining budget
- **AND** event is delivered to registered handlers

#### Scenario: Emit budget exceeded event
- **WHEN** check_budget returns Blocked result
- **THEN** RouterEvent::BudgetExceeded is emitted
- **AND** event includes limit_id, spent, and limit amounts
- **AND** event is delivered to registered handlers

#### Scenario: Emit cost recorded event
- **WHEN** record_cost completes
- **THEN** RouterEvent::CostRecorded is emitted
- **AND** event includes model_id, cost, and remaining budget
- **AND** useful for real-time UI updates

### Requirement: Configuration
The system SHALL support comprehensive budget configuration.

#### Scenario: Configure default daily limit
- **WHEN** [model_router.budget] section is in config
- **THEN** default_daily_limit_usd sets global daily limit
- **AND** default_enforcement sets enforcement mode
- **AND** warning_thresholds sets alert points

#### Scenario: Configure multiple limits
- **WHEN** [[model_router.budget.limits]] array is in config
- **THEN** each entry creates a BudgetLimit
- **AND** limits can have different scopes and periods
- **AND** limits are validated at load time

#### Scenario: Disable budget management
- **WHEN** model_router.budget.enabled = false
- **THEN** no budget checks are performed
- **AND** no budget state is maintained
- **AND** costs are still recorded in metrics (for observability)

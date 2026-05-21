# Capability: Dispatcher Resilience

## Overview

This capability defines requirements for making the Dispatcher layer resilient to failures, including graceful degradation on timeouts, unified confidence configuration, and robust error handling.

---

## ADDED Requirements

### Requirement: Graceful Timeout Degradation

The system SHALL gracefully degrade to chat mode when L3 routing times out, rather than returning errors to the user.

#### Scenario: Timeout falls back to chat
- **GIVEN** L3 routing is in progress
- **AND** the configured timeout (default 5000ms) is exceeded
- **WHEN** the timeout occurs
- **THEN** the system SHALL return `Ok(None)` instead of an error
- **AND** the user request SHALL fall back to general chat mode

#### Scenario: Log timeout degradation
- **GIVEN** L3 routing times out
- **WHEN** falling back to chat mode
- **THEN** the system SHALL log the event at WARN level
- **AND** the log SHALL include the timeout duration

#### Scenario: Provider error falls back to chat
- **GIVEN** L3 routing is in progress
- **AND** the AI provider returns an error
- **WHEN** the error is received
- **THEN** the system SHALL return `Ok(None)` instead of propagating the error
- **AND** the user request SHALL fall back to general chat mode

#### Scenario: Configurable timeout behavior
- **GIVEN** `timeout_returns_error = true` in configuration
- **WHEN** L3 routing times out
- **THEN** the system SHALL return an error instead of falling back
- **Note**: Default behavior is graceful degradation (false)

---

### Requirement: Unified Confidence Configuration

The system SHALL provide a unified, validated configuration for confidence thresholds with clear semantic hierarchy.

#### Scenario: Define confidence thresholds
- **GIVEN** dispatcher configuration
- **WHEN** configuring confidence thresholds
- **THEN** the following thresholds SHALL be supported:
  - `no_match` (default 0.3): Below this, no tool matched
  - `needs_confirmation` (default 0.7): Below this, user confirmation required
  - `auto_execute` (default 0.9): At or above, execute without confirmation

#### Scenario: Validate threshold ordering
- **GIVEN** confidence thresholds are configured
- **WHEN** the configuration is loaded
- **THEN** the system SHALL validate that `no_match < needs_confirmation < auto_execute`
- **AND** return a configuration error if validation fails

#### Scenario: Validate threshold range
- **GIVEN** confidence thresholds are configured
- **WHEN** the configuration is loaded
- **THEN** the system SHALL validate that all thresholds are in range [0.0, 1.0]

#### Scenario: Classify confidence as NoMatch
- **GIVEN** a routing result with confidence 0.2
- **AND** `no_match` threshold is 0.3
- **WHEN** classifying the confidence
- **THEN** the action SHALL be `NoMatch`

#### Scenario: Classify confidence as RequiresConfirmation
- **GIVEN** a routing result with confidence 0.5
- **AND** `no_match` is 0.3 and `needs_confirmation` is 0.7
- **WHEN** classifying the confidence
- **THEN** the action SHALL be `RequiresConfirmation`

#### Scenario: Classify confidence as OptionalConfirmation
- **GIVEN** a routing result with confidence 0.8
- **AND** `needs_confirmation` is 0.7 and `auto_execute` is 0.9
- **WHEN** classifying the confidence
- **THEN** the action SHALL be `OptionalConfirmation`

#### Scenario: Classify confidence as AutoExecute
- **GIVEN** a routing result with confidence 0.95
- **AND** `auto_execute` is 0.9
- **WHEN** classifying the confidence
- **THEN** the action SHALL be `AutoExecute`

---

### Requirement: JSON Parsing Error Recovery

The system SHALL recover gracefully from JSON parsing failures without crashing or returning errors to users.

#### Scenario: Invalid JSON falls back to chat
- **GIVEN** L3 routing receives an AI response
- **AND** the response contains no valid JSON
- **WHEN** parsing the response
- **THEN** the system SHALL return `Ok(None)`
- **AND** the user request SHALL fall back to general chat mode

#### Scenario: Log parsing failures
- **GIVEN** JSON parsing fails
- **WHEN** falling back to chat mode
- **THEN** the system SHALL log the event at WARN level
- **AND** the log SHALL include a preview of the unparseable response

#### Scenario: Try multiple extraction strategies
- **GIVEN** L3 routing receives an AI response
- **WHEN** extracting JSON from the response
- **THEN** the system SHALL try the following strategies in order:
  1. Direct JSON parse
  2. Extract from ```json code block
  3. Extract from generic ``` code block
  4. Find first complete JSON object using brace matching

---

### Requirement: Input Validation

The system SHALL validate inputs before processing to prevent unnecessary work.

#### Scenario: Skip empty input
- **GIVEN** user input is empty or whitespace only
- **WHEN** L3 routing is invoked
- **THEN** the system SHALL return `Ok(None)` immediately

#### Scenario: Skip very short input
- **GIVEN** user input is less than 3 characters
- **WHEN** L3 routing is invoked
- **THEN** the system SHALL return `Ok(None)` immediately
- **AND** the system SHALL log the skip at DEBUG level

#### Scenario: Skip when no tools available
- **GIVEN** the tool registry is empty
- **WHEN** L3 routing is invoked
- **THEN** the system SHALL return `Ok(None)` immediately
- **AND** the system SHALL log the skip at DEBUG level

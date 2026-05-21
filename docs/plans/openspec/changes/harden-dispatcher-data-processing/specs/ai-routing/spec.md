# Capability: AI Routing (Delta)

## Overview

This delta modifies the existing `ai-routing` capability to add requirements for prompt sanitization and secure routing.

---

## MODIFIED Requirements

### Requirement: L3 Routing Input Processing

The L3 router SHALL process user input securely before constructing AI prompts.

#### Scenario: Sanitize user input before prompt construction
- **GIVEN** user input is received for L3 routing
- **WHEN** constructing the AI prompt
- **THEN** the input SHALL be sanitized using the prompt injection protection module
- **AND** the sanitized input SHALL be used in the prompt

#### Scenario: Log sanitization events
- **GIVEN** user input requires sanitization (contains injection markers)
- **WHEN** sanitization is applied
- **THEN** the system SHALL log at WARN level
- **AND** the log SHALL include the fact that sanitization was applied
- **AND** the log SHALL NOT include the original unsanitized content

#### Scenario: Preserve semantic meaning after sanitization
- **GIVEN** user input "search for weather in [TASK] city"
- **WHEN** the input is sanitized
- **THEN** the semantic meaning SHALL be preserved (searching for weather)
- **AND** only the control marker SHALL be neutralized

---

## ADDED Requirements

### Requirement: Consolidated JSON Extraction

The L3 router SHALL use a single, robust JSON extraction function across all components.

#### Scenario: Use unified JSON extraction
- **GIVEN** an AI response needs JSON extraction
- **WHEN** extracting JSON in any component (prompt_builder, l3_router)
- **THEN** the same `extract_json_robust()` function SHALL be used
- **AND** no duplicate extraction logic SHALL exist

#### Scenario: Log extraction strategy used
- **GIVEN** JSON extraction is performed
- **WHEN** extraction succeeds
- **THEN** the system SHALL log which strategy was successful (direct/codeblock/brace-match)

---

### Requirement: Confidence-Based Routing Actions

The L3 router SHALL use a unified confidence classification system to determine routing actions.

#### Scenario: Use ConfidenceThresholds for classification
- **GIVEN** L3 routing produces a confidence score
- **WHEN** determining the routing action
- **THEN** the `ConfidenceThresholds.classify()` method SHALL be used
- **AND** the resulting `ConfidenceAction` SHALL determine the next step

#### Scenario: Handle NoMatch action
- **GIVEN** L3 routing returns `ConfidenceAction::NoMatch`
- **WHEN** processing the routing result
- **THEN** the system SHALL return `Ok(None)` (fall back to chat)

#### Scenario: Handle RequiresConfirmation action
- **GIVEN** L3 routing returns `ConfidenceAction::RequiresConfirmation`
- **WHEN** processing the routing result
- **THEN** the system SHALL trigger the confirmation flow via Halo UI

#### Scenario: Handle AutoExecute action
- **GIVEN** L3 routing returns `ConfidenceAction::AutoExecute`
- **WHEN** processing the routing result
- **THEN** the system SHALL execute the tool immediately without confirmation

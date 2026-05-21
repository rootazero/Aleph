# memory-dreaming Specification

## Purpose
Provide a DreamDaemon that consolidates memories during idle windows, updates the graph, and generates daily insights.

## ADDED Requirements
### Requirement: DreamDaemon Scheduling and Cancellation
The system SHALL run DreamDaemon only when memory is enabled and the user is idle, within a configurable daily window.

Dreaming config MUST include:
- enabled (bool)
- idle_threshold_seconds
- window_start_local (HH:MM)
- window_end_local (HH:MM)
- max_duration_seconds

#### Scenario: Run during idle window
- **GIVEN** Dreaming is enabled
- **AND** current time is within the window
- **AND** idle duration >= idle_threshold_seconds
- **WHEN** DreamDaemon scheduler checks
- **THEN** a dream run starts

#### Scenario: Cancel on user activity
- **GIVEN** a dream run is in progress
- **WHEN** user activity is detected
- **THEN** the dream run is cancelled gracefully
- **AND** partial results are persisted if available

---

### Requirement: Dream Pipeline Outputs
The system SHALL consolidate recent memories into summaries and update the graph.

Each dream run MUST:
- cluster recent memories (default: last 24h)
- produce a daily insight summary
- upsert graph entities/relations from the summary

#### Scenario: Create daily insight
- **WHEN** a dream run completes successfully
- **THEN** a daily insight record is stored for the run date
- **AND** includes content, source_memory_count, and created_at

---

### Requirement: Daily Insight Storage and Retrieval
The system SHALL store daily insights and provide retrieval by date.

Daily insights MUST be stored in `daily_insights` table with:
- date (YYYY-MM-DD, primary key)
- content (TEXT)
- source_memory_count (INTEGER)
- created_at (INTEGER)

#### Scenario: Retrieve latest insight
- **WHEN** get_daily_insight(date=None) is called
- **THEN** the most recent daily insight is returned

#### Scenario: Retrieve by date
- **GIVEN** date="2026-02-01"
- **WHEN** get_daily_insight(date) is called
- **THEN** the insight for that date is returned if present

---

### Requirement: Dream Decay and Pruning
The system SHALL apply decay to memory and graph scores during dream runs and prune below-threshold items.

#### Scenario: Apply decay
- **GIVEN** a graph edge with decay_score below threshold
- **WHEN** a dream run completes
- **THEN** the edge is marked inactive or removed
- **AND** a summary of pruned items is logged

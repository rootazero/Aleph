# structured-logging Specification

## Purpose

Implement privacy-aware structured logging system to enable debugging, monitoring, and user support while protecting sensitive information. Logs are stored locally with rotation, accessible via Settings UI, and exportable for bug reports.

## ADDED Requirements

### Requirement: Structured Log Initialization

The system SHALL initialize the tracing-based logging system at application startup with configurable filtering.

#### Scenario: Initialize logging on app launch

- **WHEN** AetherCore is initialized
- **THEN** `init_logging()` function is called
- **AND** tracing subscriber is configured with env filter
- **AND** log output includes timestamp, level, target, and message
- **AND** logs are written to both stdout and file

#### Scenario: Configure log level via environment

- **WHEN** `RUST_LOG` environment variable is set to "debug"
- **THEN** debug-level messages are logged
- **AND** info, warn, error levels are also logged
- **AND** trace level is excluded (too verbose)

#### Scenario: Default log level

- **WHEN** `RUST_LOG` is not set
- **THEN** log level defaults to "info"
- **AND** debug messages are not logged
- **AND** warn and error messages are always logged

### Requirement: Privacy-Protected Logging

The system SHALL scrub all personally identifiable information from log messages before writing to disk.

#### Scenario: Scrub PII from log message

- **WHEN** log message contains "User email: john@example.com"
- **THEN** logged message becomes "User email: [EMAIL]"
- **AND** PII scrubbing uses same patterns as memory module
- **AND** no email addresses appear in log files

#### Scenario: Scrub phone numbers

- **WHEN** log message contains "Contact: 123-456-7890"
- **THEN** logged message becomes "Contact: [PHONE]"
- **AND** phone number is not recoverable from logs

#### Scenario: Scrub API keys accidentally logged

- **WHEN** log message contains "API key: sk-abc123xyz"
- **THEN** logged message becomes "API key: [REDACTED]"
- **AND** API key pattern (sk-*, sk-ant-*) is detected and removed

#### Scenario: Preserve non-PII data

- **WHEN** log message contains "Processing request from app: com.apple.Notes"
- **THEN** message is logged verbatim (no PII)
- **AND** app bundle ID is preserved for debugging context

### Requirement: Log File Management with Rotation

The system SHALL manage log files with daily rotation, size limits, and automatic cleanup.

#### Scenario: Daily log rotation

- **WHEN** application runs past midnight (UTC)
- **THEN** new log file is created with date-stamped name
- **AND** file name follows pattern: `aether-YYYY-MM-DD.log`
- **AND** old log file is closed cleanly
- **AND** no log messages are lost during rotation

#### Scenario: Enforce log retention policy

- **WHEN** log files older than `log_retention_days` exist
- **THEN** old files are automatically deleted
- **AND** default retention is 7 days
- **AND** retention can be configured (1-30 days range)

#### Scenario: Enforce log size limit

- **WHEN** daily log file exceeds 50MB
- **THEN** file is rotated to `aether-YYYY-MM-DD.N.log`
- **AND** new file is started for remaining logs
- **AND** total log directory size is capped at 350MB (7 days * 50MB)

#### Scenario: Handle disk full condition

- **WHEN** log directory exceeds size limit
- **THEN** oldest log files are deleted to free space
- **AND** error is logged (if space available)
- **AND** application continues running (graceful degradation)

### Requirement: Event Logging Coverage

The system SHALL log all critical application events with appropriate log levels and context.

#### Scenario: Log hotkey trigger

- **WHEN** user presses global hotkey
- **THEN** info-level log is written: "Hotkey triggered"
- **AND** timestamp is included
- **AND** NO clipboard content is logged (privacy)

#### Scenario: Log AI provider selection

- **WHEN** router selects AI provider for request
- **THEN** info-level log: "Selected provider: OpenAI, model: gpt-4o"
- **AND** provider name and model are logged
- **AND** user input is NOT logged

#### Scenario: Log AI response latency

- **WHEN** AI provider returns response
- **THEN** info-level log: "AI response received in 1234ms"
- **AND** latency in milliseconds is logged
- **AND** response content is NOT logged

#### Scenario: Log memory retrieval

- **WHEN** memory module retrieves context
- **THEN** debug-level log: "Retrieved 3 memories for app: com.apple.Notes"
- **AND** count and app bundle ID are logged
- **AND** memory content is NOT logged

#### Scenario: Log configuration changes

- **WHEN** user modifies configuration in Settings
- **THEN** info-level log: "Configuration updated: behavior.typing_speed = 75"
- **AND** changed fields are logged
- **AND** API keys are NOT logged (even if changed)

#### Scenario: Log errors with context

- **WHEN** error occurs (e.g., API timeout)
- **THEN** error-level log: "OpenAI API timeout after 30s"
- **AND** error type and context are logged
- **AND** suggestion for recovery is included
- **AND** full error chain is logged (for debugging)

### Requirement: Performance Metrics Logging

The system SHALL log performance metrics for optimization purposes when opt-in profiling is enabled.

#### Scenario: Log pipeline stage latencies

- **WHEN** `enable_performance_logging` config is true
- **AND** AI request completes
- **THEN** debug-level log includes stage breakdown:
  - Clipboard read: Xms
  - Memory retrieval: Yms
  - AI request: Zms
  - Clipboard write: Wms
- **AND** total pipeline latency is logged

#### Scenario: Log slow operations

- **WHEN** any operation exceeds 2x target latency
- **THEN** warn-level log: "Slow operation: memory retrieval took 250ms (target: 100ms)"
- **AND** threshold and actual time are logged
- **AND** user is notified if degradation persists

#### Scenario: Skip performance logging by default

- **WHEN** `enable_performance_logging` is false (default)
- **THEN** only critical metrics are logged (total latency)
- **AND** per-stage breakdown is skipped
- **AND** logging overhead is minimized

### Requirement: Log Viewing in Settings UI

The system SHALL provide UI for viewing logs without requiring terminal access.

#### Scenario: View recent logs

- **WHEN** user clicks "View Logs" in Settings → General tab
- **THEN** modal window displays last 1000 log lines
- **AND** logs are syntax-highlighted by level (error=red, warn=yellow, info=white)
- **AND** window is searchable (Cmd+F)
- **AND** auto-scrolls to newest entries

#### Scenario: Export logs for bug reports

- **WHEN** user clicks "Export Logs" button
- **THEN** save dialog opens with filename: "aether-logs-YYYY-MM-DD.zip"
- **AND** last 3 days of logs are included
- **AND** PII scrubbing is re-applied before export
- **AND** user can attach file to GitHub issue

#### Scenario: Clear all logs

- **WHEN** user clicks "Clear Logs" button
- **THEN** confirmation dialog appears
- **AND** after confirmation, all log files are deleted
- **AND** log directory is recreated empty
- **AND** action is logged to new file

### Requirement: Logging Integration with UniFFI

The system SHALL expose logging controls to Swift via UniFFI for Settings UI integration.

#### Scenario: Get current log level

- **WHEN** Swift calls `core.get_log_level()`
- **THEN** current log level string is returned ("info", "debug", "warn", "error")
- **AND** level matches `RUST_LOG` environment variable

#### Scenario: Set log level at runtime

- **WHEN** Swift calls `core.set_log_level("debug")`
- **THEN** tracing filter is updated dynamically
- **AND** new log level takes effect immediately (no restart)
- **AND** change is persisted to config

#### Scenario: Get log file path

- **WHEN** Swift calls `core.get_log_directory()`
- **THEN** absolute path to log directory is returned
- **AND** path is `~/.config/aether/logs/`
- **AND** Swift can open directory in Finder

## MODIFIED Requirements

None - This is a new capability with no modifications to existing specs.

## References

- **Related Spec**: `memory-privacy` - Reuses PII scrubbing patterns
- **Depends On**: `tracing`, `tracing-subscriber`, `tracing-appender` crates
- **Integration**: Settings UI (GeneralSettingsView.swift)

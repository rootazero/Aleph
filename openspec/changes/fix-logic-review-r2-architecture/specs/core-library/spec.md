## ADDED Requirements

### Requirement: Concurrent Pipe Reading in Code Execution
The code executor SHALL read stdout and stderr concurrently to prevent pipe buffer deadlocks.

#### Scenario: Large output on both streams
- **WHEN** an executed command produces more than 64KB on both stdout and stderr simultaneously
- **THEN** both streams SHALL be drained concurrently without blocking

#### Scenario: One stream closes early
- **WHEN** stdout closes before stderr (or vice versa)
- **THEN** the remaining stream SHALL continue draining to completion

### Requirement: Atomic Configuration Write
The ConfigPatcher SHALL use atomic write (temp file + rename) to prevent TOCTOU races.

#### Scenario: Concurrent config modification
- **WHEN** two processes attempt to modify the configuration simultaneously
- **THEN** each write SHALL be atomic — readers see either the old or new config, never a partial write

#### Scenario: Write failure mid-operation
- **WHEN** a config write fails (disk full, permission error)
- **THEN** the original config file SHALL remain intact

### Requirement: Populated Token Usage Tracking
The agent loop SHALL populate `tokens_used` from LLM response metadata for each turn.

#### Scenario: LLM response includes usage metadata
- **WHEN** a provider returns token usage in its response
- **THEN** the agent loop state SHALL record the cumulative token count

#### Scenario: MaxTokens guard activation
- **WHEN** cumulative `tokens_used` exceeds the configured maximum
- **THEN** the agent loop SHALL terminate with a token limit exceeded error

### Requirement: Graceful Daemon Shutdown
The `handle_shutdown` IPC handler SHALL perform actual graceful shutdown including draining in-flight requests.

#### Scenario: Shutdown with active requests
- **WHEN** shutdown is requested while requests are in-flight
- **THEN** the daemon SHALL wait up to a grace period before force-terminating

#### Scenario: Clean shutdown
- **WHEN** shutdown is requested with no active requests
- **THEN** the daemon SHALL shut down immediately and return success

### Requirement: Parallel Agent Task Execution
`AgentEngine::execute()` SHALL run independent tasks concurrently using `try_join_all`.

#### Scenario: Independent tasks run in parallel
- **WHEN** multiple tasks have no dependency edges between them
- **THEN** they SHALL be dispatched concurrently

#### Scenario: One parallel task fails
- **WHEN** any parallel task fails
- **THEN** remaining tasks SHALL be cancelled and the error propagated

### Requirement: MCP Request-Response Correlation
`StdioTransport` SHALL include monotonic JSON-RPC `id` fields and correlate responses to their originating requests.

#### Scenario: Multiple concurrent requests
- **WHEN** multiple JSON-RPC requests are sent before any response arrives
- **THEN** each response SHALL be matched to its request by `id`

#### Scenario: Unmatched response
- **WHEN** a response arrives with an `id` that does not match any pending request
- **THEN** it SHALL be logged and discarded

### Requirement: Intent Classifier Unification
The intent classification system SHALL use a single pipeline instead of dual classifiers.

#### Scenario: Intent classification produces consistent results
- **WHEN** the same input is classified multiple times
- **THEN** the results SHALL be consistent (same classifier, same logic)

### Requirement: Incremental Config Save Depth
`save_incremental` SHALL support TOML structures with arbitrary nesting depth.

#### Scenario: Deeply nested config
- **WHEN** a config value is nested 5+ levels deep (e.g., `[a.b.c.d.e]`)
- **THEN** incremental save SHALL correctly preserve and update the value

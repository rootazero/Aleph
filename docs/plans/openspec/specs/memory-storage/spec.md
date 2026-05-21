# memory-storage Specification

## Purpose
TBD - created by archiving change add-contextual-memory-rag. Update Purpose after archive.
## Requirements
### Requirement: Vector Database Initialization
The system SHALL initialize an embedded vector database (SQLite + sqlite-vec extension) for storing interaction memories with vector embeddings.

#### Scenario: Initialize database on first use
- **WHEN** AlephCore starts and memory is enabled
- **THEN** the system creates `~/.aleph/memory.db` if not exists
- **AND** loads the sqlite-vec extension
- **AND** creates the `memories` table with schema:
  - `id` (TEXT, primary key, UUID)
  - `app_bundle_id` (TEXT, not null, indexed)
  - `window_title` (TEXT, not null, indexed)
  - `user_input` (TEXT, not null)
  - `ai_output` (TEXT, not null)
  - `timestamp` (INTEGER, not null, Unix timestamp)
- **AND** creates the `vec_memories` virtual table for vector search:
  - `id` (TEXT, primary key)
  - `embedding` (FLOAT[384], vector data)
- **AND** sets file permissions to 600 (owner read/write only)

#### Scenario: Handle database already exists
- **WHEN** AlephCore starts and `memory.db` already exists
- **THEN** the system opens the existing database
- **AND** verifies schema matches expected version
- **AND** applies migrations if needed (future-proofing)

#### Scenario: Handle database initialization failure
- **WHEN** database initialization fails (e.g., disk full, permission denied)
- **THEN** the system logs an error with diagnostic details
- **AND** disables memory functionality for the session
- **AND** continues normal operation without memories
- **AND** notifies user via error callback

---

### Requirement: Memory Entry Storage
The system SHALL store new interaction memories with context anchors and vector embeddings in the database.

#### Scenario: Store memory after successful AI response
- **GIVEN** a completed AI interaction with user_input and ai_output
- **AND** context captured (app_bundle_id, window_title, timestamp)
- **AND** embedding generated for concatenated text
- **WHEN** `store_memory()` is called
- **THEN** the system generates a unique UUID for the memory ID
- **AND** inserts a record into the `memories` table with all fields
- **AND** inserts the embedding into `vec_memories` table
- **AND** commits the transaction atomically
- **AND** returns success result

#### Scenario: Handle duplicate storage attempt
- **GIVEN** a memory with specific content already stored
- **WHEN** attempting to store an identical memory (same app+window+text)
- **THEN** the system allows duplicate storage (memories are not deduplicated)
- **AND** assigns a new unique ID
- **AND** stores with current timestamp
- **REASONING**: User may repeat similar queries intentionally, context matters

#### Scenario: Handle storage failure
- **WHEN** database write fails (e.g., database locked, disk error)
- **THEN** the system logs the error with context
- **AND** returns an error result
- **AND** does NOT block the user-facing AI response
- **AND** retries once after 100ms
- **AND** gives up after retry, logs final error

#### Scenario: Store memory asynchronously
- **WHEN** AI response is ready to send to user
- **THEN** storage operation runs in background tokio task
- **AND** does not block the response delivery
- **AND** completes within 200ms target (not enforced)

---

### Requirement: Memory Retrieval by Context
The system SHALL retrieve stored memories filtered by context (app_bundle_id + window_title) and ranked by vector similarity.

#### Scenario: Retrieve memories for current context
- **GIVEN** 10 memories stored for `com.apple.Notes` / "Project Plan.txt"
- **AND** 5 memories stored for `com.apple.Notes` / "Budget.txt"
- **WHEN** user queries in "Project Plan.txt" context
- **THEN** the system filters to only memories matching exact app+window
- **AND** embeds the current query text
- **AND** computes cosine similarity with all filtered embeddings
- **AND** ranks memories by similarity descending
- **AND** returns top-K memories (K from config.max_context_items)
- **AND** each memory includes similarity_score field

#### Scenario: Handle no memories available
- **GIVEN** no memories stored for current context
- **WHEN** retrieval is attempted
- **THEN** the system returns an empty list
- **AND** does not throw an error
- **AND** request proceeds without memory augmentation

#### Scenario: Apply similarity threshold
- **GIVEN** config.similarity_threshold = 0.7
- **AND** 5 memories with similarity scores: [0.9, 0.8, 0.65, 0.6, 0.5]
- **WHEN** retrieval is performed
- **THEN** only memories with score >= 0.7 are returned
- **AND** result contains 2 memories (0.9 and 0.8)
- **AND** low-relevance memories are excluded

#### Scenario: Respect max_context_items limit
- **GIVEN** config.max_context_items = 5
- **AND** 20 memories match context with high similarity
- **WHEN** retrieval is performed
- **THEN** only the top 5 most similar memories are returned
- **AND** remaining memories are not included

#### Scenario: Handle retrieval timeout
- **WHEN** vector search takes longer than 5 seconds (pathological case)
- **THEN** the system cancels the query
- **AND** returns empty memory list
- **AND** logs a warning
- **AND** request proceeds without memories

---

### Requirement: Memory Deletion
The system SHALL support selective and bulk deletion of stored memories.

#### Scenario: Delete single memory by ID
- **GIVEN** a memory with ID "abc-123" exists
- **WHEN** `delete_memory("abc-123")` is called
- **THEN** the system removes the record from `memories` table
- **AND** removes the corresponding embedding from `vec_memories`
- **AND** commits the transaction
- **AND** returns success

#### Scenario: Delete non-existent memory
- **GIVEN** no memory with ID "xyz-999" exists
- **WHEN** `delete_memory("xyz-999")` is called
- **THEN** the system returns success (idempotent)
- **AND** no error is thrown

#### Scenario: Clear all memories
- **WHEN** `clear_memories(None, None)` is called
- **THEN** the system deletes ALL records from both tables
- **AND** resets database indices
- **AND** returns count of deleted memories

#### Scenario: Clear memories by app filter
- **GIVEN** 10 memories for `com.apple.Notes` and 5 for `com.microsoft.VSCode`
- **WHEN** `clear_memories(Some("com.apple.Notes"), None)` is called
- **THEN** only memories matching the app_bundle_id are deleted
- **AND** returns count = 10
- **AND** VSCode memories remain intact

#### Scenario: Clear memories by app + window filter
- **GIVEN** 5 memories for `com.apple.Notes` / "Plan.txt"
- **AND** 3 memories for `com.apple.Notes` / "Budget.txt"
- **WHEN** `clear_memories(Some("com.apple.Notes"), Some("Plan.txt"))` is called
- **THEN** only memories matching both filters are deleted
- **AND** returns count = 5
- **AND** Budget.txt memories remain

---

### Requirement: Memory Statistics
The system SHALL provide statistics about stored memories for user visibility and management.

#### Scenario: Get memory statistics
- **WHEN** `get_memory_stats()` is called
- **THEN** the system returns a MemoryStats struct containing:
  - `total_memories` (u64): Total count of stored memories
  - `total_apps` (u64): Count of unique app_bundle_ids
  - `database_size_mb` (f64): Size of memory.db file in megabytes
  - `oldest_memory_timestamp` (i64): Unix timestamp of oldest entry
  - `newest_memory_timestamp` (i64): Unix timestamp of newest entry

#### Scenario: Stats for empty database
- **GIVEN** no memories stored
- **WHEN** `get_memory_stats()` is called
- **THEN** returns MemoryStats with:
  - total_memories = 0
  - total_apps = 0
  - database_size_mb = <file size, likely ~32KB for empty SQLite DB>
  - oldest_memory_timestamp = 0
  - newest_memory_timestamp = 0

---

### Requirement: Database Schema Versioning
The system SHALL track database schema version to support future migrations.

#### Scenario: Store schema version on creation
- **WHEN** database is first created
- **THEN** the system creates a `schema_version` table
- **AND** inserts current version number (e.g., 1)

#### Scenario: Check schema version on startup
- **WHEN** opening existing database
- **THEN** the system reads the schema_version
- **AND** compares with expected version
- **AND** applies migrations if version mismatch
- **OR** logs error if version is newer than supported

---


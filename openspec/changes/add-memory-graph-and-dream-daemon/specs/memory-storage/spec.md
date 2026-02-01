## MODIFIED Requirements
### Requirement: Memory Retrieval by Context
The system SHALL retrieve stored memories filtered by context (app_bundle_id + window_title) and optionally constrained by graph-resolved entities, then ranked by vector similarity.

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

#### Scenario: Graph disambiguation filter
- **GIVEN** resolved entity_id = E1 from graph lookup
- **AND** memory_entities links E1 to memories M1 and M2 in the current context
- **WHEN** retrieval runs with an entity hint
- **THEN** candidate memories are restricted to M1 and M2 before ranking
- **AND** if no linked memories exist, the system falls back to standard retrieval

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

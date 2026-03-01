## ADDED Requirements

### Requirement: Batched Fact Decay Processing
`apply_fact_decay` SHALL process facts in batches using cursor-based iteration to prevent OOM at scale.

#### Scenario: Large fact store
- **WHEN** the memory store contains 100,000+ facts
- **THEN** decay processing SHALL iterate in configurable batches (default 1000) without loading all facts into memory

#### Scenario: Batch processing failure
- **WHEN** a single batch fails during decay processing
- **THEN** previously processed batches SHALL remain committed and processing SHALL resume from the failed batch

### Requirement: Atomic Fact Update
`update_fact` SHALL use a single upsert operation instead of separate delete-then-insert.

#### Scenario: Concurrent fact update
- **WHEN** two updates to the same fact occur simultaneously
- **THEN** one SHALL succeed atomically — no window where the fact is temporarily missing

#### Scenario: Update failure
- **WHEN** the upsert operation fails
- **THEN** the original fact SHALL remain unchanged

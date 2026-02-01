## Context
Phase 9 (The Brain) adds a relational memory graph and a background Dream Daemon to consolidate, disambiguate, and decay memory. The goal is to reduce ambiguity in entity references and to maintain a high-quality memory state over time.

## Goals / Non-Goals
- Goals:
  - Provide a local, queryable entity-relation graph tied to memory entries.
  - Enable graph-assisted memory retrieval and disambiguation.
  - Run DreamDaemon during idle/nightly windows to summarize, update graph, and apply decay.
  - Keep all data local and PII-scrubbed.
- Non-Goals:
  - Real-time continuous graph updates on every keystroke.
  - Cloud-based storage or analysis for DreamDaemon.
  - Full knowledge-graph reasoning or SPARQL-style queries in Phase 9.

## Decisions
- Store graph tables in the existing `~/.aether/memory.db` SQLite database to avoid new dependencies.
- Represent relationships with lightweight edges and an alias index for name resolution.
- Use memory compression/fact extraction outputs as the primary signal for graph updates.
- DreamDaemon runs only when memory is enabled and user is idle, with strict time budgets and cancellation.
- Daily Insight is persisted as a lightweight record in a dedicated table for date-based retrieval.

## Risks / Trade-offs
- Graph quality depends on extraction accuracy; false links can cause wrong disambiguation.
- Background processing adds CPU/IO load; budgets and idle gating must be enforced.
- Schema changes require careful migrations within the existing memory database.

## Migration Plan
- Add new tables with forward-compatible schema and migrations.
- Keep existing memory retrieval behavior as fallback when graph lookup fails or is disabled.

## Open Questions
- Which entity kinds should be first-class (person, project, file, org, app)?
- What decay schedule and thresholds should be default for edges and nodes?
- Should Daily Insight retention follow `memory.retention_days` or its own policy?

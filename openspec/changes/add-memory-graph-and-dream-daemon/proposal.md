# Change: Add Memory Graph and Dream Daemon (Phase 9 - The Brain)

## Why
Aleph's memory is primarily linear (vector + keyword retrieval) and lacks relational structure. This causes ambiguity in entity references and weak long-term consolidation. A graph layer plus an offline Dream Daemon enables disambiguation, decay, and daily synthesis, strengthening POE (Principle–Operation–Evaluation) by providing clearer context and verifiable memory updates.

## What Changes
- Add an entity-relation memory graph stored alongside the existing memory database.
- Introduce DreamDaemon for idle/nightly consolidation: clustering, summarization, graph updates, and decay.
- Update memory retrieval to optionally use graph-based disambiguation before vector ranking.
- Add Dream status/insight storage and related configuration.

## Impact
- Affected specs: `memory-storage` (modified), `memory-graph` (new), `memory-dreaming` (new)
- Affected code: `core/src/memory/` (storage, retrieval, compression), config types, background scheduling

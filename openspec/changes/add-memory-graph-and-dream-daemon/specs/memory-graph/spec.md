# memory-graph Specification

## Purpose
Provide a local entity-relation graph that disambiguates references and links memories to entities.

## ADDED Requirements
### Requirement: Graph Storage Initialization
The system SHALL create graph storage tables inside `~/.aleph/memory.db` when memory is enabled.

Graph tables MUST include:
- `graph_nodes` (id, name, kind, aliases_json, metadata_json, created_at, updated_at, decay_score)
- `graph_edges` (id, from_id, to_id, relation, weight, confidence, context_key, created_at, updated_at, last_seen_at, decay_score)
- `graph_aliases` (alias, normalized_alias, node_id)
- `memory_entities` (memory_id, node_id, weight, source)

Indexes MUST support lookup by node kind/name, alias, and edge endpoints.

#### Scenario: Create graph tables on first use
- **WHEN** memory is enabled and the database is initialized
- **THEN** the system creates the graph tables if they do not exist
- **AND** applies migrations if a previous schema version exists

---

### Requirement: Entity and Relation Upsert
The system SHALL provide idempotent upsert APIs for graph nodes and edges.

Node upsert MUST:
- normalize the canonical name
- merge aliases without duplicates
- update `updated_at`

Edge upsert MUST:
- match on (from_id, relation, to_id, context_key)
- increment weight and update `last_seen_at`

#### Scenario: Upsert existing node
- **GIVEN** a node with kind="person" and name="Zhang"
- **WHEN** upsert_node is called with alias "Lao Zhang"
- **THEN** the existing node is updated
- **AND** alias is added without creating a new node

#### Scenario: Upsert existing edge
- **GIVEN** an edge (A)-[member_of]->(ProjectX)
- **WHEN** the same edge is upserted again
- **THEN** the edge weight increases
- **AND** last_seen_at is updated

---

### Requirement: Graph Query and Disambiguation
The system SHALL resolve entities by name or alias and optionally disambiguate using context_key.

Resolution MUST return:
- node_id
- score (0.0 - 1.0)
- reasons (alias_match, context_match, recent_activity)
- ambiguous flag

#### Scenario: Disambiguate by context
- **GIVEN** two entities with alias "Lao Zhang" in different projects
- **WHEN** resolve_entity is called with context_key="project:A"
- **THEN** the entity linked to project A is ranked highest
- **AND** ambiguous=false when score gap exceeds threshold

#### Scenario: No match
- **WHEN** resolve_entity is called with unknown name
- **THEN** the system returns an empty result list

---

### Requirement: Graph-Assisted Memory Filtering
The system SHALL use resolved entity IDs to filter memory candidates before vector ranking when entity hints are available.

#### Scenario: Filter memories by entity
- **GIVEN** resolved entity_id = E1
- **AND** memory_entities links E1 to memories M1 and M2
- **WHEN** memory retrieval runs with an entity hint
- **THEN** candidates are restricted to M1 and M2 before similarity ranking
- **AND** if no linked memories exist, the system falls back to standard retrieval

## Context

Logic Review R2 performed a full-codebase AI semantic audit of 1410 .rs files across 64 modules. While 29 files were fixed for code-level bugs (UTF-8 slicing, lock poisoning, security hardening, etc.), 14 findings require architectural or design-level changes that cannot be safely applied as simple patches.

These issues fall into five categories: security gaps, concurrency hazards, scalability limits, dead code paths, and structural design issues.

## Goals / Non-Goals

- Goals:
  - Eliminate all known pipe deadlock risks in code execution
  - Close security bypass via subshell substitution in exec parser
  - Enable memory system to handle >100K facts without OOM
  - Restore functionality of dead monitoring guards (MaxTokens, shutdown)
  - Improve provider resilience with proper rate-limit handling
- Non-Goals:
  - Full rewrite of intent classification system (unify, don't redesign)
  - Changing the MCP transport protocol (add correlation, keep JSON-RPC)
  - Adding new features — purely fixing known defects

## Decisions

### D1: Subshell Blocking Strategy
- Decision: Regex-based detection of `$()`, `` ` `` backticks, and `$(command)` patterns in exec parser
- Alternatives: Full shell AST parsing (too heavy), allowlist-only approach (breaks legitimate use)
- Rationale: Matches the existing pattern-based security model in exec/parser.rs

### D2: Concurrent Pipe Reading
- Decision: Replace sequential `read_to_string` with `tokio::join!(read_stdout, read_stderr)`
- Alternatives: Spawn separate tasks (adds complexity), use `select!` (doesn't drain both fully)
- Rationale: `tokio::join!` is the idiomatic way to drain two readers concurrently

### D3: Fact Decay Batching
- Decision: Use LanceDB scanner with `limit()` + offset-based cursor for batch iteration
- Alternatives: Stream API (LanceDB scanner already supports this), load-all (current, OOM risk)
- Rationale: LanceDB's scanner naturally supports pagination; batch size of 1000 balances memory and round-trips

### D4: ConfigPatcher Atomic Write
- Decision: Write to temp file + `rename()` for atomic swap
- Alternatives: File locking (cross-platform issues), advisory locks (not reliable on all FS)
- Rationale: Rename is atomic on POSIX; standard pattern for config file safety

### D5: Intent Classifier Unification
- Decision: Keep the more capable classifier, route through single pipeline with confidence threshold
- Alternatives: Ensemble of both (complexity), keep dual (inconsistency risk)
- Rationale: Two classifiers for the same input is a design smell; single pipeline is simpler to reason about

### D6: tokens_used Population
- Decision: Extract token count from LLM response metadata (usage.total_tokens) and propagate to agent loop state
- Alternatives: Estimate tokens from text length (inaccurate), external tokenizer (dependency)
- Rationale: All major providers return token usage in response; just need to plumb it through

## Risks / Trade-offs

- Subshell blocking regex may have false positives for legitimate `$()` in echoed strings → mitigate with allowlist escape hatch
- Parallel `AgentEngine::execute()` changes error semantics (fail-fast vs collect-all) → use `try_join_all` for fail-fast
- Intent classifier unification may regress edge cases → add A/B comparison test before removing old classifier
- ConfigPatcher atomic rename may fail on cross-filesystem moves → use same directory for temp file

## Migration Plan

1. Security fixes first (1.x tasks) — highest risk, lowest dependency
2. Concurrency fixes second (2.x) — deadlock prevention
3. Scalability fixes third (3.x) — safe with existing tests
4. Dead code fixes fourth (4.x) — enabling monitoring
5. Architecture improvements last (5.x) — largest blast radius
6. Each batch: implement → test → commit → verify full suite

Rollback: Each fix is independently revertible via git revert.

## Open Questions

- Should `handle_shutdown` drain in-flight requests or hard-stop? (Suggest: 5s grace period)
- Should `StdioTransport` correlation use u64 monotonic IDs or UUIDs? (Suggest: u64 for simplicity)
- Should fact decay batch size be configurable or hardcoded? (Suggest: configurable with 1000 default)

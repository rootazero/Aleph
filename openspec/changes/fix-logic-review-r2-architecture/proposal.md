# Change: Fix Logic Review R2 Architectural Issues

## Why

Full-codebase Logic Review Round 2 (2026-03-01) identified 14 architectural-level issues that cannot be resolved with simple code fixes. These span concurrency safety, security gates, memory scalability, and dead code paths. Left unfixed, they create real risk of pipe deadlocks, OOM crashes at scale, security bypass, and silently broken monitoring.

## What Changes

### Security
- **BREAKING**: Exec parser blocks `$()` and backtick subshell substitution
- Engine reflex/bash atomic actions require security gate before execution

### Concurrency & Deadlocks
- Code executor switches to concurrent stdout/stderr reading (prevents pipe deadlock)
- ConfigPatcher uses atomic write (rename) to eliminate TOCTOU race
- `update_fact` uses single upsert instead of non-atomic delete-then-insert

### Scalability
- `apply_fact_decay` uses cursor/batch iteration instead of loading all facts into memory
- `save_incremental` supports arbitrary nesting depth (not just 2 levels)

### Dead Code / No-Ops
- `tokens_used` is populated from LLM response metadata (fixes dead MaxTokens guard)
- `handle_shutdown` IPC performs actual graceful shutdown sequence
- `FailoverProvider::process_with_image` passes image data to inner provider

### Architecture
- `AgentEngine::execute()` runs independent tasks in parallel via `join_all`
- `StdioTransport` adds JSON-RPC `id` correlation for request-response matching
- Dual intent classifiers unified into single pipeline
- `FailoverProvider` respects `Retry-After` header on 429 responses

## Impact
- Affected specs: core-library, ai-provider-interface, memory-storage, permission-gating
- Affected code: dispatcher/, exec/, engine/, memory/, daemon/, config/, providers/, mcp/, intent/, agent_loop/
- Risk: Medium — changes touch concurrency, security, and core execution paths
- Testing: Each fix requires targeted integration tests

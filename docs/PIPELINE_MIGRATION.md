# Intent Routing Pipeline Migration Guide

This guide covers enabling and configuring the Intent Routing Pipeline, which enhances Aether's routing with caching, confidence calibration, and intelligent layer execution.

## Overview

The Intent Routing Pipeline is an optional enhancement to the existing Dispatcher Layer. When enabled, it provides:

- **Intent Cache**: LRU cache with time decay for fast-path routing
- **Confidence Calibration**: History-based confidence adjustments
- **Layer Execution Engine**: Optimized L1/L2/L3 cascade with early exit
- **Clarification Flow**: Multi-turn parameter collection
- **Conflict Detection**: Automatic detection of ambiguous intents

## Enabling the Pipeline

### Quick Start

Add to your `~/.config/aether/config.toml`:

```toml
[routing.pipeline]
enabled = true
```

That's it! The pipeline uses sensible defaults.

### Full Configuration

```toml
[routing.pipeline]
enabled = true                    # Enable the pipeline

# Cache settings
[routing.pipeline.cache]
enabled = true                    # Enable intent caching
max_size = 1000                   # Maximum cache entries
ttl_seconds = 3600                # Cache entry lifetime (1 hour)
decay_half_life_seconds = 600     # Confidence decay half-life (10 min)
cache_auto_execute_threshold = 0.85  # Cache hit auto-execute threshold

# Layer execution settings
[routing.pipeline.layers]
execution_mode = "sequential"     # "sequential" or "parallel"
l1_enabled = true                 # Enable L1 regex matching
l2_enabled = true                 # Enable L2 semantic matching
l3_enabled = true                 # Enable L3 LLM inference
l3_timeout_ms = 5000              # L3 timeout in milliseconds
l2_skip_l3_threshold = 0.85       # Skip L3 if L2 confidence >= this

# Confidence thresholds
[routing.pipeline.confidence]
auto_execute = 0.9                # Auto-execute if confidence >= this
requires_confirmation = 0.6       # Request confirmation if >= this
no_match = 0.3                    # Treat as no match if < this

# Clarification settings
[routing.pipeline.clarification]
enabled = true                    # Enable clarification flow
timeout_seconds = 300             # Clarification session timeout (5 min)
max_turns = 5                     # Maximum clarification turns

# Per-tool overrides (optional)
[[routing.pipeline.tools]]
name = "search"
min_threshold = 0.5               # Minimum confidence for this tool
auto_execute_threshold = 0.85     # Auto-execute threshold for this tool
repeat_boost = 0.1                # Confidence boost on repeat use
```

## Migration from Dispatcher Layer

### What Changes

| Aspect | Before (Dispatcher) | After (Pipeline) |
|--------|---------------------|------------------|
| Routing | Direct L1→L2→L3 | Cached + Calibrated L1→L2→L3 |
| Repeat queries | Full routing each time | Cache hit, instant response |
| Confidence | Raw from layers | Calibrated with history |
| Missing params | Error or fallback | Clarification flow |
| Conflicts | First match wins | Confirmation dialog |

### What Stays the Same

- L1/L2/L3 layer logic unchanged
- Tool registration unchanged
- Existing `[dispatcher]` config still works
- Confirmation flow for low confidence

### Compatibility

The pipeline is **backward compatible**:

1. If `[routing.pipeline].enabled = false` (default), the existing Dispatcher Layer is used
2. All existing `[dispatcher]` configuration continues to work
3. No changes required to Swift/UI code

## Feature Flag

The pipeline is controlled by a feature flag for safe rollout:

```toml
[routing.pipeline]
enabled = true   # Set to false to disable and use legacy dispatcher
```

### Gradual Rollout Strategy

1. **Testing**: Enable in development, run integration tests
2. **Canary**: Enable for small user group, monitor metrics
3. **Rollout**: Enable for all users after validation
4. **Fallback**: Set `enabled = false` to instantly revert

## Configuration Reference

### Cache Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | true | Enable/disable caching |
| `max_size` | usize | 1000 | Maximum cache entries |
| `ttl_seconds` | u64 | 3600 | Entry lifetime in seconds |
| `decay_half_life_seconds` | f32 | 600 | Confidence decay half-life |
| `cache_auto_execute_threshold` | f32 | 0.85 | Cache hit auto-execute threshold |

### Layer Config

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `execution_mode` | string | "sequential" | "sequential" or "parallel" |
| `l1_enabled` | bool | true | Enable L1 regex layer |
| `l2_enabled` | bool | true | Enable L2 semantic layer |
| `l3_enabled` | bool | true | Enable L3 LLM layer |
| `l3_timeout_ms` | u64 | 5000 | L3 timeout in milliseconds |
| `l2_skip_l3_threshold` | f32 | 0.85 | Skip L3 if L2 confidence >= this |

### Confidence Thresholds

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_execute` | f32 | 0.9 | Auto-execute threshold |
| `requires_confirmation` | f32 | 0.6 | Confirmation threshold |
| `no_match` | f32 | 0.3 | No-match threshold |

### Tool Config (per-tool overrides)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Tool name |
| `min_threshold` | f32 | 0.3 | Minimum confidence for this tool |
| `auto_execute_threshold` | f32 | 0.9 | Auto-execute threshold for this tool |
| `repeat_boost` | f32 | 0.1 | Confidence boost on repeat use |

## Troubleshooting

### Pipeline Not Activating

**Symptom**: Routing behaves like legacy dispatcher

**Check**:
1. Verify `[routing.pipeline].enabled = true` in config
2. Check logs for "IntentRoutingPipeline: initialized"
3. Ensure config file is being read (check path)

### Cache Not Working

**Symptom**: Repeat queries don't hit cache

**Check**:
1. Verify `[routing.pipeline.cache].enabled = true`
2. Check `ttl_seconds` (entries may be expired)
3. Inputs must be identical for cache hit

### L3 Timeout Issues

**Symptom**: Routing takes too long, L3 fails

**Fix**:
1. Increase `l3_timeout_ms` (default: 5000)
2. Set `l2_skip_l3_threshold` lower to skip L3 more often
3. Disable L3 with `l3_enabled = false`

### Clarification Not Working

**Symptom**: Missing params cause errors instead of clarification

**Check**:
1. Verify `[routing.pipeline.clarification].enabled = true`
2. Tool must have required parameters in schema
3. Check `timeout_seconds` for expired sessions

### Performance Issues

**Symptom**: Pipeline slower than expected

**Solutions**:
1. Enable cache: `[routing.pipeline.cache].enabled = true`
2. Use parallel mode: `execution_mode = "parallel"`
3. Increase L2 skip threshold: `l2_skip_l3_threshold = 0.8`
4. Disable L3 for speed: `l3_enabled = false`

## Metrics and Monitoring

The pipeline exposes metrics for monitoring:

```rust
// Get cache metrics
let metrics = pipeline.cache_metrics().await;
println!("Hit rate: {:.2}%", metrics.hit_rate() * 100.0);
println!("Cache size: {}/{}", metrics.size, metrics.max_size);
```

### Key Metrics

| Metric | Description | Target |
|--------|-------------|--------|
| Cache hit rate | % of requests served from cache | > 30% |
| L1 match rate | % matched by regex layer | > 40% |
| L3 skip rate | % of requests that skip L3 | > 60% |
| Clarification rate | % requiring clarification | < 5% |
| Avg latency | Average routing time | < 200ms |

## Testing

Run pipeline tests to verify configuration:

```bash
# Integration tests
cargo test tests::pipeline_integration --lib

# Performance benchmarks
cargo bench --bench pipeline_bench
```

## References

- [Architecture Documentation](./ARCHITECTURE.md#intent-routing-pipeline)
- [OpenSpec Proposal](../openspec/changes/enhance-intent-routing-pipeline/)
- [Integration Tests](../Aether/core/src/tests/pipeline_integration.rs)
- [Benchmarks](../Aether/core/benches/pipeline_bench.rs)

---

**Last Updated**: 2026-01-11
**Version**: 1.0.0

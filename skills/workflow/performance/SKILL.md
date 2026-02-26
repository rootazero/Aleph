---
name: performance
description: Performance optimization — measure first, profile, optimize with evidence
scope: standalone
---

# Performance Optimization

## When to Use

Invoke this skill when diagnosing slow performance, profiling applications, or optimizing critical paths. Always measure before optimizing.

## The Cardinal Rule

**"If you can't measure it, don't optimize it."**

Every optimization must:
1. Have a **baseline measurement** (before)
2. Target a **specific metric** (latency, throughput, memory, bundle size)
3. Show a **measurable improvement** (after)

Intuition about performance is wrong more often than right. Profile first.

## Profiling by Language

### Rust
```bash
# Benchmarking
cargo bench                        # criterion benchmarks

# CPU profiling
cargo flamegraph                   # generates flamegraph SVG
perf record -g ./target/release/app  # Linux perf

# Memory
valgrind --tool=massif ./target/release/app
heaptrack ./target/release/app
```

### JavaScript / TypeScript
```bash
# Node.js profiling
node --prof app.js                 # generate V8 log
node --prof-process isolate-*.log  # process the log

# Chrome DevTools
node --inspect app.js              # attach debugger
# Performance tab → Record → Analyze flame chart

# Bundle analysis
npx webpack-bundle-analyzer stats.json
npx vite-bundle-visualizer
```

### Python
```bash
# CPU profiling
python -m cProfile -s cumulative app.py
py-spy record -o profile.svg -- python app.py  # flamegraph

# Memory profiling
python -m tracemalloc
objgraph                           # object reference graphs

# Line-by-line
kernprof -l -v script.py           # with @profile decorator
```

## Common Optimization Patterns

### Caching

| Level | Tool | TTL | Use For |
|-------|------|-----|---------|
| In-process | HashMap/LRU | App lifetime | Computed values, config |
| Distributed | Redis | Minutes-hours | Session, API responses |
| CDN | Cloudflare/Fastly | Hours-days | Static assets, media |
| HTTP | Cache-Control headers | Varies | Browser caching |

**Cache invalidation** is the hard part. Prefer TTL-based expiry over event-based invalidation.

### Database

| Problem | Solution |
|---------|----------|
| Slow queries | Add indexes, use EXPLAIN ANALYZE |
| Too many queries | Batch operations, eager loading |
| Connection overhead | Connection pooling |
| Large result sets | Pagination, streaming |
| Hot table | Read replicas, caching layer |

### Concurrency

| Problem | Solution |
|---------|----------|
| Sequential I/O | Parallelize independent operations |
| Blocking calls in async | Move to background thread/task |
| Contention | Reduce lock scope, use lock-free structures |
| Thread overhead | Use async I/O (Tokio, asyncio, libuv) |

### Frontend

| Problem | Solution |
|---------|----------|
| Large bundle | Code splitting, tree shaking, lazy imports |
| Slow initial load | SSR/SSG, preloading critical resources |
| Layout shifts | Reserve dimensions for images/embeds |
| Render blocking | Defer non-critical JS/CSS |
| Large images | WebP/AVIF format, responsive sizing, lazy loading |

## Anti-Patterns

- **Premature optimization**: Optimizing code that isn't on the critical path
- **Optimizing without measuring**: Gut feeling is not a benchmark
- **Micro-optimizations**: Saving nanoseconds when the bottleneck is a network call
- **Caching everything**: Cache misses, staleness, and invalidation bugs
- **Adding complexity for speed**: If the simpler version is fast enough, keep it simple

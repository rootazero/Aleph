# Memory Analytics Plugin

WASM plugin (Extism PDK) that provides memory usage analytics for Aleph.

## Tools

| Tool | Description |
|------|-------------|
| `memory_stats` | Fact counts by category/type, optional decay scores |
| `memory_timeline` | Facts created per day or week |
| `memory_health` | Health assessment: stale ratio, decay, compression |
| `memory_report` | Full analytics report (markdown or JSON) |

## Build

```bash
cargo build --target wasm32-wasip1 --release
```

The host provides fact data as JSON input; the plugin computes analytics and returns results.

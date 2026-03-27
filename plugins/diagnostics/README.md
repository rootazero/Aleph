# Aleph Diagnostics Plugin

System health monitoring and performance diagnostics plugin for Aleph.

## Tools

- **system_health** — Report CPU load, memory usage, disk space, and uptime
- **perf_profile** — Timing breakdown of recent operations from an in-memory ring buffer
- **trace_export** — Export collected traces as OpenTelemetry-compatible JSON spans
- **log_analyze** — Analyze recent log patterns with level and regex filtering

## Service

- **metrics-collector** — Background service that samples system and process metrics every 30 seconds into a ring buffer

## Hooks

- **SessionStart** / **SessionEnd** — Logs session lifecycle events for diagnostics tracking

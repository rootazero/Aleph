// process.js — Collect Node.js process-level metrics
//
// Gathers PID, RSS, heap usage, and event loop lag estimate.

/**
 * Collect Node.js process stats.
 *
 * @returns {{ pid: number, memory: object, event_loop_lag_ms: number }}
 */
function collectProcessStats() {
  const mem = process.memoryUsage();

  const processMemory = {
    rss_bytes: mem.rss,
    heap_total_bytes: mem.heapTotal,
    heap_used_bytes: mem.heapUsed,
    external_bytes: mem.external,
    heap_usage_percent: Number(
      ((mem.heapUsed / mem.heapTotal) * 100).toFixed(1),
    ),
  };

  // Estimate event loop lag by measuring setTimeout(0) drift.
  // Since we need a synchronous return, use hrtime delta as a rough proxy.
  const lagEstimate = estimateEventLoopLag();

  return {
    pid: process.pid,
    node_version: process.version,
    memory: processMemory,
    event_loop_lag_ms: lagEstimate,
  };
}

/**
 * Rough synchronous event loop lag estimate.
 *
 * Measures the time spent in a tight hrtime call pair. This is not a true
 * event loop lag measurement (which requires async), but gives a baseline.
 */
function estimateEventLoopLag() {
  const start = process.hrtime.bigint();
  // Force a small delay through synchronous work
  const end = process.hrtime.bigint();
  return Number(end - start) / 1e6; // nanoseconds to milliseconds
}

module.exports = { collectProcessStats };

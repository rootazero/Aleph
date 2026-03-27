// formatters.js — Format metrics into summary, detailed, or OpenTelemetry JSON

/**
 * Format metrics as a concise human-readable summary.
 */
function formatSummary(system, proc) {
  const lines = [
    `CPU: ${system.cpu.load_avg_1m.toFixed(2)} / ${system.cpu.load_avg_5m.toFixed(2)} / ${system.cpu.load_avg_15m.toFixed(2)} (${system.cpu.cores} cores)`,
    `Memory: ${formatBytes(system.memory.used_bytes)} / ${formatBytes(system.memory.total_bytes)} (${system.memory.usage_percent}%)`,
    `Disk (/): ${system.disk.used || "N/A"} / ${system.disk.size || "N/A"} (${system.disk.usage_percent || "N/A"})`,
    `Uptime: ${system.uptime.human}`,
    `Process PID: ${proc.pid} | RSS: ${formatBytes(proc.memory.rss_bytes)} | Heap: ${proc.memory.heap_usage_percent}%`,
  ];
  return lines.join("\n");
}

/**
 * Format metrics as a detailed report with aligned tables.
 */
function formatDetailed(system, proc) {
  const sep = "-".repeat(50);
  const lines = [
    "=== System Health Report ===",
    "",
    "--- CPU ---",
    `  Load Average (1m / 5m / 15m): ${system.cpu.load_avg_1m.toFixed(2)} / ${system.cpu.load_avg_5m.toFixed(2)} / ${system.cpu.load_avg_15m.toFixed(2)}`,
    `  Cores: ${system.cpu.cores}`,
    "",
    "--- Memory ---",
    `  Total:    ${formatBytes(system.memory.total_bytes)}`,
    `  Used:     ${formatBytes(system.memory.used_bytes)}`,
    `  Free:     ${formatBytes(system.memory.free_bytes)}`,
    `  Usage:    ${system.memory.usage_percent}%`,
    "",
    "--- Disk (/) ---",
    `  Filesystem:  ${system.disk.filesystem || "N/A"}`,
    `  Size:        ${system.disk.size || "N/A"}`,
    `  Used:        ${system.disk.used || "N/A"}`,
    `  Available:   ${system.disk.available || "N/A"}`,
    `  Usage:       ${system.disk.usage_percent || "N/A"}`,
    "",
    "--- Uptime ---",
    `  ${system.uptime.human} (${system.uptime.seconds} seconds)`,
    "",
    sep,
    "",
    "--- Process ---",
    `  PID:          ${proc.pid}`,
    `  Node:         ${proc.node_version}`,
    `  RSS:          ${formatBytes(proc.memory.rss_bytes)}`,
    `  Heap Total:   ${formatBytes(proc.memory.heap_total_bytes)}`,
    `  Heap Used:    ${formatBytes(proc.memory.heap_used_bytes)}`,
    `  Heap Usage:   ${proc.memory.heap_usage_percent}%`,
    `  External:     ${formatBytes(proc.memory.external_bytes)}`,
    `  Loop Lag:     ${proc.event_loop_lag_ms.toFixed(3)} ms`,
  ];
  return lines.join("\n");
}

/**
 * Format metrics as OpenTelemetry-compatible resource spans JSON.
 */
function formatJson(system, proc) {
  const now = Date.now();
  return {
    resourceSpans: [
      {
        resource: {
          attributes: [
            { key: "service.name", value: { stringValue: "aleph-diagnostics" } },
            { key: "host.name", value: { stringValue: require("os").hostname() } },
            { key: "process.pid", value: { intValue: proc.pid } },
          ],
        },
        scopeSpans: [
          {
            scope: { name: "diagnostics.system_health", version: "0.1.0" },
            spans: [
              {
                traceId: generateTraceId(),
                spanId: generateSpanId(),
                name: "system_health_snapshot",
                kind: 1, // INTERNAL
                startTimeUnixNano: String(now * 1e6),
                endTimeUnixNano: String(now * 1e6),
                attributes: [
                  { key: "cpu.load_avg_1m", value: { doubleValue: system.cpu.load_avg_1m } },
                  { key: "cpu.cores", value: { intValue: system.cpu.cores } },
                  { key: "memory.usage_percent", value: { doubleValue: system.memory.usage_percent } },
                  { key: "memory.used_bytes", value: { intValue: system.memory.used_bytes } },
                  { key: "disk.usage_percent", value: { stringValue: system.disk.usage_percent || "N/A" } },
                  { key: "uptime.seconds", value: { intValue: system.uptime.seconds } },
                  { key: "process.rss_bytes", value: { intValue: proc.memory.rss_bytes } },
                  { key: "process.heap_usage_percent", value: { doubleValue: proc.memory.heap_usage_percent } },
                ],
                status: { code: 1 }, // OK
              },
            ],
          },
        ],
      },
    ],
  };
}

// --- Helpers ---

function formatBytes(bytes) {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return `${(bytes / Math.pow(1024, i)).toFixed(1)} ${units[i]}`;
}

function generateTraceId() {
  return randomHex(32);
}

function generateSpanId() {
  return randomHex(16);
}

function randomHex(length) {
  const chars = "0123456789abcdef";
  let result = "";
  for (let i = 0; i < length; i++) {
    result += chars[Math.floor(Math.random() * 16)];
  }
  return result;
}

module.exports = { formatSummary, formatDetailed, formatJson, formatBytes };

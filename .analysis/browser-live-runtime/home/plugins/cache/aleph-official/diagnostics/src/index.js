// diagnostics — Aleph Plugin (Node.js)
//
// Provides system health monitoring, performance profiling, trace export,
// and log analysis via JSON-RPC 2.0 over stdio.

const readline = require("readline");
const { collectSystemHealth } = require("./collectors/system");
const { collectProcessStats } = require("./collectors/process");
const { formatSummary, formatDetailed, formatJson } = require("./formatters");

// ---------------------------------------------------------------------------
// In-memory ring buffers
// ---------------------------------------------------------------------------

const MAX_BUFFER_SIZE = 1000;

/** @type {Array<{ timestamp: number, operation: string, duration_ms: number, service: string }>} */
const timingBuffer = [];

/** @type {Array<{ timestamp: number, level: string, message: string, service: string }>} */
const logBuffer = [];

/** @type {Array<{ timestamp: number, system: object, process: object }>} */
const metricsBuffer = [];

let collectorInterval = null;

// ---------------------------------------------------------------------------
// Ring buffer helpers
// ---------------------------------------------------------------------------

function pushToBuffer(buffer, entry) {
  buffer.push(entry);
  if (buffer.length > MAX_BUFFER_SIZE) {
    buffer.shift();
  }
}

function addLog(level, message, service = "diagnostics") {
  pushToBuffer(logBuffer, {
    timestamp: Date.now(),
    level,
    message,
    service,
  });
}

function addTiming(operation, durationMs, service = "diagnostics") {
  pushToBuffer(timingBuffer, {
    timestamp: Date.now(),
    operation,
    duration_ms: durationMs,
    service,
  });
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

function systemHealth(params) {
  const format = params.format || "summary";
  const start = Date.now();

  const system = collectSystemHealth();
  const proc = collectProcessStats();

  addTiming("system_health", Date.now() - start);

  if (format === "json") {
    return formatJson(system, proc);
  } else if (format === "detailed") {
    return { text: formatDetailed(system, proc) };
  } else {
    return { text: formatSummary(system, proc) };
  }
}

function perfProfile(params) {
  const lastN = params.last_n ?? 20;
  const format = params.format || "summary";
  const entries = timingBuffer.slice(-lastN);

  if (entries.length === 0) {
    return { text: "No timing data collected yet. Start the metrics-collector service or invoke tools first." };
  }

  if (format === "json") {
    return { entries };
  }

  const totalMs = entries.reduce((sum, e) => sum + e.duration_ms, 0);
  const avgMs = totalMs / entries.length;
  const maxEntry = entries.reduce((max, e) => (e.duration_ms > max.duration_ms ? e : max), entries[0]);

  if (format === "detailed") {
    const lines = [
      `=== Performance Profile (last ${entries.length} operations) ===`,
      "",
      `  Total time:  ${totalMs.toFixed(2)} ms`,
      `  Average:     ${avgMs.toFixed(2)} ms`,
      `  Slowest:     ${maxEntry.operation} (${maxEntry.duration_ms.toFixed(2)} ms)`,
      "",
      "  --- Breakdown ---",
    ];
    for (const entry of entries) {
      const ts = new Date(entry.timestamp).toISOString();
      lines.push(`  [${ts}] ${entry.operation}: ${entry.duration_ms.toFixed(2)} ms (${entry.service})`);
    }
    return { text: lines.join("\n") };
  }

  // summary
  const lines = [
    `Perf profile (${entries.length} ops): avg ${avgMs.toFixed(2)} ms, max ${maxEntry.duration_ms.toFixed(2)} ms (${maxEntry.operation})`,
  ];
  return { text: lines.join("\n") };
}

function traceExport(params) {
  const durationSecs = params.duration_secs ?? 300;
  const serviceFilter = params.service_filter || null;
  const cutoff = Date.now() - durationSecs * 1000;

  let entries = timingBuffer.filter((e) => e.timestamp >= cutoff);
  if (serviceFilter) {
    entries = entries.filter((e) => e.service === serviceFilter);
  }

  // Build OpenTelemetry-compatible spans
  const spans = entries.map((entry) => ({
    traceId: randomHex(32),
    spanId: randomHex(16),
    name: entry.operation,
    kind: 1, // INTERNAL
    startTimeUnixNano: String(entry.timestamp * 1e6),
    endTimeUnixNano: String((entry.timestamp + entry.duration_ms) * 1e6),
    attributes: [
      { key: "service.name", value: { stringValue: entry.service } },
      { key: "duration_ms", value: { doubleValue: entry.duration_ms } },
    ],
    status: { code: 1 }, // OK
  }));

  return {
    resourceSpans: [
      {
        resource: {
          attributes: [
            { key: "service.name", value: { stringValue: "aleph-diagnostics" } },
          ],
        },
        scopeSpans: [
          {
            scope: { name: "diagnostics.traces", version: "0.1.0" },
            spans,
          },
        ],
      },
    ],
    meta: {
      duration_secs: durationSecs,
      service_filter: serviceFilter,
      span_count: spans.length,
    },
  };
}

function logAnalyze(params) {
  const level = params.level || null;
  const lastN = params.last_n ?? 50;
  const pattern = params.pattern || null;

  let entries = logBuffer.slice(-lastN);

  if (level) {
    entries = entries.filter((e) => e.level === level.toLowerCase());
  }

  if (pattern) {
    try {
      const regex = new RegExp(pattern);
      entries = entries.filter((e) => regex.test(e.message));
    } catch (err) {
      return { error: `Invalid regex pattern: ${err.message}` };
    }
  }

  if (entries.length === 0) {
    return { text: "No matching log entries found." };
  }

  // Summarize by level
  const counts = {};
  for (const entry of entries) {
    counts[entry.level] = (counts[entry.level] || 0) + 1;
  }

  const lines = [
    `Log analysis (${entries.length} entries):`,
    `  Levels: ${Object.entries(counts).map(([k, v]) => `${k}=${v}`).join(", ")}`,
    "",
  ];

  for (const entry of entries) {
    const ts = new Date(entry.timestamp).toISOString();
    lines.push(`  [${ts}] [${entry.level.toUpperCase()}] ${entry.message}`);
  }

  return { text: lines.join("\n") };
}

// ---------------------------------------------------------------------------
// Hook handlers
// ---------------------------------------------------------------------------

function onSessionStart(params) {
  addLog("info", `Session started: ${params.session_id || "unknown"}`);
  return { action: "continue" };
}

function onSessionEnd(params) {
  addLog("info", `Session ended: ${params.session_id || "unknown"}`);
  return { action: "continue" };
}

// ---------------------------------------------------------------------------
// Service handlers
// ---------------------------------------------------------------------------

function startCollector(_params) {
  if (collectorInterval) {
    return { status: "already_running" };
  }

  addLog("info", "Metrics collector started (interval: 30s)");

  // Collect immediately on start
  collectAndStore();

  collectorInterval = setInterval(collectAndStore, 30000);

  return { status: "started", interval_ms: 30000 };
}

function stopCollector(_params) {
  if (!collectorInterval) {
    return { status: "not_running" };
  }

  clearInterval(collectorInterval);
  collectorInterval = null;
  addLog("info", "Metrics collector stopped");

  return { status: "stopped", metrics_collected: metricsBuffer.length };
}

function collectAndStore() {
  const start = Date.now();
  const system = collectSystemHealth();
  const proc = collectProcessStats();
  const elapsed = Date.now() - start;

  pushToBuffer(metricsBuffer, {
    timestamp: Date.now(),
    system,
    process: proc,
  });

  addTiming("metrics_collection", elapsed, "metrics-collector");
  addLog("debug", `Metrics collected in ${elapsed} ms`, "metrics-collector");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function randomHex(length) {
  const chars = "0123456789abcdef";
  let result = "";
  for (let i = 0; i < length; i++) {
    result += chars[Math.floor(Math.random() * 16)];
  }
  return result;
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 stdio listener
// ---------------------------------------------------------------------------

const HANDLER_MAP = {
  systemHealth,
  perfProfile,
  traceExport,
  logAnalyze,
  onSessionStart,
  onSessionEnd,
  startCollector,
  stopCollector,
};

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false,
});

rl.on("line", (line) => {
  let request;
  try {
    request = JSON.parse(line);
  } catch {
    writeResponse({
      jsonrpc: "2.0",
      id: null,
      error: { code: -32700, message: "Parse error" },
    });
    return;
  }

  const { id, method, params } = request;

  // Route to the correct handler
  if (method === "plugin.call") {
    const handlerName = params && params.handler;
    const handler = HANDLER_MAP[handlerName];

    if (!handler) {
      writeResponse({
        jsonrpc: "2.0",
        id,
        error: {
          code: -32601,
          message: `Unknown handler: ${handlerName}`,
        },
      });
      return;
    }

    try {
      const result = handler(params.arguments || {});
      writeResponse({ jsonrpc: "2.0", id, result });
    } catch (err) {
      writeResponse({
        jsonrpc: "2.0",
        id,
        error: {
          code: -32000,
          message: err.message || "Internal handler error",
        },
      });
    }
  } else if (method === "ping") {
    writeResponse({ jsonrpc: "2.0", id, result: "pong" });
  } else {
    writeResponse({
      jsonrpc: "2.0",
      id,
      error: { code: -32601, message: `Method not found: ${method}` },
    });
  }
});

function writeResponse(response) {
  process.stdout.write(JSON.stringify(response) + "\n");
}

// Signal readiness
writeResponse({
  jsonrpc: "2.0",
  id: "init",
  result: { status: "ready", plugin_id: "diagnostics" },
});

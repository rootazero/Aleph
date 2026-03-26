// phone-control — Aleph Plugin (Node.js)
//
// Android phone control via ADB. Communicates with the Aleph host
// over JSON-RPC 2.0 stdio.

const readline = require("readline");
const adb = require("./adb");

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

function phoneDevices() {
  const devices = adb.listDevices();
  return { devices, count: devices.length };
}

function phoneScreenshot(params) {
  const result = adb.screenshot(params.device, params.output);
  return { status: "ok", ...result };
}

function phoneTap(params) {
  if (params.x == null || params.y == null) {
    return { error: "x and y coordinates are required" };
  }
  adb.tap(params.x, params.y, params.device);
  return { status: "ok", x: params.x, y: params.y };
}

function phoneSwipe(params) {
  if (
    params.x1 == null ||
    params.y1 == null ||
    params.x2 == null ||
    params.y2 == null
  ) {
    return { error: "x1, y1, x2, y2 are all required" };
  }
  const duration = params.duration_ms ?? 300;
  adb.swipe(params.x1, params.y1, params.x2, params.y2, duration, params.device);
  return {
    status: "ok",
    from: { x: params.x1, y: params.y1 },
    to: { x: params.x2, y: params.y2 },
    duration_ms: duration,
  };
}

function phoneType(params) {
  if (!params.text) {
    return { error: "text is required" };
  }
  adb.typeText(params.text, params.device);
  return { status: "ok", text: params.text };
}

function phoneShell(params) {
  if (!params.command) {
    return { error: "command is required" };
  }
  const output = adb.shellCmd(params.command, params.device);
  return { status: "ok", output };
}

function phoneInstall(params) {
  if (!params.apk_path) {
    return { error: "apk_path is required" };
  }
  const output = adb.install(params.apk_path, params.device);
  return { status: "ok", output };
}

function phonePush(params) {
  if (!params.local_path || !params.remote_path) {
    return { error: "local_path and remote_path are required" };
  }
  const output = adb.push(params.local_path, params.remote_path, params.device);
  return { status: "ok", output };
}

function phonePull(params) {
  if (!params.remote_path) {
    return { error: "remote_path is required" };
  }
  const output = adb.pull(params.remote_path, params.local_path, params.device);
  return { status: "ok", output };
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 stdio listener
// ---------------------------------------------------------------------------

const HANDLER_MAP = {
  phoneDevices,
  phoneScreenshot,
  phoneTap,
  phoneSwipe,
  phoneType,
  phoneShell,
  phoneInstall,
  phonePush,
  phonePull,
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
  result: { status: "ready", plugin_id: "phone-control" },
});

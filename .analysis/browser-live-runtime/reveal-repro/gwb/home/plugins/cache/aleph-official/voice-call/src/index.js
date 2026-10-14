// voice-call — Aleph Plugin (Node.js)
//
// Provides a voice calling framework with WebRTC/SIP stubs and call state
// management. Communicates with the Aleph host via JSON-RPC 2.0 over stdio.

const readline = require("readline");
const { CallManager } = require("./call-manager");

// ---------------------------------------------------------------------------
// Service state
// ---------------------------------------------------------------------------

let callManager = null;

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

function callInitiate(params) {
  if (!callManager) {
    return { error: "call-manager service is not running" };
  }

  const target = params.target;
  if (!target) {
    return { error: "target is required (phone number or SIP URI)" };
  }

  try {
    const session = callManager.initiate(target, {
      caller_id: params.caller_id,
      record: params.record || false,
    });
    return { call_id: session.id, status: session.state, session };
  } catch (err) {
    return { error: err.message };
  }
}

function callAnswer(params) {
  if (!callManager) {
    return { error: "call-manager service is not running" };
  }

  const callId = params.call_id;
  if (!callId) {
    return { error: "call_id is required" };
  }

  try {
    const session = callManager.answer(callId, {
      record: params.record || false,
    });
    return { call_id: session.id, status: session.state, session };
  } catch (err) {
    return { error: err.message };
  }
}

function callHangup(params) {
  if (!callManager) {
    return { error: "call-manager service is not running" };
  }

  const callId = params.call_id;
  if (!callId) {
    return { error: "call_id is required" };
  }

  try {
    const session = callManager.hangup(callId);
    return { call_id: session.id, status: session.state, session };
  } catch (err) {
    return { error: err.message };
  }
}

function callStatus(params) {
  if (!callManager) {
    return { error: "call-manager service is not running" };
  }

  try {
    const result = callManager.getStatus(params.call_id || null);
    return result;
  } catch (err) {
    return { error: err.message };
  }
}

function callTranscribe(params) {
  if (!callManager) {
    return { error: "call-manager service is not running" };
  }

  const callId = params.call_id;
  if (!callId) {
    return { error: "call_id is required" };
  }

  try {
    const result = callManager.getTranscription(callId, params.language || "auto");
    return result;
  } catch (err) {
    return { error: err.message };
  }
}

// ---------------------------------------------------------------------------
// Service handlers
// ---------------------------------------------------------------------------

function startCallManager() {
  if (callManager && callManager._running) {
    return { status: "already_running" };
  }

  callManager = new CallManager();
  const result = callManager.start();
  return result;
}

function stopCallManager() {
  if (!callManager) {
    return { status: "not_running" };
  }

  const result = callManager.stop();
  callManager = null;
  return result;
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 stdio listener
// ---------------------------------------------------------------------------

const HANDLER_MAP = {
  callInitiate,
  callAnswer,
  callHangup,
  callStatus,
  callTranscribe,
  startCallManager,
  stopCallManager,
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
  result: { status: "ready", plugin_id: "voice-call" },
});

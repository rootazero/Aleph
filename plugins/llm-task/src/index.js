// llm-task — Aleph Plugin (Node.js)
//
// Batch LLM operations: parallel prompts, map, chain, and evaluate.
// Communicates with the Aleph host via JSON-RPC 2.0 over stdio.

const readline = require("readline");
const { runBatch, runMap, runChain, runEvaluate } = require("./batch");

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

async function llmBatch(params) {
  const { prompts, model, max_concurrent, temperature } = params;

  if (!Array.isArray(prompts) || prompts.length === 0) {
    return { error: "prompts is required and must be a non-empty array" };
  }

  return await runBatch(prompts, { model, max_concurrent, temperature });
}

async function llmMap(params) {
  const { template, items, model, max_concurrent, temperature } = params;

  if (!template) {
    return { error: "template is required" };
  }
  if (!Array.isArray(items) || items.length === 0) {
    return { error: "items is required and must be a non-empty array" };
  }

  return await runMap(template, items, { model, max_concurrent, temperature });
}

async function llmChain(params) {
  const { steps, initial_input } = params;

  if (!Array.isArray(steps) || steps.length === 0) {
    return { error: "steps is required and must be a non-empty array" };
  }

  return await runChain(steps, initial_input || "");
}

async function llmEvaluate(params) {
  const { test_cases, prompt_template, model, judge_model } = params;

  if (!Array.isArray(test_cases) || test_cases.length === 0) {
    return { error: "test_cases is required and must be a non-empty array" };
  }
  if (!prompt_template) {
    return { error: "prompt_template is required" };
  }

  return await runEvaluate(test_cases, prompt_template, { model, judge_model });
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 stdio listener
// ---------------------------------------------------------------------------

const HANDLER_MAP = {
  llmBatch,
  llmMap,
  llmChain,
  llmEvaluate,
};

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false,
});

rl.on("line", async (line) => {
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
      const result = await handler(params.arguments || {});
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
  result: { status: "ready", plugin_id: "llm-task" },
});

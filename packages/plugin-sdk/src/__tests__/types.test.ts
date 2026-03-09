// Basic assertion tests for @aleph/plugin-sdk types and helpers.
// Run with: npx tsx src/__tests__/types.test.ts

import {
  createToolResult,
  createErrorResult,
  parseRequest,
  formatResponse,
  formatErrorResponse,
} from "../index";

import type {
  PluginManifest,
  ToolDefinition,
  ToolResult,
  HookRequest,
  HookResponse,
  JsonRpcRequest,
  JsonRpcResponse,
  PluginHookEvent,
  AlephPluginEntry,
} from "../index";

let passed = 0;
let failed = 0;

function assert(condition: boolean, message: string): void {
  if (condition) {
    passed++;
  } else {
    failed++;
    console.error(`  FAIL: ${message}`);
  }
}

function assertEqual<T>(actual: T, expected: T, message: string): void {
  if (JSON.stringify(actual) === JSON.stringify(expected)) {
    passed++;
  } else {
    failed++;
    console.error(`  FAIL: ${message}`);
    console.error(`    expected: ${JSON.stringify(expected)}`);
    console.error(`    actual:   ${JSON.stringify(actual)}`);
  }
}

function assertThrows(fn: () => void, message: string): void {
  try {
    fn();
    failed++;
    console.error(`  FAIL: ${message} (expected to throw, but did not)`);
  } catch {
    passed++;
  }
}

// ============================================================================
// createToolResult tests
// ============================================================================

console.log("createToolResult:");

{
  const result = createToolResult("hello");
  assertEqual(result.content.length, 1, "should have one content block");
  assertEqual(result.content[0].type, "text", "content type should be text");
  assertEqual(result.content[0].text, "hello", "content text should match");
  assert(result.isError === undefined, "isError should be undefined");
}

{
  const result = createToolResult("");
  assertEqual(result.content[0].text, "", "should handle empty string");
}

// ============================================================================
// createErrorResult tests
// ============================================================================

console.log("createErrorResult:");

{
  const result = createErrorResult("something went wrong");
  assertEqual(result.content[0].text, "something went wrong", "error text should match");
  assertEqual(result.isError, true, "isError should be true");
}

// ============================================================================
// parseRequest tests
// ============================================================================

console.log("parseRequest:");

{
  const req = parseRequest(
    '{"jsonrpc":"2.0","id":"1","method":"tool.call","params":{"foo":"bar"}}',
  );
  assertEqual(req.jsonrpc, "2.0", "jsonrpc should be 2.0");
  assertEqual(req.id, "1", "id should match");
  assertEqual(req.method, "tool.call", "method should match");
  assertEqual((req.params as Record<string, unknown>).foo, "bar", "params should match");
}

{
  const req = parseRequest('{"jsonrpc":"2.0","id":"42","method":"hook.invoke"}');
  assertEqual(req.method, "hook.invoke", "method without params should work");
  assert(req.params === undefined, "params should be undefined when not provided");
}

{
  assertThrows(() => parseRequest(""), "should throw on empty string");
  assertThrows(() => parseRequest("not json"), "should throw on invalid JSON");
  assertThrows(
    () => parseRequest('{"jsonrpc":"1.0","id":"1","method":"test"}'),
    "should throw on wrong jsonrpc version",
  );
  assertThrows(
    () => parseRequest('{"jsonrpc":"2.0","method":"test"}'),
    "should throw when id is missing",
  );
  assertThrows(
    () => parseRequest('{"jsonrpc":"2.0","id":"1"}'),
    "should throw when method is missing",
  );
}

// ============================================================================
// formatResponse tests
// ============================================================================

console.log("formatResponse:");

{
  const line = formatResponse("1", { status: "ok" });
  const parsed: JsonRpcResponse = JSON.parse(line);
  assertEqual(parsed.jsonrpc, "2.0", "response jsonrpc should be 2.0");
  assertEqual(parsed.id, "1", "response id should match");
  assertEqual((parsed.result as Record<string, unknown>).status, "ok", "result should match");
  assert(parsed.error === undefined, "error should be undefined in success response");
}

{
  const line = formatResponse("abc", null);
  const parsed: JsonRpcResponse = JSON.parse(line);
  assertEqual(parsed.result, null, "null result should be preserved");
}

// ============================================================================
// formatErrorResponse tests
// ============================================================================

console.log("formatErrorResponse:");

{
  const line = formatErrorResponse("1", -32600, "Invalid request");
  const parsed: JsonRpcResponse = JSON.parse(line);
  assertEqual(parsed.jsonrpc, "2.0", "error response jsonrpc should be 2.0");
  assertEqual(parsed.id, "1", "error response id should match");
  assert(parsed.result === undefined, "result should be undefined in error response");
  assertEqual(parsed.error?.code, -32600, "error code should match");
  assertEqual(parsed.error?.message, "Invalid request", "error message should match");
}

{
  const line = formatErrorResponse("2", -32000, "Custom error", { detail: "extra" });
  const parsed: JsonRpcResponse = JSON.parse(line);
  assertEqual(
    (parsed.error?.data as Record<string, unknown>)?.detail,
    "extra",
    "error data should be passed through",
  );
}

// ============================================================================
// Type-level tests (compile-time checks)
// ============================================================================

console.log("Type compatibility:");

{
  // Verify PluginManifest interface works with all fields
  const manifest: PluginManifest = {
    id: "test-plugin",
    name: "Test Plugin",
    version: "0.1.0",
    description: "A test plugin",
    kind: "nodejs",
    entry: "dist/index.js",
    permissions: ["network", "filesystem:read"],
    tools: [{ name: "my_tool", description: "A tool", handler: "handleTool" }],
    hooks: [{ event: "before_tool_call", handler: "onBeforeTool", priority: 0 }],
  };
  assert(manifest.id === "test-plugin", "manifest id should be assignable");
  assert(manifest.kind === "nodejs", "manifest kind should accept 'nodejs'");
  passed++;
}

{
  // Verify ToolDefinition
  const tool: ToolDefinition = {
    name: "search",
    description: "Search the web",
    parameters: { type: "object", properties: { query: { type: "string" } } },
    handler: "handleSearch",
  };
  assert(tool.name === "search", "tool definition should work");
}

{
  // Verify HookRequest/HookResponse
  const req: HookRequest = {
    event: "before_tool_call",
    data: { tool_name: "search", params: {} },
    meta: { sessionId: "abc", timestamp: "2026-01-01T00:00:00Z" },
  };
  const resp: HookResponse = { allow: true, message: "approved" };
  assert(req.event === "before_tool_call", "hook request event should work");
  assert(resp.allow === true, "hook response allow should work");
}

{
  // Verify all PluginHookEvent values are assignable
  const events: PluginHookEvent[] = [
    "before_agent_start",
    "agent_end",
    "before_tool_call",
    "after_tool_call",
    "tool_result_persist",
    "message_received",
    "message_sending",
    "message_sent",
    "session_start",
    "session_end",
    "before_compaction",
    "after_compaction",
    "gateway_start",
    "gateway_stop",
  ];
  assertEqual(events.length, 14, "should have 14 hook events matching Rust enum");
}

{
  // Verify AlephPluginEntry type works
  const _entry: AlephPluginEntry = async (api) => {
    api.registerTool({
      name: "test",
      description: "Test tool",
      parameters: { type: "object" },
      execute: async (_id, _params) => createToolResult("ok"),
    });
  };
  passed++;
}

// ============================================================================
// Roundtrip test: parseRequest -> formatResponse
// ============================================================================

console.log("Roundtrip:");

{
  const requestLine =
    '{"jsonrpc":"2.0","id":"req-42","method":"tool.call","params":{"name":"hello"}}';
  const req = parseRequest(requestLine);
  const result = createToolResult(`Processed ${req.method}`);
  const responseLine = formatResponse(req.id, result);
  const resp: JsonRpcResponse = JSON.parse(responseLine);
  assertEqual(resp.id, "req-42", "roundtrip should preserve request id");
  assertEqual(
    ((resp.result as ToolResult).content[0] as { text: string }).text,
    "Processed tool.call",
    "roundtrip result text should match",
  );
}

// ============================================================================
// Summary
// ============================================================================

console.log(`\nResults: ${passed} passed, ${failed} failed`);
if (failed > 0) {
  process.exit(1);
} else {
  console.log("All tests passed!");
}

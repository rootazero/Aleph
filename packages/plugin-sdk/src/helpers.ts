import type { ToolResult, JsonRpcRequest, JsonRpcResponse } from "./types";

/**
 * Create a successful tool result containing a single text block.
 *
 * @param text - The text content to return
 * @returns A ToolResult with a single text content block
 *
 * @example
 * ```ts
 * const result = createToolResult("Hello, world!");
 * // { content: [{ type: "text", text: "Hello, world!" }] }
 * ```
 */
export function createToolResult(text: string): ToolResult {
  return {
    content: [{ type: "text", text }],
  };
}

/**
 * Create an error tool result.
 *
 * @param message - The error message
 * @returns A ToolResult with isError set to true
 *
 * @example
 * ```ts
 * const result = createErrorResult("File not found");
 * // { content: [{ type: "text", text: "File not found" }], isError: true }
 * ```
 */
export function createErrorResult(message: string): ToolResult {
  return {
    content: [{ type: "text", text: message }],
    isError: true,
  };
}

/**
 * Parse a JSON-RPC 2.0 request from a line of text (stdin IPC).
 *
 * Node.js plugins communicate with the Aleph host via newline-delimited
 * JSON-RPC over stdin/stdout. This function parses a single line.
 *
 * @param line - A single line of JSON text
 * @returns The parsed JsonRpcRequest
 * @throws {Error} If the line is not valid JSON or not a valid JSON-RPC request
 *
 * @example
 * ```ts
 * const req = parseRequest('{"jsonrpc":"2.0","id":"1","method":"tool.call","params":{}}');
 * console.log(req.method); // "tool.call"
 * ```
 */
export function parseRequest(line: string): JsonRpcRequest {
  const trimmed = line.trim();
  if (!trimmed) {
    throw new Error("Empty line cannot be parsed as JSON-RPC request");
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    throw new Error(`Invalid JSON in request: ${trimmed.slice(0, 100)}`);
  }

  if (typeof parsed !== "object" || parsed === null) {
    throw new Error("JSON-RPC request must be an object");
  }

  const obj = parsed as Record<string, unknown>;

  if (obj.jsonrpc !== "2.0") {
    throw new Error(`Expected jsonrpc "2.0", got "${String(obj.jsonrpc)}"`);
  }

  if (typeof obj.id !== "string") {
    throw new Error(`Expected string id, got ${typeof obj.id}`);
  }

  if (typeof obj.method !== "string") {
    throw new Error(`Expected string method, got ${typeof obj.method}`);
  }

  return {
    jsonrpc: "2.0",
    id: obj.id,
    method: obj.method,
    params: obj.params,
  };
}

/**
 * Format a JSON-RPC 2.0 success response as a single line of JSON (for stdout IPC).
 *
 * @param id - The request ID to respond to
 * @param result - The result value
 * @returns A JSON string (single line, no trailing newline)
 *
 * @example
 * ```ts
 * const line = formatResponse("1", { status: "ok" });
 * process.stdout.write(line + "\n");
 * ```
 */
export function formatResponse(id: string, result: unknown): string {
  const response: JsonRpcResponse = {
    jsonrpc: "2.0",
    id,
    result,
  };
  return JSON.stringify(response);
}

/**
 * Format a JSON-RPC 2.0 error response as a single line of JSON.
 *
 * @param id - The request ID to respond to
 * @param code - The JSON-RPC error code
 * @param message - The error message
 * @param data - Optional additional error data
 * @returns A JSON string (single line, no trailing newline)
 *
 * @example
 * ```ts
 * const line = formatErrorResponse("1", -32600, "Invalid request");
 * process.stdout.write(line + "\n");
 * ```
 */
export function formatErrorResponse(
  id: string,
  code: number,
  message: string,
  data?: unknown,
): string {
  const response: JsonRpcResponse = {
    jsonrpc: "2.0",
    id,
    error: { code, message, data },
  };
  return JSON.stringify(response);
}

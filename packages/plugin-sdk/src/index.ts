// @aleph/plugin-sdk — TypeScript SDK for building Aleph plugins
//
// Re-exports all types and helpers.

// Type exports
export type {
  // Manifest types
  PluginKind,
  PluginPermission,
  AuthorInfo,
  ConfigUiHint,
  JsonSchema,
  PluginManifest,
  ManifestToolSection,
  ManifestHookSection,
  ManifestCommandSection,
  ManifestServiceSection,
  ManifestChannelSection,
  ManifestProviderSection,
  ManifestHttpRouteSection,
  ManifestPromptSection,
  // Tool result
  ToolResultContent,
  ToolResult,
  // Registration types
  ToolDefinition,
  HookDefinition,
  PluginHookEvent,
  ChannelDefinition,
  ProviderDefinition,
  GatewayMethodDefinition,
  ServiceRegistration,
  CommandRegistration,
  HttpRouteRegistration,
  PluginRegistrationParams,
  // Hook request/response
  HookRequest,
  HookResponse,
  // JSON-RPC IPC
  JsonRpcRequest,
  JsonRpcError,
  JsonRpcResponse,
  JsonRpcNotification,
  // Plugin API
  ToolHandler,
  HookHandler,
  ToolRegistrationWithHandler,
  HookRegistrationWithHandler,
  AlephPluginApi,
  AlephPluginEntry,
} from "./types";

// Helper functions
export {
  createToolResult,
  createErrorResult,
  parseRequest,
  formatResponse,
  formatErrorResponse,
} from "./helpers";

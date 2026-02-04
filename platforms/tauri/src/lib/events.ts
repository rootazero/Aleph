import { listen, UnlistenFn } from '@tauri-apps/api/event';

// ============================================================================
// Event Payload Types
// ============================================================================

export interface StreamChunkPayload {
  text: string;
}

export interface CompletePayload {
  response: string;
}

export interface ErrorPayload {
  message: string;
}

export interface ToolStartPayload {
  tool_name: string;
}

export interface ToolResultPayload {
  tool_name: string;
  result: string;
}

export interface AgentModePayload {
  task_category: string;
  action: string;
  target: string | null;
  confidence: number;
}

export interface ToolsChangedPayload {
  tool_count: number;
}

export interface McpServerError {
  server_name: string;
  error_message: string;
}

export interface McpStartupPayload {
  succeeded: string[];
  failed: McpServerError[];
}

export interface RuntimeUpdate {
  runtime_id: string;
  current_version: string;
  latest_version: string;
}

export interface RuntimeUpdatesPayload {
  updates: RuntimeUpdate[];
}

export interface SessionPayload {
  session_id: string;
}

export interface ToolCallStartPayload {
  call_id: string;
  tool_name: string;
}

export interface ToolCallCompletePayload {
  call_id: string;
  output: string;
}

export interface ToolCallFailedPayload {
  call_id: string;
  error: string;
  is_retryable: boolean;
}

export interface LoopProgressPayload {
  session_id: string;
  iteration: number;
  status: string;
}

export interface PlanCreatedPayload {
  session_id: string;
  steps: string[];
}

export interface SessionCompletedPayload {
  session_id: string;
  summary: string;
}

export interface SubagentStartedPayload {
  parent_session_id: string;
  child_session_id: string;
  agent_id: string;
}

export interface SubagentCompletedPayload {
  child_session_id: string;
  success: boolean;
  summary: string;
}

export interface PlanTask {
  id: string;
  name: string;
  status: string;
  risk_level: string;
}

export interface PlanConfirmationPayload {
  plan_id: string;
  title: string;
  tasks: PlanTask[];
}

export interface ToolConfirmationPayload {
  confirmation_id: string;
  tool_name: string;
  description?: string;
  args: Record<string, unknown>;
}

export interface ClarificationPayload {
  clarification_id: string;
  question: string;
  options?: string[];
}

// ============================================================================
// Event Listeners
// ============================================================================

/** Event listener callback types */
export interface AlephEventHandlers {
  onThinking?: () => void;
  onStreamChunk?: (payload: StreamChunkPayload) => void;
  onComplete?: (payload: CompletePayload) => void;
  onError?: (payload: ErrorPayload) => void;
  onToolStart?: (payload: ToolStartPayload) => void;
  onToolResult?: (payload: ToolResultPayload) => void;
  onMemoryStored?: () => void;
  onAgentModeDetected?: (payload: AgentModePayload) => void;
  onToolsChanged?: (payload: ToolsChangedPayload) => void;
  onMcpStartupComplete?: (payload: McpStartupPayload) => void;
  onRuntimeUpdates?: (payload: RuntimeUpdatesPayload) => void;
  onSessionStarted?: (payload: SessionPayload) => void;
  onToolCallStarted?: (payload: ToolCallStartPayload) => void;
  onToolCallCompleted?: (payload: ToolCallCompletePayload) => void;
  onToolCallFailed?: (payload: ToolCallFailedPayload) => void;
  onLoopProgress?: (payload: LoopProgressPayload) => void;
  onPlanCreated?: (payload: PlanCreatedPayload) => void;
  onSessionCompleted?: (payload: SessionCompletedPayload) => void;
  onSubagentStarted?: (payload: SubagentStartedPayload) => void;
  onSubagentCompleted?: (payload: SubagentCompletedPayload) => void;
  onPlanConfirmationRequired?: (payload: PlanConfirmationPayload) => void;
  onToolConfirmationRequired?: (payload: ToolConfirmationPayload) => void;
  onClarificationRequired?: (payload: ClarificationPayload) => void;
}

/**
 * Subscribe to all Aether events
 * Returns an unlisten function to clean up all listeners
 */
export async function subscribeToAetherEvents(
  handlers: AlephEventHandlers
): Promise<UnlistenFn> {
  const unlisteners: UnlistenFn[] = [];

  // Core processing events
  if (handlers.onThinking) {
    unlisteners.push(await listen('aether:thinking', handlers.onThinking));
  }
  if (handlers.onStreamChunk) {
    unlisteners.push(
      await listen('aether:stream-chunk', (event) =>
        handlers.onStreamChunk!(event.payload as StreamChunkPayload)
      )
    );
  }
  if (handlers.onComplete) {
    unlisteners.push(
      await listen('aether:complete', (event) =>
        handlers.onComplete!(event.payload as CompletePayload)
      )
    );
  }
  if (handlers.onError) {
    unlisteners.push(
      await listen('aether:error', (event) =>
        handlers.onError!(event.payload as ErrorPayload)
      )
    );
  }

  // Tool events
  if (handlers.onToolStart) {
    unlisteners.push(
      await listen('aether:tool-start', (event) =>
        handlers.onToolStart!(event.payload as ToolStartPayload)
      )
    );
  }
  if (handlers.onToolResult) {
    unlisteners.push(
      await listen('aether:tool-result', (event) =>
        handlers.onToolResult!(event.payload as ToolResultPayload)
      )
    );
  }

  // Memory events
  if (handlers.onMemoryStored) {
    unlisteners.push(await listen('aether:memory-stored', handlers.onMemoryStored));
  }

  // Agent mode events
  if (handlers.onAgentModeDetected) {
    unlisteners.push(
      await listen('aether:agent-mode-detected', (event) =>
        handlers.onAgentModeDetected!(event.payload as AgentModePayload)
      )
    );
  }

  // Hot-reload events
  if (handlers.onToolsChanged) {
    unlisteners.push(
      await listen('aether:tools-changed', (event) =>
        handlers.onToolsChanged!(event.payload as ToolsChangedPayload)
      )
    );
  }
  if (handlers.onMcpStartupComplete) {
    unlisteners.push(
      await listen('aether:mcp-startup-complete', (event) =>
        handlers.onMcpStartupComplete!(event.payload as McpStartupPayload)
      )
    );
  }
  if (handlers.onRuntimeUpdates) {
    unlisteners.push(
      await listen('aether:runtime-updates', (event) =>
        handlers.onRuntimeUpdates!(event.payload as RuntimeUpdatesPayload)
      )
    );
  }

  // Session events
  if (handlers.onSessionStarted) {
    unlisteners.push(
      await listen('aether:session-started', (event) =>
        handlers.onSessionStarted!(event.payload as SessionPayload)
      )
    );
  }
  if (handlers.onToolCallStarted) {
    unlisteners.push(
      await listen('aether:tool-call-started', (event) =>
        handlers.onToolCallStarted!(event.payload as ToolCallStartPayload)
      )
    );
  }
  if (handlers.onToolCallCompleted) {
    unlisteners.push(
      await listen('aether:tool-call-completed', (event) =>
        handlers.onToolCallCompleted!(event.payload as ToolCallCompletePayload)
      )
    );
  }
  if (handlers.onToolCallFailed) {
    unlisteners.push(
      await listen('aether:tool-call-failed', (event) =>
        handlers.onToolCallFailed!(event.payload as ToolCallFailedPayload)
      )
    );
  }
  if (handlers.onLoopProgress) {
    unlisteners.push(
      await listen('aether:loop-progress', (event) =>
        handlers.onLoopProgress!(event.payload as LoopProgressPayload)
      )
    );
  }
  if (handlers.onPlanCreated) {
    unlisteners.push(
      await listen('aether:plan-created', (event) =>
        handlers.onPlanCreated!(event.payload as PlanCreatedPayload)
      )
    );
  }
  if (handlers.onSessionCompleted) {
    unlisteners.push(
      await listen('aether:session-completed', (event) =>
        handlers.onSessionCompleted!(event.payload as SessionCompletedPayload)
      )
    );
  }

  // Sub-agent events
  if (handlers.onSubagentStarted) {
    unlisteners.push(
      await listen('aether:subagent-started', (event) =>
        handlers.onSubagentStarted!(event.payload as SubagentStartedPayload)
      )
    );
  }
  if (handlers.onSubagentCompleted) {
    unlisteners.push(
      await listen('aether:subagent-completed', (event) =>
        handlers.onSubagentCompleted!(event.payload as SubagentCompletedPayload)
      )
    );
  }

  // Plan confirmation events
  if (handlers.onPlanConfirmationRequired) {
    unlisteners.push(
      await listen('aether:plan-confirmation-required', (event) =>
        handlers.onPlanConfirmationRequired!(event.payload as PlanConfirmationPayload)
      )
    );
  }

  // Tool confirmation events
  if (handlers.onToolConfirmationRequired) {
    unlisteners.push(
      await listen('aether:tool-confirmation-required', (event) =>
        handlers.onToolConfirmationRequired!(event.payload as ToolConfirmationPayload)
      )
    );
  }

  // Clarification events
  if (handlers.onClarificationRequired) {
    unlisteners.push(
      await listen('aether:clarification-required', (event) =>
        handlers.onClarificationRequired!(event.payload as ClarificationPayload)
      )
    );
  }

  // Return combined unlisten function
  return () => {
    unlisteners.forEach((unlisten) => unlisten());
  };
}

import { create } from 'zustand';
import { commands } from '@/lib/commands';
import {
  subscribeToAetherEvents,
  type AetherEventHandlers,
  type StreamChunkPayload,
  type CompletePayload,
  type ErrorPayload,
  type ToolStartPayload,
  type ToolResultPayload,
  type PlanConfirmationPayload,
} from '@/lib/events';

// ============================================================================
// Types
// ============================================================================

export interface Message {
  id: string;
  role: 'user' | 'assistant' | 'system' | 'tool';
  content: string;
  timestamp: number;
  // Tool-specific fields
  toolName?: string;
  toolResult?: string;
  // Streaming state
  isStreaming?: boolean;
}

export interface Topic {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  messageCount: number;
}

export type ProcessingState =
  | { status: 'idle' }
  | { status: 'thinking' }
  | { status: 'streaming'; content: string }
  | { status: 'tool'; toolName: string }
  | { status: 'plan-confirmation'; payload: PlanConfirmationPayload }
  | { status: 'error'; message: string };

interface ConversationStore {
  // State
  messages: Message[];
  currentTopicId: string | null;
  topics: Topic[];
  processingState: ProcessingState;
  streamingContent: string;

  // Event subscription
  unsubscribe: (() => void) | null;

  // Actions
  initialize: () => Promise<void>;
  cleanup: () => void;

  // Message operations
  sendMessage: (content: string) => Promise<void>;
  cancelProcessing: () => Promise<void>;
  clearMessages: () => void;

  // Topic operations
  createTopic: () => string;
  switchTopic: (topicId: string) => void;
  deleteCurrentTopic: () => void;

  // Internal actions (called by event handlers)
  _onThinking: () => void;
  _onStreamChunk: (payload: StreamChunkPayload) => void;
  _onComplete: (payload: CompletePayload) => void;
  _onError: (payload: ErrorPayload) => void;
  _onToolStart: (payload: ToolStartPayload) => void;
  _onToolResult: (payload: ToolResultPayload) => void;
  _onPlanConfirmation: (payload: PlanConfirmationPayload) => void;
}

// ============================================================================
// Helper Functions
// ============================================================================

function generateId(): string {
  return `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
}

// ============================================================================
// Store
// ============================================================================

export const useConversationStore = create<ConversationStore>((set, get) => ({
  messages: [],
  currentTopicId: null,
  topics: [],
  processingState: { status: 'idle' },
  streamingContent: '',
  unsubscribe: null,

  // Initialize event listeners
  initialize: async () => {
    const store = get();

    // Don't re-initialize if already subscribed
    if (store.unsubscribe) {
      return;
    }

    const handlers: AetherEventHandlers = {
      onThinking: () => get()._onThinking(),
      onStreamChunk: (payload) => get()._onStreamChunk(payload),
      onComplete: (payload) => get()._onComplete(payload),
      onError: (payload) => get()._onError(payload),
      onToolStart: (payload) => get()._onToolStart(payload),
      onToolResult: (payload) => get()._onToolResult(payload),
      onPlanConfirmationRequired: (payload) => get()._onPlanConfirmation(payload),
    };

    const unsubscribe = await subscribeToAetherEvents(handlers);
    set({ unsubscribe });

    console.log('[ConversationStore] Event listeners initialized');
  },

  // Cleanup event listeners
  cleanup: () => {
    const { unsubscribe } = get();
    if (unsubscribe) {
      unsubscribe();
      set({ unsubscribe: null });
      console.log('[ConversationStore] Event listeners cleaned up');
    }
  },

  // Send a message to the AI
  sendMessage: async (content: string) => {
    const { currentTopicId, messages } = get();

    // Create user message
    const userMessage: Message = {
      id: generateId(),
      role: 'user',
      content,
      timestamp: Date.now(),
    };

    // Add user message to state
    set({
      messages: [...messages, userMessage],
      processingState: { status: 'thinking' },
      streamingContent: '',
    });

    try {
      // Call the AI processing command
      await commands.processInput(content, currentTopicId ?? undefined, true);
    } catch (error) {
      console.error('[ConversationStore] Failed to send message:', error);
      set({
        processingState: {
          status: 'error',
          message: error instanceof Error ? error.message : 'Failed to process message',
        },
      });
    }
  },

  // Cancel current processing
  cancelProcessing: async () => {
    try {
      await commands.cancelProcessing();
      set({ processingState: { status: 'idle' } });
    } catch (error) {
      console.error('[ConversationStore] Failed to cancel:', error);
    }
  },

  // Clear all messages
  clearMessages: () => {
    set({ messages: [], streamingContent: '' });
  },

  // Create a new topic
  createTopic: () => {
    const topicId = generateId();
    const newTopic: Topic = {
      id: topicId,
      title: 'New Conversation',
      createdAt: Date.now(),
      updatedAt: Date.now(),
      messageCount: 0,
    };

    set((state) => ({
      topics: [...state.topics, newTopic],
      currentTopicId: topicId,
      messages: [],
    }));

    return topicId;
  },

  // Switch to a different topic
  switchTopic: (topicId: string) => {
    // TODO: Load messages for this topic from backend
    set({
      currentTopicId: topicId,
      messages: [],
      processingState: { status: 'idle' },
    });
  },

  // Delete current topic
  deleteCurrentTopic: () => {
    const { currentTopicId, topics } = get();
    if (!currentTopicId) return;

    set({
      topics: topics.filter((t) => t.id !== currentTopicId),
      currentTopicId: null,
      messages: [],
    });
  },

  // ============================================================================
  // Internal Event Handlers
  // ============================================================================

  _onThinking: () => {
    set({ processingState: { status: 'thinking' } });
  },

  _onStreamChunk: (payload: StreamChunkPayload) => {
    set((state) => {
      const newContent = state.streamingContent + payload.text;
      return {
        streamingContent: newContent,
        processingState: { status: 'streaming', content: newContent },
      };
    });
  },

  _onComplete: (payload: CompletePayload) => {
    const { messages, currentTopicId, topics } = get();

    // Create assistant message
    const assistantMessage: Message = {
      id: generateId(),
      role: 'assistant',
      content: payload.response,
      timestamp: Date.now(),
    };

    // Update topic title if this is the first exchange
    const currentTopic = topics.find((t) => t.id === currentTopicId);
    if (currentTopic && currentTopic.messageCount === 0 && messages.length > 0) {
      // Generate title from first user message
      const firstUserMessage = messages.find((m) => m.role === 'user');
      if (firstUserMessage) {
        commands
          .generateTopicTitle(firstUserMessage.content, payload.response)
          .then((title) => {
            set((state) => ({
              topics: state.topics.map((t) =>
                t.id === currentTopicId
                  ? { ...t, title, messageCount: t.messageCount + 2, updatedAt: Date.now() }
                  : t
              ),
            }));
          })
          .catch(console.error);
      }
    }

    set({
      messages: [...messages, assistantMessage],
      processingState: { status: 'idle' },
      streamingContent: '',
    });
  },

  _onError: (payload: ErrorPayload) => {
    set({
      processingState: { status: 'error', message: payload.message },
      streamingContent: '',
    });
  },

  _onToolStart: (payload: ToolStartPayload) => {
    const { messages } = get();

    // Add tool message
    const toolMessage: Message = {
      id: generateId(),
      role: 'tool',
      content: `Using ${payload.tool_name}...`,
      timestamp: Date.now(),
      toolName: payload.tool_name,
      isStreaming: true,
    };

    set({
      messages: [...messages, toolMessage],
      processingState: { status: 'tool', toolName: payload.tool_name },
    });
  },

  _onToolResult: (payload: ToolResultPayload) => {
    set((state) => ({
      messages: state.messages.map((m) =>
        m.toolName === payload.tool_name && m.isStreaming
          ? { ...m, content: payload.result, toolResult: payload.result, isStreaming: false }
          : m
      ),
    }));
  },

  _onPlanConfirmation: (payload: PlanConfirmationPayload) => {
    set({
      processingState: { status: 'plan-confirmation', payload },
    });
  },
}));

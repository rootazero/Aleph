import { create } from 'zustand';
import { commands } from '@/lib/commands';
import {
  subscribeToAetherEvents,
  type AlephEventHandlers,
  type StreamChunkPayload,
  type CompletePayload,
  type ErrorPayload,
  type ToolCallStartPayload,
  type ToolCallCompletePayload,
  type ToolCallFailedPayload,
  type PlanConfirmationPayload,
  type PlanCreatedPayload,
  type LoopProgressPayload,
  type SessionCompletedPayload,
  type SubagentStartedPayload,
  type SubagentCompletedPayload,
  type ToolConfirmationPayload,
  type ClarificationPayload,
} from '@/lib/events';
import type { ContentDisplayState } from '@/windows/halo/types';
import type { SystemCard } from '@/windows/halo/types';
import type { PlanStep } from '@/windows/halo/components/HaloPlanConfirmation';

// ============================================================================
// Message Types
// ============================================================================

export interface UserMessage {
  id: string;
  role: 'user';
  content: string;
  timestamp: number;
}

export interface AssistantMessage {
  id: string;
  role: 'assistant';
  content: string;
  timestamp: number;
  isStreaming?: boolean;
}

export interface SystemMessage {
  id: string;
  role: 'system';
  timestamp: number;
  card: SystemCard;
}

export type HaloMessage = UserMessage | AssistantMessage | SystemMessage;

// ============================================================================
// Command & Topic Types
// ============================================================================

export interface HaloCommand {
  key: string;
  description: string;
  icon?: string;
}

export interface HaloTopic {
  id: string;
  title: string;
  updatedAt: number;
}

// ============================================================================
// Store Interface
// ============================================================================

interface UnifiedHaloStore {
  // Display state (mutually exclusive panels)
  displayState: ContentDisplayState;

  // Input state
  inputText: string;

  // Conversation state
  messages: HaloMessage[];
  currentTopicId: string | null;
  isProcessing: boolean;
  streamingContent: string;

  // Tool call tracking (call_id -> card_id)
  toolCallCards: Map<string, string>;

  // Subagent tracking (child_session_id -> card_id)
  subagentCards: Map<string, string>;

  // Plan progress tracking (plan_id -> card_id)
  planProgressCards: Map<string, string>;

  // Confirmation tracking (confirmation_id -> card_id)
  toolConfirmationCards: Map<string, string>;
  clarificationCards: Map<string, string>;

  // Command list state
  commands: HaloCommand[];
  filteredCommands: HaloCommand[];
  selectedCommandIndex: number;

  // Topic list state
  topics: HaloTopic[];
  filteredTopics: HaloTopic[];
  selectedTopicIndex: number;

  // Window state
  visible: boolean;

  // Event subscription
  unsubscribe: (() => void) | null;

  // Actions
  setInputText: (text: string) => void;
  handleInputChange: (text: string) => void;

  // Conversation actions
  sendMessage: () => Promise<void>;
  addUserMessage: (content: string) => void;
  addAssistantMessage: (content: string) => void;
  startStreaming: () => void;
  updateStreamingContent: (content: string) => void;
  finishStreaming: () => void;

  // System card actions
  addSystemCard: (card: SystemCard) => string;
  updateSystemCard: (id: string, card: Partial<SystemCard>) => void;
  removeSystemCard: (id: string) => void;

  // Card interaction callbacks
  handleToolConfirmation: (id: string, approved: boolean) => void;
  handlePlanConfirmation: (id: string, approved: boolean) => void;
  handleClarificationResponse: (id: string, response: string) => void;
  handleErrorRetry: (id: string) => void;
  handleCardDismiss: (id: string) => void;

  // Command actions
  loadCommands: () => void;
  filterCommands: (query: string) => void;
  selectCommand: (command: HaloCommand) => void;
  moveCommandSelection: (direction: 'up' | 'down') => void;

  // Topic actions
  loadTopics: () => void;
  filterTopics: (query: string) => void;
  selectTopic: (topic: HaloTopic) => void;
  moveTopicSelection: (direction: 'up' | 'down') => void;

  // Window actions
  show: () => Promise<void>;
  hide: () => void;
  handleEscape: () => void;

  // Initialization
  initialize: () => Promise<void>;
  cleanup: () => void;
}

function generateId(): string {
  return `${Date.now()}-${Math.random().toString(36).substr(2, 9)}`;
}

export const useUnifiedHaloStore = create<UnifiedHaloStore>((set, get) => ({
  // Initial state
  displayState: { type: 'empty' },
  inputText: '',
  messages: [],
  currentTopicId: null,
  isProcessing: false,
  streamingContent: '',
  toolCallCards: new Map(),
  subagentCards: new Map(),
  planProgressCards: new Map(),
  toolConfirmationCards: new Map(),
  clarificationCards: new Map(),
  commands: [],
  filteredCommands: [],
  selectedCommandIndex: 0,
  topics: [],
  filteredTopics: [],
  selectedTopicIndex: 0,
  visible: false,
  unsubscribe: null,

  setInputText: (text) => set({ inputText: text }),

  handleInputChange: (text) => {
    const { messages, loadCommands, filterCommands, loadTopics, filterTopics } = get();
    set({ inputText: text });

    // Determine display state based on input prefix
    if (text.startsWith('//')) {
      set({ displayState: { type: 'topicList', prefix: '//' } });
      loadTopics();
      filterTopics(text.slice(2));
    } else if (text.startsWith('/')) {
      set({ displayState: { type: 'commandList', prefix: '/' } });
      loadCommands();
      filterCommands(text.slice(1));
    } else if (messages.length > 0) {
      set({ displayState: { type: 'conversation' } });
    } else {
      set({ displayState: { type: 'empty' } });
    }
  },

  // ============================================================================
  // Conversation Actions
  // ============================================================================

  sendMessage: async () => {
    const { inputText, currentTopicId, addUserMessage } = get();
    const trimmed = inputText.trim();
    if (!trimmed || trimmed.startsWith('/')) return;

    addUserMessage(trimmed);
    set({
      inputText: '',
      isProcessing: true,
      displayState: { type: 'conversation' },
    });

    try {
      await commands.processInput(trimmed, currentTopicId ?? undefined, true);
    } catch (error) {
      console.error('Failed to send message:', error);
      set({ isProcessing: false });
    }
  },

  addUserMessage: (content) => {
    const message: UserMessage = {
      id: generateId(),
      role: 'user',
      content,
      timestamp: Date.now(),
    };
    set((state) => ({
      messages: [...state.messages, message],
      displayState: { type: 'conversation' },
    }));
  },

  addAssistantMessage: (content) => {
    const message: AssistantMessage = {
      id: generateId(),
      role: 'assistant',
      content,
      timestamp: Date.now(),
    };
    set((state) => ({
      messages: [...state.messages, message],
      isProcessing: false,
    }));
  },

  startStreaming: () => {
    const streamingMessage: AssistantMessage = {
      id: generateId(),
      role: 'assistant',
      content: '',
      timestamp: Date.now(),
      isStreaming: true,
    };
    set((state) => ({
      messages: [...state.messages, streamingMessage],
      streamingContent: '',
    }));
  },

  updateStreamingContent: (content) => {
    set((state) => ({
      streamingContent: content,
      messages: state.messages.map((m) =>
        m.role === 'assistant' && 'isStreaming' in m && m.isStreaming
          ? { ...m, content }
          : m
      ),
    }));
  },

  finishStreaming: () => {
    set((state) => ({
      isProcessing: false,
      streamingContent: '',
      messages: state.messages.map((m) =>
        m.role === 'assistant' && 'isStreaming' in m && m.isStreaming
          ? { ...m, isStreaming: false }
          : m
      ),
    }));
  },

  // ============================================================================
  // System Card Actions
  // ============================================================================

  addSystemCard: (card) => {
    const id = generateId();
    const message: SystemMessage = {
      id,
      role: 'system',
      timestamp: Date.now(),
      card,
    };
    set((state) => ({
      messages: [...state.messages, message],
      displayState: { type: 'conversation' },
    }));
    return id;
  },

  updateSystemCard: (id, cardUpdate) => {
    set((state) => ({
      messages: state.messages.map((m) => {
        if (m.id === id && m.role === 'system') {
          return {
            ...m,
            card: { ...m.card, ...cardUpdate } as SystemCard,
          };
        }
        return m;
      }),
    }));
  },

  removeSystemCard: (id) => {
    set((state) => ({
      messages: state.messages.filter((m) => m.id !== id),
    }));
  },

  // ============================================================================
  // Card Interaction Callbacks
  // ============================================================================

  handleToolConfirmation: (id, approved) => {
    const { messages, removeSystemCard, toolConfirmationCards } = get();
    const message = messages.find((m) => m.id === id);
    if (!message || message.role !== 'system' || message.card.type !== 'toolConfirmation') {
      return;
    }

    // Find confirmation_id by card_id
    let confirmationId: string | undefined;
    for (const [confId, cardId] of toolConfirmationCards.entries()) {
      if (cardId === id) {
        confirmationId = confId;
        break;
      }
    }

    // Send confirmation to backend
    if (confirmationId) {
      commands.respondToolConfirmation(confirmationId, approved).catch(console.error);
      set((state) => {
        const newMap = new Map(state.toolConfirmationCards);
        newMap.delete(confirmationId!);
        return { toolConfirmationCards: newMap };
      });
    }
    removeSystemCard(id);
  },

  handlePlanConfirmation: (id, approved) => {
    const { messages, removeSystemCard, addSystemCard } = get();
    const message = messages.find((m) => m.id === id);
    if (!message || message.role !== 'system' || message.card.type !== 'planConfirmation') {
      return;
    }

    // TODO: Find plan_id from tracking map when backend emits it
    // For now, we don't have a plan_id in the confirmation card

    if (approved) {
      // Convert to progress card
      const steps = message.card.steps.map((s, i) => ({
        ...s,
        status: i === 0 ? 'running' : 'pending',
      })) as PlanStep[];
      removeSystemCard(id);
      addSystemCard({ type: 'planProgress', steps, currentIndex: 0 });
    } else {
      removeSystemCard(id);
    }
  },

  handleClarificationResponse: (id, response) => {
    const { removeSystemCard, clarificationCards } = get();

    // Find clarification_id by card_id
    let clarificationId: string | undefined;
    for (const [clarId, cardId] of clarificationCards.entries()) {
      if (cardId === id) {
        clarificationId = clarId;
        break;
      }
    }

    // Send response to backend
    if (clarificationId) {
      commands.respondClarification(clarificationId, response).catch(console.error);
      set((state) => {
        const newMap = new Map(state.clarificationCards);
        newMap.delete(clarificationId!);
        return { clarificationCards: newMap };
      });
    }
    removeSystemCard(id);
  },

  handleErrorRetry: (id) => {
    const { removeSystemCard } = get();
    removeSystemCard(id);
    // TODO: Re-send the last user message
  },

  handleCardDismiss: (id) => {
    get().removeSystemCard(id);
  },

  // ============================================================================
  // Command Actions
  // ============================================================================

  loadCommands: () => {
    const defaultCommands: HaloCommand[] = [
      { key: 'clear', description: 'Clear conversation history' },
      { key: 'settings', description: 'Open settings window' },
      { key: 'memory', description: 'Search memory' },
      { key: 'tools', description: 'List available tools' },
      { key: 'help', description: 'Show help' },
    ];
    set({ commands: defaultCommands, filteredCommands: defaultCommands });
  },

  filterCommands: (query) => {
    const { commands } = get();
    if (!query) {
      set({ filteredCommands: commands, selectedCommandIndex: 0 });
      return;
    }
    const lowerQuery = query.toLowerCase();
    const filtered = commands.filter(
      (cmd) =>
        cmd.key.toLowerCase().includes(lowerQuery) ||
        cmd.description.toLowerCase().includes(lowerQuery)
    );
    set({ filteredCommands: filtered, selectedCommandIndex: 0 });
  },

  selectCommand: (command) => {
    switch (command.key) {
      case 'clear':
        set({ messages: [], displayState: { type: 'empty' }, inputText: '' });
        break;
      case 'settings':
        commands.openSettingsWindow();
        get().hide();
        break;
      default:
        set({ inputText: `/${command.key} `, displayState: { type: 'empty' } });
    }
  },

  moveCommandSelection: (direction) => {
    const { filteredCommands, selectedCommandIndex } = get();
    if (filteredCommands.length === 0) return;

    const newIndex =
      direction === 'up'
        ? Math.max(0, selectedCommandIndex - 1)
        : Math.min(filteredCommands.length - 1, selectedCommandIndex + 1);
    set({ selectedCommandIndex: newIndex });
  },

  // ============================================================================
  // Topic Actions
  // ============================================================================

  loadTopics: () => {
    const mockTopics: HaloTopic[] = [
      { id: '1', title: 'Recent conversation', updatedAt: Date.now() - 3600000 },
      { id: '2', title: 'Code review discussion', updatedAt: Date.now() - 86400000 },
    ];
    set({ topics: mockTopics, filteredTopics: mockTopics });
  },

  filterTopics: (query) => {
    const { topics } = get();
    if (!query) {
      set({ filteredTopics: topics, selectedTopicIndex: 0 });
      return;
    }
    const lowerQuery = query.toLowerCase();
    const filtered = topics.filter((t) => t.title.toLowerCase().includes(lowerQuery));
    set({ filteredTopics: filtered, selectedTopicIndex: 0 });
  },

  selectTopic: (topic) => {
    set({
      currentTopicId: topic.id,
      inputText: '',
      displayState: { type: 'conversation' },
      messages: [],
    });
  },

  moveTopicSelection: (direction) => {
    const { filteredTopics, selectedTopicIndex } = get();
    if (filteredTopics.length === 0) return;

    const newIndex =
      direction === 'up'
        ? Math.max(0, selectedTopicIndex - 1)
        : Math.min(filteredTopics.length - 1, selectedTopicIndex + 1);
    set({ selectedTopicIndex: newIndex });
  },

  // ============================================================================
  // Window Actions
  // ============================================================================

  show: async () => {
    set({ visible: true });
  },

  hide: () => {
    set({
      visible: false,
      inputText: '',
      displayState: { type: 'empty' },
    });
    commands.hideHaloWindow().catch(console.error);
  },

  handleEscape: () => {
    const { displayState, hide, messages } = get();

    if (displayState.type === 'commandList' || displayState.type === 'topicList') {
      set({
        inputText: '',
        displayState: messages.length > 0 ? { type: 'conversation' } : { type: 'empty' },
      });
    } else {
      hide();
    }
  },

  // ============================================================================
  // Initialization
  // ============================================================================

  initialize: async () => {
    const store = get();
    if (store.unsubscribe) return;

    const handlers: AlephEventHandlers = {
      onThinking: () => set({ isProcessing: true }),

      onStreamChunk: (payload: StreamChunkPayload) => {
        const { messages, streamingContent } = get();
        const newContent = streamingContent + payload.text;

        const hasStreamingMessage = messages.some(
          (m) => m.role === 'assistant' && 'isStreaming' in m && m.isStreaming
        );
        if (!hasStreamingMessage) {
          get().startStreaming();
        }
        get().updateStreamingContent(newContent);
      },

      onComplete: (payload: CompletePayload) => {
        const { messages } = get();
        const hasStreamingMessage = messages.some(
          (m) => m.role === 'assistant' && 'isStreaming' in m && m.isStreaming
        );

        if (hasStreamingMessage) {
          get().finishStreaming();
        } else {
          get().addAssistantMessage(payload.response);
        }
      },

      onError: (payload: ErrorPayload) => {
        console.error('AI Error:', payload.message);
        set({ isProcessing: false });
        get().addSystemCard({
          type: 'error',
          message: payload.message,
          canRetry: true,
        });
      },

      // Tool call lifecycle
      onToolCallStarted: (payload: ToolCallStartPayload) => {
        const cardId = get().addSystemCard({
          type: 'processing',
          content: `Running ${payload.tool_name}...`,
        });
        // Track card by call_id for later cleanup
        set((state) => {
          const newMap = new Map(state.toolCallCards);
          newMap.set(payload.call_id, cardId);
          return { toolCallCards: newMap };
        });
      },

      onToolCallCompleted: (payload: ToolCallCompletePayload) => {
        const { toolCallCards, removeSystemCard } = get();
        const cardId = toolCallCards.get(payload.call_id);
        if (cardId) {
          removeSystemCard(cardId);
          set((state) => {
            const newMap = new Map(state.toolCallCards);
            newMap.delete(payload.call_id);
            return { toolCallCards: newMap };
          });
        }
      },

      onToolCallFailed: (payload: ToolCallFailedPayload) => {
        const { toolCallCards, removeSystemCard, addSystemCard } = get();
        const cardId = toolCallCards.get(payload.call_id);
        if (cardId) {
          removeSystemCard(cardId);
          set((state) => {
            const newMap = new Map(state.toolCallCards);
            newMap.delete(payload.call_id);
            return { toolCallCards: newMap };
          });
        }
        addSystemCard({
          type: 'error',
          message: payload.error,
          canRetry: payload.is_retryable,
        });
      },

      // Memory events
      onMemoryStored: () => {
        get().addSystemCard({
          type: 'toast',
          level: 'info',
          message: 'Memory saved',
        });
      },

      // Plan events
      onPlanConfirmationRequired: (payload: PlanConfirmationPayload) => {
        const steps: PlanStep[] = payload.tasks.map((task) => ({
          id: task.id,
          title: task.name,
          status: 'pending' as const,
        }));
        get().addSystemCard({
          type: 'planConfirmation',
          steps,
        });
      },

      onPlanCreated: (payload: PlanCreatedPayload) => {
        const steps: PlanStep[] = payload.steps.map((step, i) => ({
          id: `step-${i}`,
          title: step,
          status: i === 0 ? 'running' : ('pending' as const),
        }));
        const cardId = get().addSystemCard({
          type: 'planProgress',
          steps,
          currentIndex: 0,
        });
        set((state) => {
          const newMap = new Map(state.planProgressCards);
          newMap.set(payload.session_id, cardId);
          return { planProgressCards: newMap };
        });
      },

      onLoopProgress: (payload: LoopProgressPayload) => {
        const { planProgressCards, messages } = get();
        const cardId = planProgressCards.get(payload.session_id);
        if (cardId) {
          const message = messages.find((m) => m.id === cardId);
          if (message && message.role === 'system' && message.card.type === 'planProgress') {
            const steps = message.card.steps.map((s, i) => ({
              ...s,
              status:
                i < payload.iteration
                  ? 'completed'
                  : i === payload.iteration
                    ? 'running'
                    : ('pending' as const),
            })) as PlanStep[];
            get().updateSystemCard(cardId, { steps, currentIndex: payload.iteration });
          }
        }
      },

      onSessionCompleted: (payload: SessionCompletedPayload) => {
        const { planProgressCards, removeSystemCard } = get();
        const cardId = planProgressCards.get(payload.session_id);
        if (cardId) {
          removeSystemCard(cardId);
          set((state) => {
            const newMap = new Map(state.planProgressCards);
            newMap.delete(payload.session_id);
            return { planProgressCards: newMap };
          });
        }
        set({ isProcessing: false });
        get().addSystemCard({
          type: 'success',
          message: payload.summary || 'Task completed',
        });
      },

      // Subagent events
      onSubagentStarted: (payload: SubagentStartedPayload) => {
        const cardId = get().addSystemCard({
          type: 'agentProgress',
          progress: {
            goal: `Running agent ${payload.agent_id}`,
            steps: [{ id: 'init', action: 'Initializing...', status: 'running' }],
            currentStep: 0,
          },
        });
        set((state) => {
          const newMap = new Map(state.subagentCards);
          newMap.set(payload.child_session_id, cardId);
          return { subagentCards: newMap };
        });
      },

      onSubagentCompleted: (payload: SubagentCompletedPayload) => {
        const { subagentCards, updateSystemCard, removeSystemCard, addSystemCard } = get();
        const cardId = subagentCards.get(payload.child_session_id);
        if (cardId) {
          if (payload.success) {
            updateSystemCard(cardId, {
              progress: {
                goal: 'Agent completed',
                steps: [{ id: 'done', action: payload.summary || 'Completed', status: 'completed' }],
                currentStep: 0,
              },
            });
            // Auto-remove after short delay
            setTimeout(() => removeSystemCard(cardId), 2000);
          } else {
            removeSystemCard(cardId);
            addSystemCard({
              type: 'error',
              message: payload.summary || 'Agent failed',
              canRetry: false,
            });
          }
          set((state) => {
            const newMap = new Map(state.subagentCards);
            newMap.delete(payload.child_session_id);
            return { subagentCards: newMap };
          });
        }
      },

      // Tool confirmation events
      onToolConfirmationRequired: (payload: ToolConfirmationPayload) => {
        const cardId = get().addSystemCard({
          type: 'toolConfirmation',
          tool: payload.tool_name,
          description: payload.description,
          args: payload.args,
        });
        set((state) => {
          const newMap = new Map(state.toolConfirmationCards);
          newMap.set(payload.confirmation_id, cardId);
          return { toolConfirmationCards: newMap };
        });
      },

      // Clarification events
      onClarificationRequired: (payload: ClarificationPayload) => {
        const cardId = get().addSystemCard({
          type: 'clarification',
          question: payload.question,
          options: payload.options,
        });
        set((state) => {
          const newMap = new Map(state.clarificationCards);
          newMap.set(payload.clarification_id, cardId);
          return { clarificationCards: newMap };
        });
      },
    };

    const unsubscribe = await subscribeToAetherEvents(handlers);
    set({ unsubscribe });
    console.log('[UnifiedHaloStore] Initialized with card support');
  },

  cleanup: () => {
    const { unsubscribe } = get();
    if (unsubscribe) {
      unsubscribe();
      set({ unsubscribe: null });
    }
  },
}));

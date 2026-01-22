import { create } from 'zustand';
import { commands } from '@/lib/commands';
import {
  subscribeToAetherEvents,
  type AetherEventHandlers,
  type StreamChunkPayload,
  type CompletePayload,
  type ErrorPayload,
} from '@/lib/events';
import type { ContentDisplayState } from '@/windows/halo/types';

// Message type for conversation
export interface HaloMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  timestamp: number;
  isStreaming?: boolean;
}

// Command type
export interface HaloCommand {
  key: string;
  description: string;
  icon?: string;
}

// Topic type
export interface HaloTopic {
  id: string;
  title: string;
  updatedAt: number;
}

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
      // Topic list mode
      set({ displayState: { type: 'topicList', prefix: '//' } });
      loadTopics();
      filterTopics(text.slice(2)); // Remove "//" prefix
    } else if (text.startsWith('/')) {
      // Command list mode
      set({ displayState: { type: 'commandList', prefix: '/' } });
      loadCommands();
      filterCommands(text.slice(1)); // Remove "/" prefix
    } else if (messages.length > 0) {
      // Has conversation history
      set({ displayState: { type: 'conversation' } });
    } else {
      // Empty state (only input box)
      set({ displayState: { type: 'empty' } });
    }
  },

  // Conversation actions
  sendMessage: async () => {
    const { inputText, currentTopicId, addUserMessage } = get();
    const trimmed = inputText.trim();
    if (!trimmed || trimmed.startsWith('/')) return;

    addUserMessage(trimmed);
    set({
      inputText: '',
      isProcessing: true,
      displayState: { type: 'conversation' }
    });

    try {
      await commands.processInput(trimmed, currentTopicId ?? undefined, true);
    } catch (error) {
      console.error('Failed to send message:', error);
      set({ isProcessing: false });
    }
  },

  addUserMessage: (content) => {
    const message: HaloMessage = {
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
    const message: HaloMessage = {
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
    const streamingMessage: HaloMessage = {
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
        m.isStreaming ? { ...m, content } : m
      ),
    }));
  },

  finishStreaming: () => {
    set((state) => ({
      isProcessing: false,
      streamingContent: '',
      messages: state.messages.map((m) =>
        m.isStreaming ? { ...m, isStreaming: false } : m
      ),
    }));
  },

  // Command actions
  loadCommands: () => {
    // TODO: Load from Rust backend via commands.listSkills() or similar
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
    // Execute the command
    switch (command.key) {
      case 'clear':
        set({ messages: [], displayState: { type: 'empty' }, inputText: '' });
        break;
      case 'settings':
        commands.openSettingsWindow();
        get().hide();
        break;
      default:
        // For other commands, put them in input and let user complete
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

  // Topic actions
  loadTopics: () => {
    // TODO: Load from backend
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
    const filtered = topics.filter((t) =>
      t.title.toLowerCase().includes(lowerQuery)
    );
    set({ filteredTopics: filtered, selectedTopicIndex: 0 });
  },

  selectTopic: (topic) => {
    // Switch to this topic and show conversation
    set({
      currentTopicId: topic.id,
      inputText: '',
      displayState: { type: 'conversation' },
      messages: [], // TODO: Load messages from backend
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

  // Window actions
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
    const { displayState, hide } = get();

    // If showing command or topic list, close it first
    if (displayState.type === 'commandList' || displayState.type === 'topicList') {
      const { messages } = get();
      set({
        inputText: '',
        displayState: messages.length > 0 ? { type: 'conversation' } : { type: 'empty' },
      });
    } else {
      // Otherwise, hide the window
      hide();
    }
  },

  // Initialization
  initialize: async () => {
    const store = get();
    if (store.unsubscribe) return;

    const handlers: AetherEventHandlers = {
      onThinking: () => set({ isProcessing: true }),
      onStreamChunk: (payload: StreamChunkPayload) => {
        const { messages, streamingContent } = get();
        const newContent = streamingContent + payload.text;

        // Start streaming if not already
        const hasStreamingMessage = messages.some((m) => m.isStreaming);
        if (!hasStreamingMessage) {
          get().startStreaming();
        }
        get().updateStreamingContent(newContent);
      },
      onComplete: (payload: CompletePayload) => {
        const { messages } = get();
        const hasStreamingMessage = messages.some((m) => m.isStreaming);

        if (hasStreamingMessage) {
          get().finishStreaming();
        } else {
          get().addAssistantMessage(payload.response);
        }
      },
      onError: (payload: ErrorPayload) => {
        console.error('AI Error:', payload.message);
        set({ isProcessing: false });
      },
    };

    const unsubscribe = await subscribeToAetherEvents(handlers);
    set({ unsubscribe });
    console.log('[UnifiedHaloStore] Initialized');
  },

  cleanup: () => {
    const { unsubscribe } = get();
    if (unsubscribe) {
      unsubscribe();
      set({ unsubscribe: null });
    }
  },
}));

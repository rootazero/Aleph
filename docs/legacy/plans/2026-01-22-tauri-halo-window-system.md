# Tauri Halo Window System Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a macOS-like Halo window system with dynamic content panels (conversation/commands/topics) that switch based on input prefix.

**Architecture:** Single HaloWindow with three mutually exclusive sub-panels controlled by ContentDisplayState. Input box detects `/` for commands, `//` for topics. Height adapts to content up to 400px max.

**Tech Stack:** React 18, Zustand, Tailwind CSS, Framer Motion, Tauri 2.0

---

## Overview

Window behavior reference (macOS version):
- Fixed width: 600px (Windows uses 600px, smaller than macOS due to lower DPI)
- Initial state: Only input box visible
- After sending message: Show conversation panel
- `/` prefix: Show command list panel
- `//` prefix: Show topic list panel
- Panels are mutually exclusive (only one visible at a time)
- Height adapts to content, max 400px
- ESC key: Close command/topic panel first, then close window

---

### Task 1: Update Tauri Window Configuration

**Files:**
- Modify: `platforms/tauri/src-tauri/tauri.conf.json`

**Step 1: Update halo window dimensions**

Change the halo window config from width 400 to 600:

```json
{
  "label": "halo",
  "title": "",
  "url": "/halo.html",
  "width": 600,
  "height": 80,
  "transparent": true,
  "decorations": false,
  "alwaysOnTop": true,
  "skipTaskbar": true,
  "visible": false,
  "resizable": false,
  "shadow": false,
  "focus": false
}
```

**Step 2: Verify change**

Run: `cd platforms/tauri && pnpm tauri dev`
Expected: Window appears at 600px width

**Step 3: Commit**

```bash
git add platforms/tauri/src-tauri/tauri.conf.json
git commit -m "feat(tauri): update halo window width to 600px"
```

---

### Task 2: Create ContentDisplayState Type

**Files:**
- Create: `platforms/tauri/src/windows/halo/types/ContentDisplayState.ts`

**Step 1: Create the type definition**

```typescript
// Content display states - mutually exclusive
export type ContentDisplayState =
  | { type: 'empty' }                           // Initial: only input box
  | { type: 'conversation' }                    // Show conversation history
  | { type: 'commandList'; prefix: string }     // "/" commands
  | { type: 'topicList'; prefix: string };      // "//" topics

// Helper functions
export function isShowingPanel(state: ContentDisplayState): boolean {
  return state.type !== 'empty';
}

export function isShowingCommandList(state: ContentDisplayState): boolean {
  return state.type === 'commandList';
}

export function isShowingTopicList(state: ContentDisplayState): boolean {
  return state.type === 'topicList';
}
```

**Step 2: Create index export**

Create `platforms/tauri/src/windows/halo/types/index.ts`:

```typescript
export * from './ContentDisplayState';
```

**Step 3: Commit**

```bash
git add platforms/tauri/src/windows/halo/types/
git commit -m "feat(tauri): add ContentDisplayState type for halo window"
```

---

### Task 3: Create Unified Halo Store

**Files:**
- Create: `platforms/tauri/src/stores/unifiedHaloStore.ts`

**Step 1: Create the unified store**

```typescript
import { create } from 'zustand';
import { commands } from '@/lib/commands';
import {
  subscribeToAlephEvents,
  type AlephEventHandlers,
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

    const handlers: AlephEventHandlers = {
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

    const unsubscribe = await subscribeToAlephEvents(handlers);
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
```

**Step 2: Commit**

```bash
git add platforms/tauri/src/stores/unifiedHaloStore.ts
git commit -m "feat(tauri): create unified halo store with display state management"
```

---

### Task 4: Create Input Area Component

**Files:**
- Create: `platforms/tauri/src/windows/halo/components/InputArea.tsx`

**Step 1: Create the input component**

```typescript
import { useRef, useEffect } from 'react';
import { Send, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';

export function InputArea() {
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const {
    inputText,
    isProcessing,
    displayState,
    handleInputChange,
    sendMessage,
    handleEscape,
    moveCommandSelection,
    moveTopicSelection,
    selectCommand,
    selectTopic,
    filteredCommands,
    filteredTopics,
    selectedCommandIndex,
    selectedTopicIndex,
    hide,
  } = useUnifiedHaloStore();

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    // Arrow keys for selection in command/topic mode
    if (displayState.type === 'commandList' || displayState.type === 'topicList') {
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (displayState.type === 'commandList') {
          moveCommandSelection('up');
        } else {
          moveTopicSelection('up');
        }
        return;
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (displayState.type === 'commandList') {
          moveCommandSelection('down');
        } else {
          moveTopicSelection('down');
        }
        return;
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        if (displayState.type === 'commandList' && filteredCommands[selectedCommandIndex]) {
          selectCommand(filteredCommands[selectedCommandIndex]);
        } else if (displayState.type === 'topicList' && filteredTopics[selectedTopicIndex]) {
          selectTopic(filteredTopics[selectedTopicIndex]);
        }
        return;
      }
      if (e.key === 'Tab') {
        e.preventDefault();
        if (displayState.type === 'commandList' && filteredCommands[selectedCommandIndex]) {
          selectCommand(filteredCommands[selectedCommandIndex]);
        } else if (displayState.type === 'topicList' && filteredTopics[selectedTopicIndex]) {
          selectTopic(filteredTopics[selectedTopicIndex]);
        }
        return;
      }
    }

    // Normal mode
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    } else if (e.key === 'Escape') {
      handleEscape();
    }
  };

  const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    handleInputChange(e.target.value);
  };

  return (
    <div className="flex flex-col gap-2 p-3">
      <div className="relative">
        <textarea
          ref={inputRef}
          value={inputText}
          onChange={handleChange}
          onKeyDown={handleKeyDown}
          placeholder="Type a message... (/ for commands, // for topics)"
          rows={2}
          disabled={isProcessing}
          className="w-full px-3 py-2 pr-10 rounded-lg border border-input bg-background/80 backdrop-blur-sm text-sm resize-none focus:outline-none focus:ring-2 focus:ring-ring disabled:opacity-50"
        />
        <button
          onClick={hide}
          className="absolute top-2 right-2 p-1 rounded hover:bg-secondary/80 transition-colors"
        >
          <X className="w-4 h-4 text-muted-foreground" />
        </button>
      </div>

      <div className="flex items-center justify-between">
        <span className="text-xs text-muted-foreground">
          {displayState.type === 'commandList' && 'Select command with ↑↓, Enter to execute'}
          {displayState.type === 'topicList' && 'Select topic with ↑↓, Enter to switch'}
          {displayState.type === 'empty' && 'Enter to send, Esc to close'}
          {displayState.type === 'conversation' && 'Enter to send, Shift+Enter for new line'}
        </span>
        {displayState.type !== 'commandList' && displayState.type !== 'topicList' && (
          <Button
            size="sm"
            onClick={sendMessage}
            disabled={!inputText.trim() || inputText.startsWith('/') || isProcessing}
          >
            <Send className="w-3.5 h-3.5 mr-1.5" />
            Send
          </Button>
        )}
      </div>
    </div>
  );
}
```

**Step 2: Commit**

```bash
git add platforms/tauri/src/windows/halo/components/InputArea.tsx
git commit -m "feat(tauri): create InputArea component with prefix detection"
```

---

### Task 5: Create Command List Component

**Files:**
- Create: `platforms/tauri/src/windows/halo/components/CommandList.tsx`

**Step 1: Create the command list component**

```typescript
import { motion } from 'framer-motion';
import { Command, Settings, Trash2, Brain, Wrench, HelpCircle } from 'lucide-react';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';
import type { HaloCommand } from '@/stores/unifiedHaloStore';

const ICON_MAP: Record<string, React.ReactNode> = {
  clear: <Trash2 className="w-4 h-4" />,
  settings: <Settings className="w-4 h-4" />,
  memory: <Brain className="w-4 h-4" />,
  tools: <Wrench className="w-4 h-4" />,
  help: <HelpCircle className="w-4 h-4" />,
};

interface CommandItemProps {
  command: HaloCommand;
  isSelected: boolean;
  onClick: () => void;
}

function CommandItem({ command, isSelected, onClick }: CommandItemProps) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center gap-3 px-3 py-2 rounded-md transition-colors text-left ${
        isSelected
          ? 'bg-primary/10 text-primary'
          : 'hover:bg-secondary/80 text-foreground'
      }`}
    >
      <span className="text-muted-foreground">
        {ICON_MAP[command.key] || <Command className="w-4 h-4" />}
      </span>
      <div className="flex-1 min-w-0">
        <div className="font-medium text-sm">/{command.key}</div>
        <div className="text-xs text-muted-foreground truncate">
          {command.description}
        </div>
      </div>
    </button>
  );
}

interface CommandListProps {
  maxHeight?: number;
}

export function CommandList({ maxHeight = 300 }: CommandListProps) {
  const { filteredCommands, selectedCommandIndex, selectCommand } =
    useUnifiedHaloStore();

  if (filteredCommands.length === 0) {
    return (
      <div className="px-3 py-6 text-center text-sm text-muted-foreground">
        No commands found
      </div>
    );
  }

  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: 'auto' }}
      exit={{ opacity: 0, height: 0 }}
      transition={{ duration: 0.15 }}
      className="overflow-hidden"
    >
      <div
        className="overflow-y-auto py-1 px-1"
        style={{ maxHeight }}
      >
        {filteredCommands.map((cmd, index) => (
          <CommandItem
            key={cmd.key}
            command={cmd}
            isSelected={index === selectedCommandIndex}
            onClick={() => selectCommand(cmd)}
          />
        ))}
      </div>
    </motion.div>
  );
}
```

**Step 2: Commit**

```bash
git add platforms/tauri/src/windows/halo/components/CommandList.tsx
git commit -m "feat(tauri): create CommandList component for slash commands"
```

---

### Task 6: Create Topic List Component

**Files:**
- Create: `platforms/tauri/src/windows/halo/components/TopicList.tsx`

**Step 1: Create the topic list component**

```typescript
import { motion } from 'framer-motion';
import { MessageSquare, Clock } from 'lucide-react';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';
import type { HaloTopic } from '@/stores/unifiedHaloStore';

function formatRelativeTime(timestamp: number): string {
  const now = Date.now();
  const diff = now - timestamp;
  const minutes = Math.floor(diff / 60000);
  const hours = Math.floor(diff / 3600000);
  const days = Math.floor(diff / 86400000);

  if (minutes < 1) return 'Just now';
  if (minutes < 60) return `${minutes}m ago`;
  if (hours < 24) return `${hours}h ago`;
  return `${days}d ago`;
}

interface TopicItemProps {
  topic: HaloTopic;
  isSelected: boolean;
  onClick: () => void;
}

function TopicItem({ topic, isSelected, onClick }: TopicItemProps) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center gap-3 px-3 py-2 rounded-md transition-colors text-left ${
        isSelected
          ? 'bg-primary/10 text-primary'
          : 'hover:bg-secondary/80 text-foreground'
      }`}
    >
      <MessageSquare className="w-4 h-4 text-muted-foreground flex-shrink-0" />
      <div className="flex-1 min-w-0">
        <div className="font-medium text-sm truncate">{topic.title}</div>
        <div className="flex items-center gap-1 text-xs text-muted-foreground">
          <Clock className="w-3 h-3" />
          {formatRelativeTime(topic.updatedAt)}
        </div>
      </div>
    </button>
  );
}

interface TopicListProps {
  maxHeight?: number;
}

export function TopicList({ maxHeight = 300 }: TopicListProps) {
  const { filteredTopics, selectedTopicIndex, selectTopic } =
    useUnifiedHaloStore();

  if (filteredTopics.length === 0) {
    return (
      <div className="px-3 py-6 text-center text-sm text-muted-foreground">
        No topics found
      </div>
    );
  }

  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: 'auto' }}
      exit={{ opacity: 0, height: 0 }}
      transition={{ duration: 0.15 }}
      className="overflow-hidden"
    >
      <div
        className="overflow-y-auto py-1 px-1"
        style={{ maxHeight }}
      >
        {filteredTopics.map((topic, index) => (
          <TopicItem
            key={topic.id}
            topic={topic}
            isSelected={index === selectedTopicIndex}
            onClick={() => selectTopic(topic)}
          />
        ))}
      </div>
    </motion.div>
  );
}
```

**Step 2: Commit**

```bash
git add platforms/tauri/src/windows/halo/components/TopicList.tsx
git commit -m "feat(tauri): create TopicList component for topic switching"
```

---

### Task 7: Create Conversation Area Component

**Files:**
- Create: `platforms/tauri/src/windows/halo/components/ConversationArea.tsx`

**Step 1: Create the conversation area component**

```typescript
import { useRef, useEffect } from 'react';
import { motion } from 'framer-motion';
import { User, Bot, Loader2 } from 'lucide-react';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';
import type { HaloMessage } from '@/stores/unifiedHaloStore';

interface MessageBubbleProps {
  message: HaloMessage;
}

function MessageBubble({ message }: MessageBubbleProps) {
  const isUser = message.role === 'user';

  return (
    <div
      className={`flex gap-2 ${isUser ? 'flex-row-reverse' : 'flex-row'}`}
    >
      <div
        className={`flex-shrink-0 w-6 h-6 rounded-full flex items-center justify-center ${
          isUser ? 'bg-primary' : 'bg-secondary'
        }`}
      >
        {isUser ? (
          <User className="w-3.5 h-3.5 text-primary-foreground" />
        ) : (
          <Bot className="w-3.5 h-3.5 text-secondary-foreground" />
        )}
      </div>
      <div
        className={`max-w-[80%] px-3 py-2 rounded-lg text-sm ${
          isUser
            ? 'bg-primary text-primary-foreground'
            : 'bg-secondary text-secondary-foreground'
        }`}
      >
        {message.content}
        {message.isStreaming && (
          <span className="inline-block w-1.5 h-4 ml-1 bg-current animate-pulse" />
        )}
      </div>
    </div>
  );
}

interface ConversationAreaProps {
  maxHeight?: number;
}

export function ConversationArea({ maxHeight = 300 }: ConversationAreaProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const { messages, isProcessing } = useUnifiedHaloStore();

  // Auto-scroll to bottom when new messages arrive
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages]);

  if (messages.length === 0) {
    return null;
  }

  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: 'auto' }}
      exit={{ opacity: 0, height: 0 }}
      transition={{ duration: 0.15 }}
      className="overflow-hidden"
    >
      <div
        ref={scrollRef}
        className="overflow-y-auto px-3 py-2 space-y-3"
        style={{ maxHeight }}
      >
        {messages.map((msg) => (
          <MessageBubble key={msg.id} message={msg} />
        ))}
        {isProcessing && !messages.some((m) => m.isStreaming) && (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="w-4 h-4 animate-spin" />
            <span>Thinking...</span>
          </div>
        )}
      </div>
    </motion.div>
  );
}
```

**Step 2: Commit**

```bash
git add platforms/tauri/src/windows/halo/components/ConversationArea.tsx
git commit -m "feat(tauri): create ConversationArea component for message display"
```

---

### Task 8: Create Unified Halo View

**Files:**
- Create: `platforms/tauri/src/windows/halo/UnifiedHaloView.tsx`

**Step 1: Create the unified view component**

```typescript
import { useEffect, useRef } from 'react';
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import { motion, AnimatePresence } from 'framer-motion';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';
import { InputArea } from './components/InputArea';
import { CommandList } from './components/CommandList';
import { TopicList } from './components/TopicList';
import { ConversationArea } from './components/ConversationArea';

// Layout constants
const LAYOUT = {
  WIDTH: 600,
  INPUT_HEIGHT: 80,
  MAX_CONTENT_HEIGHT: 300,
  PADDING: 16,
};

export function UnifiedHaloView() {
  const containerRef = useRef<HTMLDivElement>(null);
  const {
    displayState,
    visible,
    show,
    handleEscape,
    initialize,
    cleanup,
  } = useUnifiedHaloStore();

  // Initialize store
  useEffect(() => {
    initialize().catch(console.error);
    return () => cleanup();
  }, [initialize, cleanup]);

  // Listen for activation events
  useEffect(() => {
    const unlisten = listen('halo:activate', () => {
      show();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [show]);

  // Handle escape key
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        handleEscape();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleEscape]);

  // Auto-resize window based on content
  useEffect(() => {
    if (!containerRef.current) return;

    const resizeObserver = new ResizeObserver(async (entries) => {
      for (const entry of entries) {
        const { height } = entry.contentRect;
        if (height > 0) {
          try {
            const appWindow = getCurrentWindow();
            await appWindow.setSize(
              new LogicalSize(LAYOUT.WIDTH, Math.ceil(height) + LAYOUT.PADDING)
            );
          } catch (error) {
            console.error('Failed to resize window:', error);
          }
        }
      }
    });

    resizeObserver.observe(containerRef.current);
    return () => resizeObserver.disconnect();
  }, []);

  // Render the appropriate content panel
  const renderContentPanel = () => {
    switch (displayState.type) {
      case 'commandList':
        return <CommandList maxHeight={LAYOUT.MAX_CONTENT_HEIGHT} />;
      case 'topicList':
        return <TopicList maxHeight={LAYOUT.MAX_CONTENT_HEIGHT} />;
      case 'conversation':
        return <ConversationArea maxHeight={LAYOUT.MAX_CONTENT_HEIGHT} />;
      case 'empty':
      default:
        return null;
    }
  };

  return (
    <div className="h-screen w-screen flex items-start justify-center p-2">
      <motion.div
        ref={containerRef}
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        className="w-full max-w-[600px] bg-background/95 backdrop-blur-xl border border-border/50 rounded-xl shadow-2xl overflow-hidden"
      >
        {/* Content panel (conversation/commands/topics) */}
        <AnimatePresence mode="wait">
          {displayState.type !== 'empty' && (
            <motion.div
              key={displayState.type}
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              transition={{ duration: 0.15 }}
            >
              {renderContentPanel()}
            </motion.div>
          )}
        </AnimatePresence>

        {/* Divider */}
        {displayState.type !== 'empty' && (
          <div className="h-px bg-border/50" />
        )}

        {/* Input area (always visible) */}
        <InputArea />
      </motion.div>
    </div>
  );
}
```

**Step 2: Commit**

```bash
git add platforms/tauri/src/windows/halo/UnifiedHaloView.tsx
git commit -m "feat(tauri): create UnifiedHaloView with dynamic content panels"
```

---

### Task 9: Update HaloWindow Entry Point

**Files:**
- Modify: `platforms/tauri/src/windows/halo/HaloWindow.tsx`

**Step 1: Replace HaloWindow with UnifiedHaloView**

Replace the entire content of HaloWindow.tsx:

```typescript
import { UnifiedHaloView } from './UnifiedHaloView';

export function HaloWindow() {
  return <UnifiedHaloView />;
}
```

**Step 2: Commit**

```bash
git add platforms/tauri/src/windows/halo/HaloWindow.tsx
git commit -m "refactor(tauri): replace HaloWindow with UnifiedHaloView"
```

---

### Task 10: Add Component Exports

**Files:**
- Create: `platforms/tauri/src/windows/halo/components/index.ts`

**Step 1: Create index file for exports**

```typescript
// Panel components
export { CommandList } from './CommandList';
export { TopicList } from './TopicList';
export { ConversationArea } from './ConversationArea';
export { InputArea } from './InputArea';

// Re-export existing Halo components for backward compatibility
export { HaloListening } from './HaloListening';
export { HaloRetrievingMemory } from './HaloRetrievingMemory';
export { HaloProcessing } from './HaloProcessing';
export { HaloTypewriting } from './HaloTypewriting';
export { HaloSuccess } from './HaloSuccess';
export { HaloError } from './HaloError';
export { HaloToast } from './HaloToast';
export { HaloClarification } from './HaloClarification';
export { HaloConversationInput } from './HaloConversationInput';
export { HaloToolConfirmation } from './HaloToolConfirmation';
export { HaloPlanConfirmation } from './HaloPlanConfirmation';
export { HaloPlanProgress } from './HaloPlanProgress';
export { HaloTaskGraphConfirmation, HaloTaskGraphProgress } from './HaloTaskGraph';
export { HaloAgentPlan, HaloAgentProgress, HaloAgentConflict } from './HaloAgent';
```

**Step 2: Commit**

```bash
git add platforms/tauri/src/windows/halo/components/index.ts
git commit -m "feat(tauri): add component index exports"
```

---

### Task 11: Update Global Styles

**Files:**
- Modify: `platforms/tauri/src/styles/globals.css`

**Step 1: Add halo-specific styles**

Add the following styles at the end of globals.css:

```css
/* Halo Window Styles */
.halo-window {
  background: transparent;
}

.halo-container {
  background: hsl(var(--background) / 0.95);
  backdrop-filter: blur(20px);
  -webkit-backdrop-filter: blur(20px);
}

/* Scrollbar styles for halo panels */
.halo-container ::-webkit-scrollbar {
  width: 6px;
}

.halo-container ::-webkit-scrollbar-track {
  background: transparent;
}

.halo-container ::-webkit-scrollbar-thumb {
  background: hsl(var(--muted-foreground) / 0.3);
  border-radius: 3px;
}

.halo-container ::-webkit-scrollbar-thumb:hover {
  background: hsl(var(--muted-foreground) / 0.5);
}
```

**Step 2: Commit**

```bash
git add platforms/tauri/src/styles/globals.css
git commit -m "style(tauri): add halo window scrollbar and backdrop styles"
```

---

### Task 12: Integration Testing

**Files:**
- No new files

**Step 1: Start development server**

Run: `cd platforms/tauri && pnpm tauri dev`

**Step 2: Test initial state**

Expected: Window shows only input box (600px wide, ~80px tall)

**Step 3: Test `/` command mode**

Type `/` in the input
Expected: Command list appears below input, window height increases

**Step 4: Test `//` topic mode**

Type `//` in the input
Expected: Topic list appears below input, command list disappears (mutually exclusive)

**Step 5: Test conversation mode**

Type a message and press Enter
Expected: Message appears in conversation area, window height adapts

**Step 6: Test ESC behavior**

Press ESC while in command mode
Expected: Command list closes, returns to conversation or empty state

Press ESC again
Expected: Window hides

**Step 7: Final commit**

```bash
git add -A
git commit -m "feat(tauri): complete halo window system with dynamic panels

- Fixed width 600px, max content height 400px
- Mutually exclusive panels: conversation/commands/topics
- / prefix shows command list
- // prefix shows topic list
- Height adapts to content
- ESC closes panel first, then window"
```

---

## Summary

This plan implements a macOS-like Halo window system for Tauri with:

1. **ContentDisplayState** - Manages mutually exclusive panel states
2. **UnifiedHaloStore** - Centralized state management for all panel types
3. **InputArea** - Input detection for `/` and `//` prefixes
4. **CommandList** - Slash command panel with keyboard navigation
5. **TopicList** - Topic switching panel with search
6. **ConversationArea** - Message display with streaming support
7. **UnifiedHaloView** - Main container with dynamic height adaptation

Key behaviors:
- Fixed width: 600px
- Initial state: Only input box visible
- `/` shows commands, `//` shows topics (mutually exclusive)
- Sending message shows conversation
- Max content height: 300px (400px total with input)
- ESC closes panel first, then window

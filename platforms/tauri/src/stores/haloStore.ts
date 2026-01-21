import { create } from 'zustand';
import { commands } from '@/lib/commands';
import type { PlanStep } from '@/windows/halo/components/HaloPlanConfirmation';
import type { TaskGraph } from '@/windows/halo/components/HaloTaskGraph';
import type { AgentPlan, AgentProgress, ConflictInfo } from '@/windows/halo/components/HaloAgent';

// Halo state types (aligned with macOS version - 15+ states)
export type HaloStateType =
  | { type: 'idle' }
  | { type: 'listening' }
  | { type: 'retrievingMemory' }
  | { type: 'processingWithAI'; provider: string }
  | { type: 'processing'; content: string }
  | { type: 'typewriting'; content: string; progress: number }
  | { type: 'success'; message?: string }
  | { type: 'error'; message: string; canRetry: boolean }
  | { type: 'toast'; message: string; level: 'info' | 'warning' | 'error' }
  | { type: 'clarification'; question: string; options?: string[] }
  | { type: 'conversationInput'; placeholder?: string }
  | { type: 'toolConfirmation'; tool: string; args: Record<string, unknown> }
  | { type: 'planConfirmation'; steps: PlanStep[] }
  | { type: 'planProgress'; steps: PlanStep[]; currentIndex: number }
  | { type: 'taskGraphConfirmation'; graph: TaskGraph }
  | { type: 'taskGraphProgress'; graph: TaskGraph }
  | { type: 'agentPlan'; plan: AgentPlan }
  | { type: 'agentProgress'; progress: AgentProgress }
  | { type: 'agentConflict'; conflict: ConflictInfo };

interface HaloStore {
  // State
  state: HaloStateType;
  position: { x: number; y: number };
  visible: boolean;

  // Actions
  setState: (state: HaloStateType) => void;
  show: (position?: { x: number; y: number }) => void;
  hide: () => void;
  reset: () => void;

  // Business operations
  showSuccess: (message?: string) => void;
  showError: (message: string, canRetry?: boolean) => void;
  showToast: (message: string, level?: 'info' | 'warning' | 'error') => void;

  // Confirmation handlers
  confirmTool: (approved: boolean) => void;
  confirmPlan: (approved: boolean) => void;
  confirmTaskGraph: (approved: boolean) => void;
  confirmAgent: (approved: boolean) => void;
  resolveConflict: (optionId: string) => void;

  // Input handlers
  submitClarification: (response: string) => void;
  submitConversation: (input: string) => void;
}

export const useHaloStore = create<HaloStore>((set, get) => ({
  state: { type: 'idle' },
  position: { x: 0, y: 0 },
  visible: false,

  setState: (state) => set({ state }),

  show: async (position) => {
    if (position) {
      set({ visible: true, position, state: { type: 'listening' } });
    } else {
      try {
        const pos = await commands.getCursorPosition();
        set({ visible: true, position: pos, state: { type: 'listening' } });
      } catch (error) {
        console.error('Failed to get cursor position:', error);
        set({ visible: true, state: { type: 'listening' } });
      }
    }
  },

  hide: () => {
    set({ visible: false, state: { type: 'idle' } });
    commands.hideHaloWindow().catch(console.error);
  },

  reset: () => set({ state: { type: 'idle' }, visible: false }),

  showSuccess: (message) => {
    set({ state: { type: 'success', message } });
    setTimeout(() => {
      if (get().state.type === 'success') {
        get().hide();
      }
    }, 1500);
  },

  showError: (message, canRetry = true) => {
    set({ state: { type: 'error', message, canRetry } });
  },

  showToast: (message, level = 'info') => {
    set({ state: { type: 'toast', message, level } });
    setTimeout(() => {
      if (get().state.type === 'toast') {
        get().hide();
      }
    }, 3000);
  },

  // Confirmation handlers
  confirmTool: (approved) => {
    if (approved) {
      set({ state: { type: 'processingWithAI', provider: 'Executing tool...' } });
      // TODO: Call Rust backend to execute tool
    } else {
      get().hide();
    }
  },

  confirmPlan: (approved) => {
    if (approved) {
      const currentState = get().state;
      if (currentState.type === 'planConfirmation') {
        set({
          state: {
            type: 'planProgress',
            steps: currentState.steps.map((s) => ({ ...s, status: 'pending' as const })),
            currentIndex: 0,
          },
        });
        // TODO: Call Rust backend to start plan execution
      }
    } else {
      get().hide();
    }
  },

  confirmTaskGraph: (approved) => {
    if (approved) {
      const currentState = get().state;
      if (currentState.type === 'taskGraphConfirmation') {
        set({
          state: {
            type: 'taskGraphProgress',
            graph: currentState.graph,
          },
        });
        // TODO: Call Rust backend to start task graph execution
      }
    } else {
      get().hide();
    }
  },

  confirmAgent: (approved) => {
    if (approved) {
      const currentState = get().state;
      if (currentState.type === 'agentPlan') {
        set({
          state: {
            type: 'agentProgress',
            progress: {
              goal: currentState.plan.goal,
              steps: currentState.plan.steps,
              currentStep: 0,
            },
          },
        });
        // TODO: Call Rust backend to start agent
      }
    } else {
      get().hide();
    }
  },

  resolveConflict: (optionId) => {
    set({ state: { type: 'processingWithAI', provider: 'Resolving...' } });
    // TODO: Call Rust backend with selected option
    console.log('Selected conflict option:', optionId);
  },

  submitClarification: (response) => {
    set({ state: { type: 'processingWithAI', provider: 'Processing response...' } });
    // TODO: Call Rust backend with clarification response
    console.log('Clarification response:', response);
  },

  submitConversation: (input) => {
    set({ state: { type: 'processingWithAI', provider: 'Thinking...' } });
    // TODO: Call Rust backend with conversation input
    console.log('Conversation input:', input);
  },
}));

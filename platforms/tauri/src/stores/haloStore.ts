import { create } from 'zustand';
import { commands } from '@/lib/commands';

// Halo state types (aligned with macOS version)
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
  | { type: 'conversationInput'; placeholder?: string };

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
    // Auto-hide after delay
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
    // Auto-hide after delay
    setTimeout(() => {
      if (get().state.type === 'toast') {
        get().hide();
      }
    }, 3000);
  },
}));

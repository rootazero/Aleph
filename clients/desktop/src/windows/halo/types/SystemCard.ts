import type { PlanStep } from '../components/HaloPlanConfirmation';
import type { TaskGraph } from '../components/HaloTaskGraph';
import type { AgentPlan, AgentProgress, ConflictInfo } from '../components/HaloAgent';

/**
 * System card types - displayed as embedded cards in the conversation flow.
 * Each card type corresponds to a specific UI component.
 */
export type SystemCard =
  // Simple status cards
  | { type: 'processing'; provider?: string; content?: string }
  | { type: 'success'; message?: string }
  | { type: 'error'; message: string; canRetry: boolean }
  | { type: 'toast'; level: 'info' | 'warning' | 'error'; message: string }
  | { type: 'listening' }
  | { type: 'retrievingMemory' }
  | { type: 'typewriting'; content: string; progress: number }

  // Interactive cards
  | { type: 'clarification'; question: string; options?: string[] }
  | {
      type: 'toolConfirmation';
      tool: string;
      description?: string;
      args: Record<string, unknown>;
    }

  // Plan cards
  | { type: 'planConfirmation'; steps: PlanStep[] }
  | { type: 'planProgress'; steps: PlanStep[]; currentIndex: number }

  // Task graph cards
  | { type: 'taskGraphConfirmation'; graph: TaskGraph }
  | { type: 'taskGraphProgress'; graph: TaskGraph }

  // Agent cards
  | { type: 'agentPlan'; plan: AgentPlan }
  | { type: 'agentProgress'; progress: AgentProgress }
  | { type: 'agentConflict'; conflict: ConflictInfo };

/**
 * Get the accent color for a system card type.
 */
export function getCardAccentColor(card: SystemCard): string {
  switch (card.type) {
    case 'success':
      return 'hsl(var(--success))';
    case 'error':
      return 'hsl(var(--error))';
    case 'toast':
      switch (card.level) {
        case 'error':
          return 'hsl(var(--error))';
        case 'warning':
          return 'hsl(var(--warning))';
        default:
          return 'hsl(var(--info))';
      }
    case 'clarification':
      return 'hsl(var(--info))';
    case 'agentPlan':
    case 'agentProgress':
    case 'agentConflict':
      return 'hsl(var(--accent-blue))';
    default:
      // processing, toolConfirmation, plan*, taskGraph*, listening, retrievingMemory, typewriting
      return 'hsl(var(--accent-purple))';
  }
}

/**
 * Check if a system card is interactive (requires user action).
 */
export function isInteractiveCard(card: SystemCard): boolean {
  return (
    card.type === 'toolConfirmation' ||
    card.type === 'clarification' ||
    card.type === 'planConfirmation' ||
    card.type === 'taskGraphConfirmation' ||
    card.type === 'agentPlan' ||
    card.type === 'agentConflict' ||
    (card.type === 'error' && card.canRetry)
  );
}

/**
 * Check if a system card should auto-dismiss.
 */
export function shouldAutoDismiss(card: SystemCard): boolean {
  return card.type === 'success' || card.type === 'toast';
}

import { motion } from 'framer-motion';
import { Bot, Play, Circle, CheckCircle2, XCircle, AlertTriangle, ArrowRight } from 'lucide-react';
import { ArcSpinner } from '@/components/ui/arc-spinner';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

export interface AgentStep {
  id: string;
  action: string;
  status: 'pending' | 'running' | 'completed' | 'failed';
}

export interface AgentPlan {
  goal: string;
  steps: AgentStep[];
}

export interface AgentProgress {
  goal: string;
  steps: AgentStep[];
  currentStep: number;
  thought?: string;
}

export interface ConflictInfo {
  description: string;
  options: Array<{
    id: string;
    label: string;
    description?: string;
  }>;
}

// Agent Plan Confirmation
interface HaloAgentPlanProps {
  plan: AgentPlan;
  onConfirm: () => void;
  onCancel: () => void;
}

export function HaloAgentPlan({ plan, onConfirm, onCancel }: HaloAgentPlanProps) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      className="flex flex-col gap-3 p-3 min-w-[300px] max-w-[450px]"
    >
      {/* Header */}
      <div className="flex items-center gap-2">
        <div className="w-7 h-7 rounded-md bg-[hsl(var(--accent-blue))]/10 flex items-center justify-center">
          <Bot className="w-4 h-4 text-[hsl(var(--accent-blue))]" />
        </div>
        <div className="flex-1 min-w-0">
          <p className="text-sm font-medium text-foreground">Agent Plan</p>
          <p className="text-xs text-muted-foreground truncate">{plan.goal}</p>
        </div>
      </div>

      {/* Steps */}
      <div className="flex flex-col gap-0.5 max-h-[180px] overflow-y-auto">
        {plan.steps.map((step, index) => (
          <motion.div
            key={step.id}
            initial={{ opacity: 0, x: -10 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ delay: index * 0.03 }}
            className="flex items-center gap-2 py-1.5 px-2 rounded-sm hover:bg-secondary/50"
          >
            <span className="w-5 h-5 rounded-full bg-secondary flex items-center justify-center text-xs text-muted-foreground flex-shrink-0">
              {index + 1}
            </span>
            <p className="text-sm text-foreground truncate">{step.action}</p>
          </motion.div>
        ))}
      </div>

      {/* Actions */}
      <div className="flex items-center justify-end gap-2 pt-1">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="sm" onClick={onConfirm}>
          <Play className="w-3.5 h-3.5 mr-1.5" />
          Start Agent
        </Button>
      </div>
    </motion.div>
  );
}

// Agent Progress
interface HaloAgentProgressProps {
  progress: AgentProgress;
}

export function HaloAgentProgress({ progress }: HaloAgentProgressProps) {
  const completedCount = progress.steps.filter((s) => s.status === 'completed').length;
  const progressPercent = (completedCount / progress.steps.length) * 100;

  const getStatusIcon = (status: AgentStep['status']) => {
    switch (status) {
      case 'running':
        return <ArcSpinner size={14} color="hsl(var(--accent-blue))" />;
      case 'completed':
        return <CheckCircle2 className="w-3.5 h-3.5 text-[hsl(var(--success))]" />;
      case 'failed':
        return <XCircle className="w-3.5 h-3.5 text-[hsl(var(--error))]" />;
      default:
        return <Circle className="w-3.5 h-3.5 text-muted-foreground" />;
    }
  };

  return (
    <div className="flex flex-col gap-3 p-3 min-w-[300px] max-w-[450px]">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Bot className="w-4 h-4 text-[hsl(var(--accent-blue))]" />
          <span className="text-sm font-medium text-foreground">Agent Running</span>
        </div>
        <span className="text-xs text-muted-foreground">
          Step {progress.currentStep + 1}/{progress.steps.length}
        </span>
      </div>

      {/* Current thought */}
      {progress.thought && (
        <div className="bg-secondary/50 rounded-md px-2.5 py-1.5">
          <p className="text-xs text-muted-foreground italic">"{progress.thought}"</p>
        </div>
      )}

      {/* Progress bar */}
      <div className="h-1 bg-secondary rounded-full overflow-hidden">
        <motion.div
          className="h-full bg-[hsl(var(--accent-blue))]"
          initial={{ width: 0 }}
          animate={{ width: `${progressPercent}%` }}
          transition={{ duration: 0.3 }}
        />
      </div>

      {/* Steps */}
      <div className="flex flex-col gap-0.5 max-h-[130px] overflow-y-auto">
        {progress.steps.map((step, index) => {
          const isActive = index === progress.currentStep;

          return (
            <div
              key={step.id}
              className={cn(
                'flex items-center gap-2 py-1.5 px-2 rounded-sm',
                isActive && 'bg-accent'
              )}
            >
              <div className="flex-shrink-0">{getStatusIcon(step.status)}</div>
              <p
                className={cn(
                  'text-sm truncate',
                  step.status === 'completed' && 'text-muted-foreground'
                )}
              >
                {step.action}
              </p>
            </div>
          );
        })}
      </div>
    </div>
  );
}

// Agent Conflict Resolution
interface HaloAgentConflictProps {
  conflict: ConflictInfo;
  onSelect: (optionId: string) => void;
  onCancel: () => void;
}

export function HaloAgentConflict({ conflict, onSelect, onCancel }: HaloAgentConflictProps) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      className="flex flex-col gap-3 p-3 min-w-[300px] max-w-[420px]"
    >
      {/* Header */}
      <div className="flex items-center gap-2">
        <div className="w-7 h-7 rounded-md bg-[hsl(var(--warning))]/10 flex items-center justify-center">
          <AlertTriangle className="w-4 h-4 text-[hsl(var(--warning))]" />
        </div>
        <div>
          <p className="text-sm font-medium text-foreground">Decision Required</p>
          <p className="text-xs text-muted-foreground">{conflict.description}</p>
        </div>
      </div>

      {/* Options */}
      <div className="flex flex-col gap-1.5">
        {conflict.options.map((option, index) => (
          <motion.button
            key={option.id}
            initial={{ opacity: 0, x: -10 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ delay: index * 0.05 }}
            onClick={() => onSelect(option.id)}
            className="flex items-center gap-2 p-2.5 rounded-md border border-border hover:border-[hsl(var(--accent-blue))] hover:bg-accent text-left transition-colors group"
          >
            <div className="flex-1">
              <p className="text-sm font-medium text-foreground">{option.label}</p>
              {option.description && (
                <p className="text-xs text-muted-foreground">{option.description}</p>
              )}
            </div>
            <ArrowRight className="w-3.5 h-3.5 text-muted-foreground group-hover:text-[hsl(var(--accent-blue))] transition-colors" />
          </motion.button>
        ))}
      </div>

      {/* Cancel */}
      <div className="flex justify-end pt-1">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          Cancel Agent
        </Button>
      </div>
    </motion.div>
  );
}

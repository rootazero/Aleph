import { motion } from 'framer-motion';
import { Bot, Play, Circle, CheckCircle2, Loader2, XCircle, AlertTriangle, ArrowRight } from 'lucide-react';
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
      className="flex flex-col gap-3 p-4 min-w-[320px] max-w-[480px]"
    >
      {/* Header */}
      <div className="flex items-center gap-2">
        <div className="w-8 h-8 rounded-medium bg-primary/10 flex items-center justify-center">
          <Bot className="w-4 h-4 text-primary" />
        </div>
        <div className="flex-1 min-w-0">
          <p className="text-body font-medium text-foreground">Agent Plan</p>
          <p className="text-caption text-muted-foreground truncate">{plan.goal}</p>
        </div>
      </div>

      {/* Steps */}
      <div className="flex flex-col gap-1 max-h-[200px] overflow-y-auto">
        {plan.steps.map((step, index) => (
          <motion.div
            key={step.id}
            initial={{ opacity: 0, x: -10 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ delay: index * 0.03 }}
            className="flex items-center gap-2 py-1.5 px-2 rounded-small hover:bg-secondary/50"
          >
            <span className="w-5 h-5 rounded-full bg-secondary flex items-center justify-center text-caption text-muted-foreground flex-shrink-0">
              {index + 1}
            </span>
            <p className="text-body text-foreground truncate">{step.action}</p>
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

const statusIcons = {
  pending: Circle,
  running: Loader2,
  completed: CheckCircle2,
  failed: XCircle,
};

const statusStyles = {
  pending: 'text-muted-foreground',
  running: 'text-primary animate-spin',
  completed: 'text-success',
  failed: 'text-error',
};

export function HaloAgentProgress({ progress }: HaloAgentProgressProps) {
  const completedCount = progress.steps.filter((s) => s.status === 'completed').length;
  const progressPercent = (completedCount / progress.steps.length) * 100;

  return (
    <div className="flex flex-col gap-3 p-4 min-w-[320px] max-w-[480px]">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Bot className="w-5 h-5 text-primary" />
          <span className="text-body font-medium text-foreground">Agent Running</span>
        </div>
        <span className="text-caption text-muted-foreground">
          Step {progress.currentStep + 1}/{progress.steps.length}
        </span>
      </div>

      {/* Current thought */}
      {progress.thought && (
        <div className="bg-secondary/50 rounded-medium px-3 py-2">
          <p className="text-caption text-muted-foreground italic">
            "{progress.thought}"
          </p>
        </div>
      )}

      {/* Progress bar */}
      <div className="h-1.5 bg-secondary rounded-full overflow-hidden">
        <motion.div
          className="h-full bg-primary"
          initial={{ width: 0 }}
          animate={{ width: `${progressPercent}%` }}
          transition={{ duration: 0.3 }}
        />
      </div>

      {/* Steps */}
      <div className="flex flex-col gap-1 max-h-[150px] overflow-y-auto">
        {progress.steps.map((step, index) => {
          const Icon = statusIcons[step.status];
          const isActive = index === progress.currentStep;

          return (
            <div
              key={step.id}
              className={cn(
                'flex items-center gap-2 py-1.5 px-2 rounded-small',
                isActive && 'bg-accent'
              )}
            >
              <Icon
                className={cn('w-4 h-4 flex-shrink-0', statusStyles[step.status])}
              />
              <p
                className={cn(
                  'text-body truncate',
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
      className="flex flex-col gap-3 p-4 min-w-[320px] max-w-[450px]"
    >
      {/* Header */}
      <div className="flex items-center gap-2">
        <div className="w-8 h-8 rounded-medium bg-warning/10 flex items-center justify-center">
          <AlertTriangle className="w-4 h-4 text-warning" />
        </div>
        <div>
          <p className="text-body font-medium text-foreground">Decision Required</p>
          <p className="text-caption text-muted-foreground">{conflict.description}</p>
        </div>
      </div>

      {/* Options */}
      <div className="flex flex-col gap-2">
        {conflict.options.map((option, index) => (
          <motion.button
            key={option.id}
            initial={{ opacity: 0, x: -10 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ delay: index * 0.05 }}
            onClick={() => onSelect(option.id)}
            className="flex items-center gap-3 p-3 rounded-medium border border-border hover:border-primary hover:bg-accent text-left transition-colors group"
          >
            <div className="flex-1">
              <p className="text-body font-medium text-foreground">{option.label}</p>
              {option.description && (
                <p className="text-caption text-muted-foreground">
                  {option.description}
                </p>
              )}
            </div>
            <ArrowRight className="w-4 h-4 text-muted-foreground group-hover:text-primary transition-colors" />
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

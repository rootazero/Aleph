import { motion } from 'framer-motion';
import { ListChecks, Circle, CheckCircle2, XCircle } from 'lucide-react';
import { ArcSpinner } from '@/components/ui/arc-spinner';
import { cn } from '@/lib/utils';
import type { PlanStep } from './HaloPlanConfirmation';

interface HaloPlanProgressProps {
  steps: PlanStep[];
  currentIndex: number;
}

export function HaloPlanProgress({ steps, currentIndex }: HaloPlanProgressProps) {
  const completedCount = steps.filter((s) => s.status === 'completed').length;
  const progress = (completedCount / steps.length) * 100;

  const getStatusIcon = (status: PlanStep['status']) => {
    switch (status) {
      case 'running':
        return <ArcSpinner size={14} />;
      case 'completed':
        return <CheckCircle2 className="w-3.5 h-3.5 text-[hsl(var(--success))]" />;
      case 'failed':
        return <XCircle className="w-3.5 h-3.5 text-[hsl(var(--error))]" />;
      default:
        return <Circle className="w-3.5 h-3.5 text-muted-foreground" />;
    }
  };

  return (
    <div className="flex flex-col gap-3 p-3 min-w-[300px] max-w-[420px]">
      {/* Header with progress */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <ListChecks className="w-4 h-4 text-[hsl(var(--accent-purple))]" />
          <span className="text-sm font-medium text-foreground">Executing Plan</span>
        </div>
        <span className="text-xs text-muted-foreground">
          {completedCount}/{steps.length}
        </span>
      </div>

      {/* Progress bar */}
      <div className="h-1 bg-secondary rounded-full overflow-hidden">
        <motion.div
          className="h-full bg-[hsl(var(--accent-purple))]"
          initial={{ width: 0 }}
          animate={{ width: `${progress}%` }}
          transition={{ duration: 0.3 }}
        />
      </div>

      {/* Steps list */}
      <div className="flex flex-col gap-0.5 max-h-[160px] overflow-y-auto">
        {steps.map((step, index) => {
          const isActive = index === currentIndex;

          return (
            <motion.div
              key={step.id}
              initial={false}
              animate={{
                backgroundColor: isActive ? 'hsl(var(--accent))' : 'transparent',
              }}
              className={cn(
                'flex items-start gap-2 py-1.5 px-2 rounded-sm',
                isActive && 'bg-accent'
              )}
            >
              <div className="mt-0.5 flex-shrink-0">{getStatusIcon(step.status)}</div>
              <div className="flex-1 min-w-0">
                <p
                  className={cn(
                    'text-sm truncate',
                    step.status === 'completed' && 'text-muted-foreground line-through',
                    step.status === 'failed' && 'text-[hsl(var(--error))]'
                  )}
                >
                  {step.title}
                </p>
              </div>
            </motion.div>
          );
        })}
      </div>
    </div>
  );
}

import { motion } from 'framer-motion';
import { ListChecks, Circle, CheckCircle2, Loader2, XCircle } from 'lucide-react';
import { cn } from '@/lib/utils';
import type { PlanStep } from './HaloPlanConfirmation';

interface HaloPlanProgressProps {
  steps: PlanStep[];
  currentIndex: number;
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

export function HaloPlanProgress({ steps, currentIndex }: HaloPlanProgressProps) {
  const completedCount = steps.filter((s) => s.status === 'completed').length;
  const progress = (completedCount / steps.length) * 100;

  return (
    <div className="flex flex-col gap-3 p-4 min-w-[320px] max-w-[450px]">
      {/* Header with progress */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <ListChecks className="w-5 h-5 text-primary" />
          <span className="text-body font-medium text-foreground">
            Executing Plan
          </span>
        </div>
        <span className="text-caption text-muted-foreground">
          {completedCount}/{steps.length}
        </span>
      </div>

      {/* Progress bar */}
      <div className="h-1.5 bg-secondary rounded-full overflow-hidden">
        <motion.div
          className="h-full bg-primary"
          initial={{ width: 0 }}
          animate={{ width: `${progress}%` }}
          transition={{ duration: 0.3 }}
        />
      </div>

      {/* Steps list */}
      <div className="flex flex-col gap-1 max-h-[180px] overflow-y-auto">
        {steps.map((step, index) => {
          const Icon = statusIcons[step.status];
          const isActive = index === currentIndex;

          return (
            <motion.div
              key={step.id}
              initial={false}
              animate={{
                backgroundColor: isActive ? 'hsl(var(--accent))' : 'transparent',
              }}
              className={cn(
                'flex items-start gap-2 py-1.5 px-2 rounded-small',
                isActive && 'bg-accent'
              )}
            >
              <Icon
                className={cn('w-4 h-4 mt-0.5 flex-shrink-0', statusStyles[step.status])}
              />
              <div className="flex-1 min-w-0">
                <p
                  className={cn(
                    'text-body truncate',
                    step.status === 'completed' && 'text-muted-foreground line-through',
                    step.status === 'failed' && 'text-error'
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

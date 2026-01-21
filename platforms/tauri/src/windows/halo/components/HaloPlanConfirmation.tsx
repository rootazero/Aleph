import { motion } from 'framer-motion';
import { ListChecks, Play, X, Circle } from 'lucide-react';
import { Button } from '@/components/ui/button';

export interface PlanStep {
  id: string;
  title: string;
  description?: string;
  status: 'pending' | 'running' | 'completed' | 'failed';
}

interface HaloPlanConfirmationProps {
  steps: PlanStep[];
  onConfirm: () => void;
  onCancel: () => void;
}

export function HaloPlanConfirmation({
  steps,
  onConfirm,
  onCancel,
}: HaloPlanConfirmationProps) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      className="flex flex-col gap-3 p-4 min-w-[320px] max-w-[450px]"
    >
      {/* Header */}
      <div className="flex items-center gap-2">
        <div className="w-8 h-8 rounded-medium bg-primary/10 flex items-center justify-center">
          <ListChecks className="w-4 h-4 text-primary" />
        </div>
        <div>
          <p className="text-body font-medium text-foreground">Execute Plan?</p>
          <p className="text-caption text-muted-foreground">
            {steps.length} step{steps.length !== 1 ? 's' : ''}
          </p>
        </div>
      </div>

      {/* Steps list */}
      <div className="flex flex-col gap-1 max-h-[200px] overflow-y-auto">
        {steps.map((step, index) => (
          <motion.div
            key={step.id}
            initial={{ opacity: 0, x: -10 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ delay: index * 0.03 }}
            className="flex items-start gap-2 py-1.5 px-2 rounded-small hover:bg-secondary/50"
          >
            <Circle className="w-3 h-3 text-muted-foreground mt-1 flex-shrink-0" />
            <div className="flex-1 min-w-0">
              <p className="text-body text-foreground truncate">{step.title}</p>
              {step.description && (
                <p className="text-caption text-muted-foreground truncate">
                  {step.description}
                </p>
              )}
            </div>
          </motion.div>
        ))}
      </div>

      {/* Actions */}
      <div className="flex items-center justify-end gap-2 pt-1">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          <X className="w-3.5 h-3.5 mr-1.5" />
          Cancel
        </Button>
        <Button size="sm" onClick={onConfirm}>
          <Play className="w-3.5 h-3.5 mr-1.5" />
          Execute All
        </Button>
      </div>
    </motion.div>
  );
}

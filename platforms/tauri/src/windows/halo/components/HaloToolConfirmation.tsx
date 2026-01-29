import { motion } from 'framer-motion';
import { Wrench, Play, X } from 'lucide-react';
import { Button } from '@/components/ui/button';

interface HaloToolConfirmationProps {
  tool: string;
  args: Record<string, unknown>;
  onConfirm: () => void;
  onCancel: () => void;
}

export function HaloToolConfirmation({
  tool,
  args,
  onConfirm,
  onCancel,
}: HaloToolConfirmationProps) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      className="flex flex-col gap-3 p-3 min-w-[280px] max-w-[400px]"
    >
      {/* Header */}
      <div className="flex items-center gap-2">
        <div className="w-7 h-7 rounded-md bg-[hsl(var(--accent-purple))]/10 flex items-center justify-center">
          <Wrench className="w-4 h-4 text-[hsl(var(--accent-purple))]" />
        </div>
        <div>
          <p className="text-sm font-medium text-foreground">Run Tool?</p>
          <p className="text-xs text-muted-foreground font-mono">{tool}</p>
        </div>
      </div>

      {/* Arguments */}
      {Object.keys(args).length > 0 && (
        <div className="bg-secondary/50 rounded-md p-2.5 max-h-[120px] overflow-y-auto">
          <pre className="text-xs text-muted-foreground font-mono whitespace-pre-wrap break-all">
            {JSON.stringify(args, null, 2)}
          </pre>
        </div>
      )}

      {/* Actions */}
      <div className="flex items-center justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          <X className="w-3.5 h-3.5 mr-1.5" />
          Deny
        </Button>
        <Button size="sm" onClick={onConfirm}>
          <Play className="w-3.5 h-3.5 mr-1.5" />
          Approve
        </Button>
      </div>
    </motion.div>
  );
}

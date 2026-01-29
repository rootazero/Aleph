import { motion } from 'framer-motion';
import { Network, Play, X, Circle, CheckCircle2, XCircle } from 'lucide-react';
import { ArcSpinner } from '@/components/ui/arc-spinner';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

export interface TaskNode {
  id: string;
  title: string;
  status: 'pending' | 'running' | 'completed' | 'failed';
  dependencies: string[];
}

export interface TaskGraph {
  nodes: TaskNode[];
}

interface HaloTaskGraphConfirmationProps {
  graph: TaskGraph;
  onConfirm: () => void;
  onCancel: () => void;
}

export function HaloTaskGraphConfirmation({
  graph,
  onConfirm,
  onCancel,
}: HaloTaskGraphConfirmationProps) {
  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      className="flex flex-col gap-3 p-3 min-w-[300px] max-w-[420px]"
    >
      {/* Header */}
      <div className="flex items-center gap-2">
        <div className="w-7 h-7 rounded-md bg-[hsl(var(--accent-purple))]/10 flex items-center justify-center">
          <Network className="w-4 h-4 text-[hsl(var(--accent-purple))]" />
        </div>
        <div>
          <p className="text-sm font-medium text-foreground">Execute Task Graph?</p>
          <p className="text-xs text-muted-foreground">
            {graph.nodes.length} task{graph.nodes.length !== 1 ? 's' : ''} with dependencies
          </p>
        </div>
      </div>

      {/* Task list */}
      <div className="flex flex-col gap-0.5 max-h-[160px] overflow-y-auto">
        {graph.nodes.map((node, index) => (
          <motion.div
            key={node.id}
            initial={{ opacity: 0, x: -10 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ delay: index * 0.03 }}
            className="flex items-start gap-2 py-1.5 px-2 rounded-sm hover:bg-secondary/50"
          >
            <Circle className="w-3 h-3 text-muted-foreground mt-1 flex-shrink-0" />
            <div className="flex-1 min-w-0">
              <p className="text-sm text-foreground truncate">{node.title}</p>
              {node.dependencies.length > 0 && (
                <p className="text-xs text-muted-foreground">
                  Depends on: {node.dependencies.length} task(s)
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
          Execute
        </Button>
      </div>
    </motion.div>
  );
}

interface HaloTaskGraphProgressProps {
  graph: TaskGraph;
}

export function HaloTaskGraphProgress({ graph }: HaloTaskGraphProgressProps) {
  const completedCount = graph.nodes.filter((n) => n.status === 'completed').length;
  const runningCount = graph.nodes.filter((n) => n.status === 'running').length;
  const progress = (completedCount / graph.nodes.length) * 100;

  const getStatusIcon = (status: TaskNode['status']) => {
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
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Network className="w-4 h-4 text-[hsl(var(--accent-purple))]" />
          <span className="text-sm font-medium text-foreground">Task Graph</span>
        </div>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          {runningCount > 0 && (
            <span className="text-[hsl(var(--accent-purple))]">{runningCount} running</span>
          )}
          <span>
            {completedCount}/{graph.nodes.length}
          </span>
        </div>
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

      {/* Tasks */}
      <div className="flex flex-col gap-0.5 max-h-[140px] overflow-y-auto">
        {graph.nodes.map((node) => (
          <div
            key={node.id}
            className={cn(
              'flex items-start gap-2 py-1.5 px-2 rounded-sm',
              node.status === 'running' && 'bg-[hsl(var(--accent-purple))]/5'
            )}
          >
            <div className="mt-0.5 flex-shrink-0">{getStatusIcon(node.status)}</div>
            <p
              className={cn(
                'text-sm truncate flex-1',
                node.status === 'completed' && 'text-muted-foreground line-through',
                node.status === 'failed' && 'text-[hsl(var(--error))]'
              )}
            >
              {node.title}
            </p>
          </div>
        ))}
      </div>
    </div>
  );
}

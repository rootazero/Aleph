import { motion } from 'framer-motion';
import { Network, Play, X, Circle, CheckCircle2, Loader2, XCircle } from 'lucide-react';
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
      className="flex flex-col gap-3 p-4 min-w-[320px] max-w-[450px]"
    >
      {/* Header */}
      <div className="flex items-center gap-2">
        <div className="w-8 h-8 rounded-medium bg-purple-500/10 flex items-center justify-center">
          <Network className="w-4 h-4 text-purple-500" />
        </div>
        <div>
          <p className="text-body font-medium text-foreground">Execute Task Graph?</p>
          <p className="text-caption text-muted-foreground">
            {graph.nodes.length} task{graph.nodes.length !== 1 ? 's' : ''} with dependencies
          </p>
        </div>
      </div>

      {/* Task list */}
      <div className="flex flex-col gap-1 max-h-[180px] overflow-y-auto">
        {graph.nodes.map((node, index) => (
          <motion.div
            key={node.id}
            initial={{ opacity: 0, x: -10 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ delay: index * 0.03 }}
            className="flex items-start gap-2 py-1.5 px-2 rounded-small hover:bg-secondary/50"
          >
            <Circle className="w-3 h-3 text-muted-foreground mt-1 flex-shrink-0" />
            <div className="flex-1 min-w-0">
              <p className="text-body text-foreground truncate">{node.title}</p>
              {node.dependencies.length > 0 && (
                <p className="text-caption text-muted-foreground">
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

const statusIcons = {
  pending: Circle,
  running: Loader2,
  completed: CheckCircle2,
  failed: XCircle,
};

const statusStyles = {
  pending: 'text-muted-foreground',
  running: 'text-purple-500 animate-spin',
  completed: 'text-success',
  failed: 'text-error',
};

export function HaloTaskGraphProgress({ graph }: HaloTaskGraphProgressProps) {
  const completedCount = graph.nodes.filter((n) => n.status === 'completed').length;
  const runningCount = graph.nodes.filter((n) => n.status === 'running').length;
  const progress = (completedCount / graph.nodes.length) * 100;

  return (
    <div className="flex flex-col gap-3 p-4 min-w-[320px] max-w-[450px]">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Network className="w-5 h-5 text-purple-500" />
          <span className="text-body font-medium text-foreground">
            Task Graph
          </span>
        </div>
        <div className="flex items-center gap-2 text-caption text-muted-foreground">
          {runningCount > 0 && (
            <span className="text-purple-500">{runningCount} running</span>
          )}
          <span>{completedCount}/{graph.nodes.length}</span>
        </div>
      </div>

      {/* Progress bar */}
      <div className="h-1.5 bg-secondary rounded-full overflow-hidden">
        <motion.div
          className="h-full bg-purple-500"
          initial={{ width: 0 }}
          animate={{ width: `${progress}%` }}
          transition={{ duration: 0.3 }}
        />
      </div>

      {/* Tasks */}
      <div className="flex flex-col gap-1 max-h-[160px] overflow-y-auto">
        {graph.nodes.map((node) => {
          const Icon = statusIcons[node.status];

          return (
            <div
              key={node.id}
              className={cn(
                'flex items-start gap-2 py-1.5 px-2 rounded-small',
                node.status === 'running' && 'bg-purple-500/5'
              )}
            >
              <Icon
                className={cn('w-4 h-4 mt-0.5 flex-shrink-0', statusStyles[node.status])}
              />
              <p
                className={cn(
                  'text-body truncate flex-1',
                  node.status === 'completed' && 'text-muted-foreground line-through',
                  node.status === 'failed' && 'text-error'
                )}
              >
                {node.title}
              </p>
            </div>
          );
        })}
      </div>
    </div>
  );
}

import { motion } from 'framer-motion';
import { Command, Settings, Trash2, Brain, Wrench, HelpCircle } from 'lucide-react';
import { useUnifiedHaloStore } from '@/stores/unifiedHaloStore';
import type { HaloCommand } from '@/stores/unifiedHaloStore';

const ICON_MAP: Record<string, React.ReactNode> = {
  clear: <Trash2 className="w-4 h-4" />,
  settings: <Settings className="w-4 h-4" />,
  memory: <Brain className="w-4 h-4" />,
  tools: <Wrench className="w-4 h-4" />,
  help: <HelpCircle className="w-4 h-4" />,
};

interface CommandItemProps {
  command: HaloCommand;
  isSelected: boolean;
  onClick: () => void;
}

function CommandItem({ command, isSelected, onClick }: CommandItemProps) {
  return (
    <button
      onClick={onClick}
      className={`w-full flex items-center gap-3 px-3 py-2 rounded-md transition-colors text-left ${
        isSelected
          ? 'bg-primary/10 text-primary'
          : 'hover:bg-secondary/80 text-foreground'
      }`}
    >
      <span className="text-muted-foreground">
        {ICON_MAP[command.key] || <Command className="w-4 h-4" />}
      </span>
      <div className="flex-1 min-w-0">
        <div className="font-medium text-sm">/{command.key}</div>
        <div className="text-xs text-muted-foreground truncate">
          {command.description}
        </div>
      </div>
    </button>
  );
}

interface CommandListProps {
  maxHeight?: number;
}

export function CommandList({ maxHeight = 300 }: CommandListProps) {
  const { filteredCommands, selectedCommandIndex, selectCommand } =
    useUnifiedHaloStore();

  if (filteredCommands.length === 0) {
    return (
      <div className="px-3 py-6 text-center text-sm text-muted-foreground">
        No commands found
      </div>
    );
  }

  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: 'auto' }}
      exit={{ opacity: 0, height: 0 }}
      transition={{ duration: 0.15 }}
      className="overflow-hidden"
    >
      <div
        className="overflow-y-auto py-1 px-1"
        style={{ maxHeight }}
      >
        {filteredCommands.map((cmd, index) => (
          <CommandItem
            key={cmd.key}
            command={cmd}
            isSelected={index === selectedCommandIndex}
            onClick={() => selectCommand(cmd)}
          />
        ))}
      </div>
    </motion.div>
  );
}

import { motion } from 'framer-motion';
import { Brain } from 'lucide-react';

export function HaloRetrievingMemory() {
  return (
    <div className="flex items-center gap-3">
      <motion.div
        animate={{ opacity: [0.5, 1, 0.5] }}
        transition={{ duration: 1.5, repeat: Infinity, ease: 'easeInOut' }}
      >
        <Brain className="w-4 h-4 text-[hsl(var(--accent-purple))]" />
      </motion.div>
      <span className="text-sm text-foreground">Retrieving memory...</span>
    </div>
  );
}

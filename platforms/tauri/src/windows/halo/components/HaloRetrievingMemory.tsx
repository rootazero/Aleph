import { motion } from 'framer-motion';
import { Brain } from 'lucide-react';

export function HaloRetrievingMemory() {
  return (
    <div className="flex items-center gap-3 px-4 py-3">
      <motion.div
        animate={{ opacity: [0.5, 1, 0.5] }}
        transition={{ duration: 1.5, repeat: Infinity, ease: 'easeInOut' }}
      >
        <Brain className="w-5 h-5 text-purple-500" />
      </motion.div>
      <span className="text-body text-foreground">Retrieving memory...</span>
    </div>
  );
}

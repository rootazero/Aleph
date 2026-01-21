import { motion } from 'framer-motion';
import { Loader2 } from 'lucide-react';

interface HaloProcessingProps {
  provider?: string;
  content?: string;
}

export function HaloProcessing({ provider, content }: HaloProcessingProps) {
  return (
    <div className="flex items-center gap-3 px-4 py-3 min-w-[200px]">
      <motion.div
        animate={{ rotate: 360 }}
        transition={{ duration: 1, repeat: Infinity, ease: 'linear' }}
      >
        <Loader2 className="w-4 h-4 text-purple-500" />
      </motion.div>
      <span className="text-body text-foreground">
        {content || provider || 'Processing...'}
      </span>
    </div>
  );
}

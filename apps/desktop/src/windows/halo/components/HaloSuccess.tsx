import { motion } from 'framer-motion';
import { Check } from 'lucide-react';

interface HaloSuccessProps {
  message?: string;
}

export function HaloSuccess({ message }: HaloSuccessProps) {
  return (
    <div className="flex items-center gap-3">
      <motion.div
        initial={{ scale: 0 }}
        animate={{ scale: 1 }}
        transition={{ type: 'spring', stiffness: 500, damping: 30 }}
        className="w-5 h-5 rounded-full bg-[hsl(var(--success))] flex items-center justify-center"
      >
        <Check className="w-3 h-3 text-white" />
      </motion.div>
      {message && <span className="text-sm text-foreground">{message}</span>}
    </div>
  );
}

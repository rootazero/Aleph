import { motion } from 'framer-motion';
import { Check } from 'lucide-react';

interface HaloSuccessProps {
  message?: string;
}

export function HaloSuccess({ message }: HaloSuccessProps) {
  return (
    <div className="flex items-center gap-3 px-4 py-3">
      <motion.div
        initial={{ scale: 0 }}
        animate={{ scale: 1 }}
        transition={{ type: 'spring', stiffness: 500, damping: 30 }}
        className="w-6 h-6 rounded-full bg-success flex items-center justify-center"
      >
        <Check className="w-4 h-4 text-white" />
      </motion.div>
      {message && (
        <span className="text-body text-foreground">{message}</span>
      )}
    </div>
  );
}

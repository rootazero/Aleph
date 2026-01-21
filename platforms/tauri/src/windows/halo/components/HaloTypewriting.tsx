import { motion } from 'framer-motion';
import { Keyboard } from 'lucide-react';

interface HaloTypewritingProps {
  content: string;
  progress: number; // 0-100
}

export function HaloTypewriting({ content, progress }: HaloTypewritingProps) {
  return (
    <div className="flex flex-col gap-2 px-4 py-3 min-w-[200px] max-w-[320px]">
      <div className="flex items-center gap-2">
        <motion.div
          animate={{ y: [0, -2, 0] }}
          transition={{ duration: 0.3, repeat: Infinity }}
        >
          <Keyboard className="w-4 h-4 text-purple-500" />
        </motion.div>
        <span className="text-caption text-muted-foreground">Typing...</span>
      </div>

      {content && (
        <p className="text-body text-foreground line-clamp-2">{content}</p>
      )}

      {/* Progress bar */}
      <div className="h-1 bg-secondary rounded-full overflow-hidden">
        <motion.div
          className="h-full bg-purple-500"
          initial={{ width: 0 }}
          animate={{ width: `${progress}%` }}
          transition={{ duration: 0.1 }}
        />
      </div>
    </div>
  );
}

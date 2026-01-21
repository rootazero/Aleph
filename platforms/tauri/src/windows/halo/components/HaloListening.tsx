import { motion } from 'framer-motion';

export function HaloListening() {
  return (
    <div className="flex items-center justify-center p-6" data-testid="halo-listening">
      <motion.div
        data-testid="listening-circle"
        className="w-4 h-4 rounded-full bg-purple-500"
        animate={{
          scale: [1, 1.5, 1],
          opacity: [1, 0.5, 1],
        }}
        transition={{
          duration: 0.8,
          repeat: Infinity,
          ease: 'easeInOut',
        }}
      />
    </div>
  );
}

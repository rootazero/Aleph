import { motion } from 'framer-motion';
import { Button } from './button';

interface SaveBarProps {
  onSave: () => void;
  onDiscard: () => void;
  isSaving?: boolean;
}

export function SaveBar({ onSave, onDiscard, isSaving }: SaveBarProps) {
  return (
    <motion.div
      initial={{ y: 50, opacity: 0 }}
      animate={{ y: 0, opacity: 1 }}
      exit={{ y: 50, opacity: 0 }}
      transition={{ duration: 0.2 }}
      className="border-t bg-secondary/50 p-3 flex items-center justify-end gap-2"
    >
      <Button variant="ghost" onClick={onDiscard} disabled={isSaving}>
        Discard
      </Button>
      <Button onClick={onSave} disabled={isSaving}>
        {isSaving ? 'Saving...' : 'Save'}
      </Button>
    </motion.div>
  );
}

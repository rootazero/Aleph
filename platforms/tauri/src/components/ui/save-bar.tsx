import { motion } from 'framer-motion';
import { Circle, AlertCircle, Loader2 } from 'lucide-react';
import { Button } from './button';
import { useTranslation } from 'react-i18next';

interface SaveBarProps {
  onSave: () => void;
  onDiscard: () => void;
  isSaving?: boolean;
  statusMessage?: string | null;
}

export function SaveBar({
  onSave,
  onDiscard,
  isSaving,
  statusMessage,
}: SaveBarProps) {
  const { t } = useTranslation();

  return (
    <motion.div
      initial={{ y: 50, opacity: 0 }}
      animate={{ y: 0, opacity: 1 }}
      exit={{ y: 50, opacity: 0 }}
      transition={{ duration: 0.2 }}
      className="border-t bg-secondary/50 backdrop-blur-sm px-md py-sm flex items-center justify-between gap-md"
    >
      {/* Left: Status indicator */}
      <div className="flex items-center gap-sm text-caption">
        {isSaving ? (
          <>
            <Loader2 className="w-3.5 h-3.5 animate-spin text-muted-foreground" />
            <span className="text-muted-foreground">
              {t('common.saving', 'Saving...')}
            </span>
          </>
        ) : statusMessage ? (
          <>
            <AlertCircle className="w-3.5 h-3.5 text-error" />
            <span className="text-error">{statusMessage}</span>
          </>
        ) : (
          <>
            <Circle className="w-3.5 h-3.5 fill-warning text-warning" />
            <span className="text-muted-foreground">
              {t('common.unsavedChanges', 'Unsaved changes')}
            </span>
          </>
        )}
      </div>

      {/* Right: Action buttons */}
      <div className="flex items-center gap-sm">
        <Button
          variant="ghost"
          size="sm"
          onClick={onDiscard}
          disabled={isSaving}
        >
          {t('common.cancel', 'Cancel')}
        </Button>
        <Button size="sm" onClick={onSave} disabled={isSaving}>
          {t('common.save', 'Save')}
        </Button>
      </div>
    </motion.div>
  );
}

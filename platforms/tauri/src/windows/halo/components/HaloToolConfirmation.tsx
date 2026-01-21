import { motion } from 'framer-motion';
import { Wrench, Play, X } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useTranslation } from 'react-i18next';

interface HaloToolConfirmationProps {
  tool: string;
  args: Record<string, unknown>;
  onConfirm: () => void;
  onCancel: () => void;
}

export function HaloToolConfirmation({
  tool,
  args,
  onConfirm,
  onCancel,
}: HaloToolConfirmationProps) {
  const { t } = useTranslation();

  return (
    <motion.div
      initial={{ opacity: 0, scale: 0.95 }}
      animate={{ opacity: 1, scale: 1 }}
      className="flex flex-col gap-3 p-4 min-w-[300px] max-w-[420px]"
    >
      {/* Header */}
      <div className="flex items-center gap-2">
        <div className="w-8 h-8 rounded-medium bg-warning/10 flex items-center justify-center">
          <Wrench className="w-4 h-4 text-warning" />
        </div>
        <div>
          <p className="text-body font-medium text-foreground">{t('halo.tool.title')}</p>
          <p className="text-caption text-muted-foreground">{tool}</p>
        </div>
      </div>

      {/* Arguments */}
      {Object.keys(args).length > 0 && (
        <div className="bg-secondary/50 rounded-medium p-3 max-h-[150px] overflow-y-auto">
          <pre className="text-code text-muted-foreground whitespace-pre-wrap break-all">
            {JSON.stringify(args, null, 2)}
          </pre>
        </div>
      )}

      {/* Actions */}
      <div className="flex items-center justify-end gap-2">
        <Button variant="ghost" size="sm" onClick={onCancel}>
          <X className="w-3.5 h-3.5 mr-1.5" />
          {t('halo.tool.deny')}
        </Button>
        <Button size="sm" onClick={onConfirm}>
          <Play className="w-3.5 h-3.5 mr-1.5" />
          {t('halo.tool.approve')}
        </Button>
      </div>
    </motion.div>
  );
}

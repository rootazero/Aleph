import { AlertCircle, RefreshCw, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

interface HaloErrorProps {
  message: string;
  canRetry: boolean;
  onRetry: () => void;
  onClose: () => void;
}

export function HaloError({ message, canRetry, onRetry, onClose }: HaloErrorProps) {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-3 p-4 max-w-[280px]">
      <div className="flex items-start gap-3">
        <AlertCircle className="w-5 h-5 text-error flex-shrink-0 mt-0.5" />
        <p className="text-body text-foreground">{message}</p>
      </div>
      <div className="flex items-center justify-end gap-2">
        {canRetry && (
          <button
            onClick={onRetry}
            className="flex items-center gap-1.5 px-3 py-1.5 text-caption rounded-medium bg-secondary hover:bg-secondary/80 transition-colors"
          >
            <RefreshCw className="w-3.5 h-3.5" />
            {t('common.retry')}
          </button>
        )}
        <button
          onClick={onClose}
          className="flex items-center gap-1.5 px-3 py-1.5 text-caption rounded-medium bg-secondary hover:bg-secondary/80 transition-colors"
        >
          <X className="w-3.5 h-3.5" />
          {t('common.close')}
        </button>
      </div>
    </div>
  );
}

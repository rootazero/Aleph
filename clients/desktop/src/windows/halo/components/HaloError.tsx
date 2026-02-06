import { AlertCircle, RefreshCw, X } from 'lucide-react';

interface HaloErrorProps {
  message: string;
  canRetry: boolean;
  onRetry: () => void;
  onClose: () => void;
}

export function HaloError({ message, canRetry, onRetry, onClose }: HaloErrorProps) {
  return (
    <div className="flex flex-col gap-2 p-3 max-w-[280px]">
      <div className="flex items-start gap-2">
        <AlertCircle className="w-4 h-4 text-[hsl(var(--error))] flex-shrink-0 mt-0.5" />
        <p className="text-sm text-foreground">{message}</p>
      </div>
      <div className="flex items-center justify-end gap-2">
        {canRetry && (
          <button
            onClick={onRetry}
            className="flex items-center gap-1 px-2 py-1 text-xs rounded-md bg-secondary hover:bg-secondary/80 transition-colors"
          >
            <RefreshCw className="w-3 h-3" />
            Retry
          </button>
        )}
        <button
          onClick={onClose}
          className="flex items-center gap-1 px-2 py-1 text-xs rounded-md bg-secondary hover:bg-secondary/80 transition-colors"
        >
          <X className="w-3 h-3" />
          Close
        </button>
      </div>
    </div>
  );
}

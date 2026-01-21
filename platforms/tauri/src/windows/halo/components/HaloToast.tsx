import { Info, AlertTriangle, XCircle } from 'lucide-react';
import { cn } from '@/lib/utils';

interface HaloToastProps {
  message: string;
  level: 'info' | 'warning' | 'error';
}

const icons = {
  info: Info,
  warning: AlertTriangle,
  error: XCircle,
};

const styles = {
  info: 'text-info',
  warning: 'text-warning',
  error: 'text-error',
};

export function HaloToast({ message, level }: HaloToastProps) {
  const Icon = icons[level];

  return (
    <div className="flex items-center gap-3 px-4 py-3 min-w-[200px] max-w-[320px]">
      <Icon className={cn('w-5 h-5 flex-shrink-0', styles[level])} />
      <p className="text-body text-foreground">{message}</p>
    </div>
  );
}

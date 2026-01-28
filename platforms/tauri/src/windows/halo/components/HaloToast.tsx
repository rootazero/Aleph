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
  info: 'text-[hsl(var(--info))]',
  warning: 'text-[hsl(var(--warning))]',
  error: 'text-[hsl(var(--error))]',
};

export function HaloToast({ message, level }: HaloToastProps) {
  const Icon = icons[level];

  return (
    <div className="flex items-center gap-2 min-w-[200px] max-w-[320px]">
      <Icon className={cn('w-4 h-4 flex-shrink-0', styles[level])} />
      <p className="text-sm text-foreground">{message}</p>
    </div>
  );
}

import { cn } from '@/lib/utils';
import { Info, AlertTriangle, CheckCircle, XCircle } from 'lucide-react';

type InfoBoxVariant = 'info' | 'warning' | 'success' | 'error';

interface InfoBoxProps {
  variant?: InfoBoxVariant;
  children: React.ReactNode;
  className?: string;
}

const variantConfig: Record<
  InfoBoxVariant,
  { icon: typeof Info; className: string }
> = {
  info: {
    icon: Info,
    className: 'info-box-info',
  },
  warning: {
    icon: AlertTriangle,
    className: 'info-box-warning',
  },
  success: {
    icon: CheckCircle,
    className: 'info-box-success',
  },
  error: {
    icon: XCircle,
    className: 'info-box-error',
  },
};

export function InfoBox({
  variant = 'info',
  children,
  className,
}: InfoBoxProps) {
  const config = variantConfig[variant];
  const Icon = config.icon;

  return (
    <div className={cn(config.className, className)}>
      <Icon className="w-4 h-4 shrink-0 mt-0.5" />
      <div className="text-caption">{children}</div>
    </div>
  );
}

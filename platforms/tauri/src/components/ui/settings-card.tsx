import { cn } from '@/lib/utils';
import { type LucideIcon } from 'lucide-react';

type SettingsCardVariant = 'inline' | 'section' | 'stacked';

interface SettingsCardProps {
  title: string;
  description?: string;
  icon?: LucideIcon;
  variant?: SettingsCardVariant;
  children: React.ReactNode;
  className?: string;
}

export function SettingsCard({
  title,
  description,
  icon: Icon,
  variant = 'inline',
  children,
  className,
}: SettingsCardProps) {
  // Section variant: full-width card with header and stacked content
  if (variant === 'section') {
    return (
      <div
        className={cn(
          'p-md rounded-md card-glass border border-border',
          className
        )}
      >
        <div className="flex items-center gap-sm mb-md">
          {Icon && (
            <Icon className="w-5 h-5 text-muted-foreground" />
          )}
          <div>
            <h3 className="text-heading text-foreground">{title}</h3>
            {description && (
              <p className="text-caption text-muted-foreground">{description}</p>
            )}
          </div>
        </div>
        <div className="space-y-md">{children}</div>
      </div>
    );
  }

  // Stacked variant: title on top, control below
  if (variant === 'stacked') {
    return (
      <div
        className={cn(
          'p-md rounded-md card-glass border border-border space-y-sm',
          className
        )}
      >
        <div className="flex items-center gap-sm">
          {Icon && (
            <Icon className="w-4 h-4 text-muted-foreground" />
          )}
          <div>
            <label className="text-body font-medium text-foreground">{title}</label>
            {description && (
              <p className="text-caption text-muted-foreground">{description}</p>
            )}
          </div>
        </div>
        <div>{children}</div>
      </div>
    );
  }

  // Inline variant (default): title left, control right
  return (
    <div
      className={cn(
        'flex items-center justify-between p-md rounded-md card-glass border border-border',
        className
      )}
    >
      <div className="flex items-center gap-sm">
        {Icon && (
          <Icon className="w-4 h-4 text-muted-foreground" />
        )}
        <div className="space-y-0.5">
          <label className="text-body font-medium text-foreground">{title}</label>
          {description && (
            <p className="text-caption text-muted-foreground">{description}</p>
          )}
        </div>
      </div>
      <div>{children}</div>
    </div>
  );
}

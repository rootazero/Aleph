import { cn } from '@/lib/utils';

interface SettingsCardProps {
  title: string;
  description?: string;
  children: React.ReactNode;
  className?: string;
}

export function SettingsCard({
  title,
  description,
  children,
  className,
}: SettingsCardProps) {
  return (
    <div
      className={cn(
        'flex items-center justify-between p-4 rounded-card bg-card border border-border',
        className
      )}
    >
      <div className="space-y-1">
        <label className="text-body font-medium text-foreground">{title}</label>
        {description && (
          <p className="text-caption text-muted-foreground">{description}</p>
        )}
      </div>
      <div>{children}</div>
    </div>
  );
}

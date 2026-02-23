import { cn } from '@/lib/utils';

interface SettingsSectionProps {
  header: string;
  description?: string;
  children: React.ReactNode;
  className?: string;
}

export function SettingsSection({
  header,
  description,
  children,
  className,
}: SettingsSectionProps) {
  return (
    <section className={cn('space-y-3', className)}>
      <div className="px-1">
        <h2 className="text-heading text-foreground">{header}</h2>
        {description && (
          <p className="text-caption text-muted-foreground mt-0.5">
            {description}
          </p>
        )}
      </div>
      <div className="space-y-2">{children}</div>
    </section>
  );
}

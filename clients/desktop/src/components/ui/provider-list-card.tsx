import { cn } from '@/lib/utils';
import { Badge } from '@/components/ui/badge';
import type { ProviderConfig } from '@/lib/commands';
import type { PresetProvider } from '@/lib/presetProviders';

interface ProviderListCardProps {
  provider?: ProviderConfig;
  preset?: PresetProvider;
  isSelected: boolean;
  isConfigured: boolean;
  isActive: boolean;
  isDefault: boolean;
  onClick: () => void;
}

export function ProviderListCard({
  provider,
  preset,
  isSelected,
  isConfigured,
  isActive,
  isDefault,
  onClick,
}: ProviderListCardProps) {
  const name = provider?.name || preset?.name || 'Unknown';
  const Icon = preset?.icon;
  const color = preset?.color || '#6B7280';

  return (
    <button
      onClick={onClick}
      className={cn(
        'w-full flex items-center gap-sm px-sm py-xs rounded-sm transition-colors text-left',
        isSelected
          ? 'bg-primary/10 border border-primary/30'
          : 'hover:bg-muted/50 border border-transparent',
      )}
    >
      {/* Icon */}
      <div
        className="w-8 h-8 rounded-sm flex items-center justify-center shrink-0"
        style={{ backgroundColor: `${color}15` }}
      >
        {Icon ? (
          <Icon className="w-4 h-4" style={{ color }} />
        ) : (
          <span className="text-sm" style={{ color }}>
            {name.charAt(0).toUpperCase()}
          </span>
        )}
      </div>

      {/* Name and status */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-1.5">
          <span className="text-body font-medium text-foreground truncate">
            {name}
          </span>
          {isDefault && (
            <Badge variant="secondary" className="text-[10px] px-1 py-0">
              Default
            </Badge>
          )}
        </div>
        {provider?.model && (
          <p className="text-caption text-muted-foreground truncate">
            {provider.model}
          </p>
        )}
      </div>

      {/* Status indicator */}
      <div className="shrink-0">
        {isConfigured ? (
          <div
            className={cn(
              'w-2 h-2 rounded-full',
              isActive ? 'bg-green-500' : 'bg-gray-400'
            )}
            title={isActive ? 'Active' : 'Inactive'}
          />
        ) : (
          <div className="w-2 h-2 rounded-full border border-muted-foreground/30" />
        )}
      </div>
    </button>
  );
}

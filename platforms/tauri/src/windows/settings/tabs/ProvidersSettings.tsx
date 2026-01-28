import { useState, useMemo } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { ProviderListCard } from '@/components/ui/provider-list-card';
import { ProviderEditPanel } from '@/components/ui/provider-edit-panel';
import { Plus, Search } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { ProviderConfig } from '@/lib/commands';
import { presetProviders, type PresetProvider } from '@/lib/presetProviders';

export function ProvidersSettings() {
  const { t } = useTranslation();
  const providers = useSettingsStore((s) => s.providers);
  const updateProviders = useSettingsStore((s) => s.updateProviders);

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [isAddingNew, setIsAddingNew] = useState(false);

  // Combine presets and configured providers
  const allProviders = useMemo(() => {
    const configured = new Map(providers.providers.map((p) => [p.id, p]));

    // Start with presets
    const items: Array<{
      id: string;
      preset?: PresetProvider;
      provider?: ProviderConfig;
    }> = presetProviders.map((preset) => ({
      id: preset.id,
      preset,
      provider: configured.get(preset.id),
    }));

    // Add custom providers not in presets
    providers.providers.forEach((provider) => {
      if (!presetProviders.find((p) => p.id === provider.id)) {
        items.push({
          id: provider.id,
          provider,
        });
      }
    });

    return items;
  }, [providers.providers]);

  // Filter by search
  const filteredProviders = useMemo(() => {
    if (!searchQuery.trim()) return allProviders;
    const query = searchQuery.toLowerCase();
    return allProviders.filter(
      (item) =>
        item.preset?.name.toLowerCase().includes(query) ||
        item.provider?.name.toLowerCase().includes(query) ||
        item.preset?.description?.toLowerCase().includes(query)
    );
  }, [allProviders, searchQuery]);

  // Get selected item
  const selectedItem = useMemo(() => {
    if (!selectedId) return null;
    return allProviders.find((item) => item.id === selectedId) || null;
  }, [selectedId, allProviders]);

  const isConfigured = (id: string) =>
    providers.providers.some((p) => p.id === id);
  const isActive = (id: string) =>
    providers.providers.find((p) => p.id === id)?.enabled ?? false;
  const isDefault = (id: string) => providers.default_provider_id === id;

  const handleSelect = (id: string) => {
    setSelectedId(id);
    const item = allProviders.find((i) => i.id === id);
    // If selecting an unconfigured preset, enter "add new" mode
    setIsAddingNew(!!item?.preset && !item?.provider);
  };

  const handleAddCustom = () => {
    setSelectedId(null);
    setIsAddingNew(true);
  };

  const handleSave = (provider: ProviderConfig) => {
    const existingIndex = providers.providers.findIndex(
      (p) => p.id === provider.id
    );
    if (existingIndex >= 0) {
      // Update existing
      updateProviders({
        providers: providers.providers.map((p) =>
          p.id === provider.id ? provider : p
        ),
      });
    } else {
      // Add new
      updateProviders({
        providers: [...providers.providers, provider],
      });
    }
    setIsAddingNew(false);
  };

  const handleDelete = () => {
    if (!selectedId) return;
    updateProviders({
      providers: providers.providers.filter((p) => p.id !== selectedId),
      default_provider_id:
        providers.default_provider_id === selectedId
          ? ''
          : providers.default_provider_id,
    });
    setSelectedId(null);
  };

  const handleSetDefault = () => {
    if (!selectedId) return;
    updateProviders({
      default_provider_id: selectedId,
      providers: providers.providers.map((p) => ({
        ...p,
        is_default: p.id === selectedId,
      })),
    });
  };

  return (
    <div className="h-full flex flex-col">
      {/* Header */}
      <div className="px-lg pt-lg pb-md">
        <h1 className="text-title mb-1">{t('settings.providers.title')}</h1>
        <p className="text-caption text-muted-foreground">
          {t('settings.providers.description')}
        </p>
      </div>

      {/* Toolbar */}
      <div className="px-lg pb-md flex items-center gap-md">
        {/* Search */}
        <div className="relative w-60">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
          <Input
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder={t('settings.providers.searchPlaceholder', 'Search providers...')}
            className="pl-9"
          />
        </div>
        <div className="flex-1" />
        {/* Add Custom */}
        <Button onClick={handleAddCustom}>
          <Plus className="w-4 h-4 mr-1.5" />
          {t('settings.providers.addCustom', 'Add Custom')}
        </Button>
      </div>

      {/* Two-panel layout */}
      <div className="flex-1 flex gap-md px-lg pb-lg min-h-0">
        {/* Left: Provider List */}
        <aside className="w-60 shrink-0 flex flex-col rounded-md border border-border bg-muted/30 overflow-hidden">
          <div className="flex-1 overflow-y-auto p-sm space-y-xs">
            {filteredProviders.map((item) => (
              <ProviderListCard
                key={item.id}
                preset={item.preset}
                provider={item.provider}
                isSelected={selectedId === item.id}
                isConfigured={isConfigured(item.id)}
                isActive={isActive(item.id)}
                isDefault={isDefault(item.id)}
                onClick={() => handleSelect(item.id)}
              />
            ))}
            {filteredProviders.length === 0 && (
              <p className="text-caption text-muted-foreground text-center py-md">
                {t('settings.providers.noResults', 'No providers found')}
              </p>
            )}
          </div>
        </aside>

        {/* Right: Edit Panel */}
        <main className="flex-1 rounded-md border border-border bg-card overflow-hidden">
          {selectedItem || isAddingNew ? (
            <ProviderEditPanel
              provider={selectedItem?.provider || null}
              preset={selectedItem?.preset || null}
              isDefault={selectedId ? isDefault(selectedId) : false}
              isNew={isAddingNew}
              onSave={handleSave}
              onDelete={handleDelete}
              onSetDefault={handleSetDefault}
            />
          ) : (
            <div className="flex items-center justify-center h-full text-muted-foreground">
              <div className="text-center">
                <p className="text-body">
                  {t('settings.providers.selectProvider', 'Select a provider to configure')}
                </p>
                <p className="text-caption mt-1">
                  {t('settings.providers.selectProviderHint', 'Or click "Add Custom" to create a new one')}
                </p>
              </div>
            </div>
          )}
        </main>
      </div>
    </div>
  );
}

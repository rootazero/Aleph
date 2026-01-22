import { useState, useEffect } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Plus, Star, Trash2, Settings, Eye, EyeOff, Check, AlertCircle } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';
import {
  generationProviders,
  categoryMeta,
  type GenerationCategory,
  type GenerationPresetProvider,
} from '@/lib/generationPresets';
import type { GenerationProviderConfig } from '@/lib/commands';

// Check if a preset provider is already configured
function isPresetConfigured(
  presetId: string,
  providers: GenerationProviderConfig[]
): GenerationProviderConfig | undefined {
  return providers.find(
    (p) => p.id === presetId || p.name.toLowerCase() === presetId.toLowerCase()
  );
}

function GenerationProviderCard({
  preset,
  isConfigured,
  onClick,
}: {
  preset: GenerationPresetProvider;
  isConfigured: boolean;
  onClick: () => void;
}) {
  const { t } = useTranslation();
  const Icon = preset.icon;

  return (
    <button
      onClick={onClick}
      disabled={preset.isUnsupported}
      className={cn(
        'relative flex flex-col items-center gap-2 p-4 rounded-card border transition-all text-left',
        preset.isUnsupported
          ? 'opacity-50 cursor-not-allowed border-border/50 bg-muted/30'
          : 'hover:border-primary/50 hover:bg-accent/30',
        isConfigured && !preset.isUnsupported
          ? 'border-primary/30 bg-primary/5'
          : 'border-border bg-card'
      )}
    >
      {isConfigured && !preset.isUnsupported && (
        <div className="absolute top-2 right-2">
          <Check className="h-4 w-4 text-primary" />
        </div>
      )}
      {preset.isUnsupported && (
        <div className="absolute top-2 right-2">
          <AlertCircle className="h-4 w-4 text-muted-foreground" />
        </div>
      )}
      <div
        className="w-10 h-10 rounded-lg flex items-center justify-center"
        style={{ backgroundColor: `${preset.color}20` }}
      >
        <Icon className="h-5 w-5" style={{ color: preset.color }} />
      </div>
      <div className="text-center w-full">
        <p className="text-body font-medium text-foreground">{preset.name}</p>
        <p className="text-caption text-muted-foreground line-clamp-2">
          {preset.description}
        </p>
      </div>
      {preset.isUnsupported ? (
        <span className="text-xs text-muted-foreground">
          {t('settings.generationProviders.comingSoon')}
        </span>
      ) : isConfigured ? (
        <span className="text-xs text-primary font-medium">
          {t('settings.generationProviders.configured')}
        </span>
      ) : null}
    </button>
  );
}

function ConfiguredProviderCard({
  provider,
  isDefault,
  onToggle,
  onSetDefault,
  onEdit,
  onDelete,
}: {
  provider: GenerationProviderConfig;
  isDefault: boolean;
  onToggle: () => void;
  onSetDefault: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();

  // Find matching preset for icon
  const allPresets = [
    ...generationProviders.image,
    ...generationProviders.video,
    ...generationProviders.audio,
  ];
  const matchingPreset = allPresets.find(
    (p) => p.id === provider.id || p.name.toLowerCase() === provider.name.toLowerCase()
  );

  return (
    <div
      className={cn(
        'p-4 rounded-card border transition-colors',
        provider.enabled ? 'border-border bg-card' : 'border-border/50 bg-muted/30'
      )}
    >
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          {matchingPreset ? (
            <div
              className="w-8 h-8 rounded-lg flex items-center justify-center"
              style={{ backgroundColor: `${matchingPreset.color}20` }}
            >
              <matchingPreset.icon
                className="h-4 w-4"
                style={{ color: matchingPreset.color }}
              />
            </div>
          ) : (
            <div className="w-8 h-8 rounded-lg bg-muted flex items-center justify-center">
              <Settings className="h-4 w-4 text-muted-foreground" />
            </div>
          )}
          <div>
            <div className="flex items-center gap-2">
              <span className="text-body font-medium text-foreground">
                {provider.name}
              </span>
              {isDefault && (
                <Star className="h-4 w-4 text-yellow-500 fill-yellow-500" />
              )}
            </div>
            <span className="text-caption text-muted-foreground">
              {t(`settings.generationProviders.categories.${provider.category}`)}
              {provider.model && ` · ${provider.model}`}
            </span>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="icon"
            onClick={onSetDefault}
            title={t('common.setAsDefault')}
            className={cn(isDefault && 'text-yellow-500')}
          >
            <Star className={cn('h-4 w-4', isDefault && 'fill-current')} />
          </Button>
          <Button variant="ghost" size="icon" onClick={onEdit} title={t('common.edit')}>
            <Settings className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={onDelete}
            title={t('common.delete')}
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
          <Switch checked={provider.enabled} onCheckedChange={onToggle} />
        </div>
      </div>
    </div>
  );
}

function GenerationProviderDialog({
  open,
  onOpenChange,
  provider,
  preset,
  onSave,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  provider: GenerationProviderConfig | null;
  preset: GenerationPresetProvider | null;
  onSave: (provider: GenerationProviderConfig) => void;
}) {
  const { t } = useTranslation();
  const [form, setForm] = useState<GenerationProviderConfig>({
    id: crypto.randomUUID(),
    name: '',
    type: 'openai',
    category: 'image',
    api_key: '',
    base_url: '',
    model: '',
    enabled: true,
    is_default: false,
  });
  const [showApiKey, setShowApiKey] = useState(false);

  // Update form when provider or preset changes
  useEffect(() => {
    if (provider) {
      setForm(provider);
    } else if (preset) {
      setForm({
        id: preset.id,
        name: preset.name,
        type: preset.type,
        category: preset.category,
        api_key: '',
        base_url: preset.baseUrl || '',
        model: preset.defaultModel,
        enabled: true,
        is_default: false,
      });
    } else {
      setForm({
        id: crypto.randomUUID(),
        name: '',
        type: 'openai',
        category: 'image',
        api_key: '',
        base_url: '',
        model: '',
        enabled: true,
        is_default: false,
      });
    }
  }, [provider, preset, open]);

  const handleSave = () => {
    if (form.name.trim()) {
      onSave(form);
      onOpenChange(false);
    }
  };

  const dialogTitle = provider
    ? t('settings.generationProviders.editProvider')
    : preset
      ? t('settings.generationProviders.configurePreset', { name: preset.name })
      : t('settings.generationProviders.addProvider');

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{dialogTitle}</DialogTitle>
          <DialogDescription>
            {t('settings.generationProviders.configureDescription')}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <label className="text-body font-medium">
              {t('settings.generationProviders.providerName')}
            </label>
            <Input
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="My DALL-E"
            />
          </div>

          <div className="space-y-2">
            <label className="text-body font-medium">
              {t('settings.generationProviders.apiKey')}
            </label>
            <div className="relative">
              <Input
                type={showApiKey ? 'text' : 'password'}
                value={form.api_key || ''}
                onChange={(e) => setForm({ ...form, api_key: e.target.value })}
                placeholder="sk-..."
                className="pr-10"
              />
              <button
                type="button"
                onClick={() => setShowApiKey(!showApiKey)}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
              >
                {showApiKey ? (
                  <EyeOff className="h-4 w-4" />
                ) : (
                  <Eye className="h-4 w-4" />
                )}
              </button>
            </div>
          </div>

          {form.base_url && (
            <div className="space-y-2">
              <label className="text-body font-medium">
                {t('settings.generationProviders.baseUrl')}
              </label>
              <Input
                value={form.base_url || ''}
                onChange={(e) => setForm({ ...form, base_url: e.target.value })}
                placeholder="https://api.example.com"
              />
            </div>
          )}

          <div className="space-y-2">
            <label className="text-body font-medium">
              {t('settings.generationProviders.model')}
            </label>
            <Input
              value={form.model || ''}
              onChange={(e) => setForm({ ...form, model: e.target.value })}
              placeholder={preset?.defaultModel || 'model-name'}
            />
          </div>
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </Button>
          <Button onClick={handleSave} disabled={!form.name.trim()}>
            {provider ? t('common.save') : t('common.add')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function GenerationProvidersSettings() {
  const { t } = useTranslation();
  const generationProvidersState = useSettingsStore((s) => s.generationProviders);
  const updateGenerationProviders = useSettingsStore((s) => s.updateGenerationProviders);

  const [activeCategory, setActiveCategory] = useState<GenerationCategory>('image');
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingProvider, setEditingProvider] = useState<GenerationProviderConfig | null>(null);
  const [selectedPreset, setSelectedPreset] = useState<GenerationPresetProvider | null>(null);

  // Get providers for the active category
  const categoryProviders = generationProvidersState.providers.filter(
    (p) => p.category === activeCategory
  );

  const handleToggle = (id: string) => {
    updateGenerationProviders({
      providers: generationProvidersState.providers.map((p) =>
        p.id === id ? { ...p, enabled: !p.enabled } : p
      ),
    });
  };

  const handleSetDefault = (id: string, category: GenerationCategory) => {
    const defaultKey = `default_${category}_provider_id` as keyof typeof generationProvidersState;
    updateGenerationProviders({
      [defaultKey]: id,
      providers: generationProvidersState.providers.map((p) => ({
        ...p,
        is_default: p.category === category ? p.id === id : p.is_default,
      })),
    });
  };

  const handleEdit = (provider: GenerationProviderConfig) => {
    setEditingProvider(provider);
    setSelectedPreset(null);
    setDialogOpen(true);
  };

  const handleDelete = (id: string) => {
    const provider = generationProvidersState.providers.find((p) => p.id === id);
    if (!provider) return;

    const defaultKey = `default_${provider.category}_provider_id` as keyof typeof generationProvidersState;
    const currentDefault = generationProvidersState[defaultKey];

    updateGenerationProviders({
      providers: generationProvidersState.providers.filter((p) => p.id !== id),
      [defaultKey]: currentDefault === id ? '' : currentDefault,
    });
  };

  const handleSave = (provider: GenerationProviderConfig) => {
    const existingIndex = generationProvidersState.providers.findIndex(
      (p) => p.id === provider.id
    );
    if (existingIndex >= 0) {
      // Update existing
      updateGenerationProviders({
        providers: generationProvidersState.providers.map((p) =>
          p.id === provider.id ? provider : p
        ),
      });
    } else {
      // Add new
      updateGenerationProviders({
        providers: [...generationProvidersState.providers, provider],
      });
    }
    setEditingProvider(null);
    setSelectedPreset(null);
  };

  const handleAddNew = () => {
    setEditingProvider(null);
    setSelectedPreset(null);
    setDialogOpen(true);
  };

  const handlePresetClick = (preset: GenerationPresetProvider) => {
    if (preset.isUnsupported) return;

    const existingProvider = isPresetConfigured(
      preset.id,
      generationProvidersState.providers
    );
    if (existingProvider) {
      // Edit existing provider
      setEditingProvider(existingProvider);
      setSelectedPreset(null);
    } else {
      // Configure new preset
      setEditingProvider(null);
      setSelectedPreset(preset);
    }
    setDialogOpen(true);
  };

  const getDefaultForCategory = (category: GenerationCategory): string => {
    const key = `default_${category}_provider_id` as keyof typeof generationProvidersState;
    return generationProvidersState[key] as string;
  };

  return (
    <div className="space-y-6 max-w-2xl">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-title mb-1">{t('settings.generationProviders.title')}</h1>
          <p className="text-caption text-muted-foreground">
            {t('settings.generationProviders.description')}
          </p>
        </div>
        <Button onClick={handleAddNew}>
          <Plus className="h-4 w-4 mr-2" />
          {t('settings.generationProviders.addProvider')}
        </Button>
      </div>

      {/* Category Tabs */}
      <Tabs
        value={activeCategory}
        onValueChange={(v) => setActiveCategory(v as GenerationCategory)}
      >
        <TabsList className="grid w-full grid-cols-3">
          {(Object.keys(categoryMeta) as GenerationCategory[]).map((category) => {
            const meta = categoryMeta[category];
            const Icon = meta.icon;
            return (
              <TabsTrigger key={category} value={category} className="gap-2">
                <Icon className="h-4 w-4" />
                {t(meta.labelKey)}
              </TabsTrigger>
            );
          })}
        </TabsList>
      </Tabs>

      {/* Preset Providers Grid */}
      <div className="space-y-3">
        <h2 className="text-body font-medium text-foreground">
          {t('settings.generationProviders.presets')}
        </h2>
        <div className="grid grid-cols-2 sm:grid-cols-3 gap-3">
          {generationProviders[activeCategory].map((preset) => (
            <GenerationProviderCard
              key={preset.id}
              preset={preset}
              isConfigured={
                !!isPresetConfigured(preset.id, generationProvidersState.providers)
              }
              onClick={() => handlePresetClick(preset)}
            />
          ))}
        </div>
      </div>

      {/* Configured Providers List */}
      {categoryProviders.length > 0 && (
        <div className="space-y-3">
          <h2 className="text-body font-medium text-foreground">
            {t('settings.generationProviders.configuredProviders')}
          </h2>
          <div className="space-y-3">
            {categoryProviders.map((provider) => (
              <ConfiguredProviderCard
                key={provider.id}
                provider={provider}
                isDefault={provider.id === getDefaultForCategory(provider.category)}
                onToggle={() => handleToggle(provider.id)}
                onSetDefault={() => handleSetDefault(provider.id, provider.category)}
                onEdit={() => handleEdit(provider)}
                onDelete={() => handleDelete(provider.id)}
              />
            ))}
          </div>
        </div>
      )}

      {/* Empty state */}
      {categoryProviders.length === 0 && (
        <div className="text-center py-8 text-muted-foreground border border-dashed rounded-card">
          <p>{t('settings.generationProviders.noProviders')}</p>
          <p className="text-caption mt-1">
            {t('settings.generationProviders.noProvidersHint')}
          </p>
        </div>
      )}

      <GenerationProviderDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        provider={editingProvider}
        preset={selectedPreset}
        onSave={handleSave}
      />
    </div>
  );
}

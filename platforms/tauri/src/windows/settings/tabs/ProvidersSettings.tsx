import { useState } from 'react';
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Plus, Star, Trash2, Settings, Eye, EyeOff } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';
import type { ProviderConfig } from '@/lib/commands';

const providerIcons: Record<string, string> = {
  openai: '🤖',
  anthropic: '🧠',
  gemini: '✨',
  ollama: '🦙',
  custom: '⚙️',
};

function ProviderCard({
  provider,
  isDefault,
  onToggle,
  onSetDefault,
  onEdit,
  onDelete,
}: {
  provider: ProviderConfig;
  isDefault: boolean;
  onToggle: () => void;
  onSetDefault: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();

  return (
    <div
      className={cn(
        'p-4 rounded-card border transition-colors',
        provider.enabled ? 'border-border bg-card' : 'border-border/50 bg-muted/30'
      )}
    >
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <span className="text-2xl">{providerIcons[provider.type]}</span>
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
              {t(`settings.providers.types.${provider.type}`)}
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

function ProviderDialog({
  open,
  onOpenChange,
  provider,
  onSave,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  provider: ProviderConfig | null;
  onSave: (provider: ProviderConfig) => void;
}) {
  const { t } = useTranslation();
  const [form, setForm] = useState<ProviderConfig>(
    provider || {
      id: crypto.randomUUID(),
      name: '',
      type: 'openai',
      api_key: '',
      base_url: '',
      model: '',
      enabled: true,
      is_default: false,
    }
  );
  const [showApiKey, setShowApiKey] = useState(false);

  const handleSave = () => {
    if (form.name.trim()) {
      onSave(form);
      onOpenChange(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            {provider ? t('settings.providers.editProvider') : t('settings.providers.addProvider')}
          </DialogTitle>
          <DialogDescription>
            {t('settings.providers.configureDescription')}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <label className="text-body font-medium">{t('settings.providers.providerName')}</label>
            <Input
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="My OpenAI"
            />
          </div>

          <div className="space-y-2">
            <label className="text-body font-medium">{t('settings.providers.providerType')}</label>
            <Select
              value={form.type}
              onValueChange={(value) =>
                setForm({
                  ...form,
                  type: value as ProviderConfig['type'],
                })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="openai">{t('settings.providers.types.openai')}</SelectItem>
                <SelectItem value="anthropic">{t('settings.providers.types.anthropic')}</SelectItem>
                <SelectItem value="gemini">{t('settings.providers.types.gemini')}</SelectItem>
                <SelectItem value="ollama">{t('settings.providers.types.ollama')}</SelectItem>
                <SelectItem value="custom">{t('settings.providers.types.custom')}</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-2">
            <label className="text-body font-medium">{t('settings.providers.apiKey')}</label>
            <div className="relative">
              <Input
                type={showApiKey ? 'text' : 'password'}
                value={form.api_key || ''}
                onChange={(e) => setForm({ ...form, api_key: e.target.value })}
                placeholder={form.type === 'ollama' ? t('common.notRequired') : 'sk-...'}
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

          {(form.type === 'custom' || form.type === 'ollama') && (
            <div className="space-y-2">
              <label className="text-body font-medium">{t('settings.providers.baseUrl')}</label>
              <Input
                value={form.base_url || ''}
                onChange={(e) => setForm({ ...form, base_url: e.target.value })}
                placeholder={
                  form.type === 'ollama'
                    ? 'http://localhost:11434'
                    : 'https://api.example.com/v1'
                }
              />
            </div>
          )}

          <div className="space-y-2">
            <label className="text-body font-medium">{t('settings.providers.model')}</label>
            <Input
              value={form.model || ''}
              onChange={(e) => setForm({ ...form, model: e.target.value })}
              placeholder={
                form.type === 'openai'
                  ? 'gpt-4'
                  : form.type === 'anthropic'
                    ? 'claude-3-opus'
                    : form.type === 'gemini'
                      ? 'gemini-pro'
                      : form.type === 'ollama'
                        ? 'llama2'
                        : 'model-name'
              }
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

export function ProvidersSettings() {
  const { t } = useTranslation();
  const providers = useSettingsStore((s) => s.providers);
  const updateProviders = useSettingsStore((s) => s.updateProviders);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingProvider, setEditingProvider] = useState<ProviderConfig | null>(
    null
  );

  const handleToggle = (id: string) => {
    updateProviders({
      providers: providers.providers.map((p) =>
        p.id === id ? { ...p, enabled: !p.enabled } : p
      ),
    });
  };

  const handleSetDefault = (id: string) => {
    updateProviders({
      default_provider_id: id,
      providers: providers.providers.map((p) => ({
        ...p,
        is_default: p.id === id,
      })),
    });
  };

  const handleEdit = (provider: ProviderConfig) => {
    setEditingProvider(provider);
    setDialogOpen(true);
  };

  const handleDelete = (id: string) => {
    updateProviders({
      providers: providers.providers.filter((p) => p.id !== id),
      default_provider_id:
        providers.default_provider_id === id ? '' : providers.default_provider_id,
    });
  };

  const handleSave = (provider: ProviderConfig) => {
    if (editingProvider) {
      updateProviders({
        providers: providers.providers.map((p) =>
          p.id === provider.id ? provider : p
        ),
      });
    } else {
      updateProviders({
        providers: [...providers.providers, provider],
      });
    }
    setEditingProvider(null);
  };

  const handleAddNew = () => {
    setEditingProvider(null);
    setDialogOpen(true);
  };

  return (
    <div className="space-y-6 max-w-2xl">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-title mb-1">{t('settings.providers.title')}</h1>
          <p className="text-caption text-muted-foreground">
            {t('settings.providers.description')}
          </p>
        </div>
        <Button onClick={handleAddNew}>
          <Plus className="h-4 w-4 mr-2" />
          {t('settings.providers.addProvider')}
        </Button>
      </div>

      <div className="space-y-3">
        {providers.providers.length === 0 ? (
          <div className="text-center py-12 text-muted-foreground">
            <p>{t('settings.providers.noProviders')}</p>
            <p className="text-caption mt-1">{t('settings.providers.noProvidersHint')}</p>
          </div>
        ) : (
          providers.providers.map((provider) => (
            <ProviderCard
              key={provider.id}
              provider={provider}
              isDefault={provider.id === providers.default_provider_id}
              onToggle={() => handleToggle(provider.id)}
              onSetDefault={() => handleSetDefault(provider.id)}
              onEdit={() => handleEdit(provider)}
              onDelete={() => handleDelete(provider.id)}
            />
          ))
        )}
      </div>

      <ProviderDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        provider={editingProvider}
        onSave={handleSave}
      />
    </div>
  );
}

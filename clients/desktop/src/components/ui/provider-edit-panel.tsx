import { useState, useEffect } from 'react';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import { InfoBox } from '@/components/ui/info-box';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Eye, EyeOff, Zap, Star, Trash2, Loader2, CheckCircle, XCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { ProviderConfig } from '@/lib/commands';
import type { PresetProvider } from '@/lib/presetProviders';
import { cn } from '@/lib/utils';

type TestResult = 'idle' | 'testing' | 'success' | 'error';

interface ProviderEditPanelProps {
  provider: ProviderConfig | null;
  preset: PresetProvider | null;
  isDefault: boolean;
  isNew: boolean;
  onSave: (provider: ProviderConfig) => void;
  onDelete: () => void;
  onSetDefault: () => void;
  onTestConnection?: (provider: ProviderConfig) => Promise<boolean>;
}

export function ProviderEditPanel({
  provider,
  preset,
  isDefault,
  isNew,
  onSave,
  onDelete,
  onSetDefault,
  onTestConnection,
}: ProviderEditPanelProps) {
  const { t } = useTranslation();
  const [form, setForm] = useState<ProviderConfig>({
    id: '',
    name: '',
    type: 'openai',
    api_key: '',
    base_url: '',
    model: '',
    enabled: true,
    is_default: false,
  });
  const [showApiKey, setShowApiKey] = useState(false);
  const [testResult, setTestResult] = useState<TestResult>('idle');
  const [testMessage, setTestMessage] = useState('');

  // Update form when provider/preset changes
  useEffect(() => {
    if (provider) {
      setForm(provider);
    } else if (preset) {
      setForm({
        id: preset.id,
        name: preset.name,
        type: preset.type,
        api_key: '',
        base_url: preset.baseUrl || '',
        model: preset.defaultModel,
        enabled: true,
        is_default: false,
      });
    }
    setTestResult('idle');
    setTestMessage('');
  }, [provider, preset]);

  const handleSave = () => {
    if (form.name.trim()) {
      onSave(form);
    }
  };

  const handleTestConnection = async () => {
    if (!onTestConnection) return;

    setTestResult('testing');
    setTestMessage('');

    try {
      const success = await onTestConnection(form);
      setTestResult(success ? 'success' : 'error');
      setTestMessage(success ? 'Connection successful!' : 'Connection failed');
    } catch (error) {
      setTestResult('error');
      setTestMessage(String(error));
    }

    // Auto-clear after 5 seconds
    setTimeout(() => {
      setTestResult('idle');
      setTestMessage('');
    }, 5000);
  };

  const showBaseUrl =
    form.type === 'custom' || form.type === 'ollama' || !!form.base_url;

  if (!provider && !preset) {
    return (
      <div className="flex items-center justify-center h-full text-muted-foreground">
        <p>{t('settings.providers.selectProvider', 'Select a provider to configure')}</p>
      </div>
    );
  }

  const Icon = preset?.icon;
  const color = preset?.color || '#6B7280';

  return (
    <div className="p-lg space-y-lg h-full overflow-y-auto">
      {/* Header */}
      <div className="flex items-center gap-md">
        {Icon && (
          <div
            className="w-12 h-12 rounded-md flex items-center justify-center"
            style={{ backgroundColor: `${color}15` }}
          >
            <Icon className="w-6 h-6" style={{ color }} />
          </div>
        )}
        <div className="flex-1">
          <h2 className="text-heading text-foreground">
            {isNew
              ? t('settings.providers.configurePreset', { name: preset?.name || 'Provider' })
              : provider?.name}
          </h2>
          <p className="text-caption text-muted-foreground">
            {preset?.description || t(`settings.providers.types.${form.type}`)}
          </p>
        </div>
        {!isNew && (
          <Button
            variant="ghost"
            size="icon"
            onClick={onSetDefault}
            className={cn(isDefault && 'text-yellow-500')}
            title={t('common.setAsDefault')}
          >
            <Star className={cn('w-5 h-5', isDefault && 'fill-current')} />
          </Button>
        )}
      </div>

      {/* Form Fields */}
      <div className="space-y-md">
        {/* Name */}
        <div className="space-y-xs">
          <label className="text-body font-medium text-foreground">
            {t('settings.providers.providerName')}
          </label>
          <Input
            value={form.name}
            onChange={(e) => setForm({ ...form, name: e.target.value })}
            placeholder="My Provider"
          />
        </div>

        {/* Type */}
        <div className="space-y-xs">
          <label className="text-body font-medium text-foreground">
            {t('settings.providers.providerType')}
          </label>
          <Select
            value={form.type}
            onValueChange={(value) =>
              setForm({ ...form, type: value as ProviderConfig['type'] })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="openai">OpenAI</SelectItem>
              <SelectItem value="anthropic">Anthropic</SelectItem>
              <SelectItem value="gemini">Google Gemini</SelectItem>
              <SelectItem value="ollama">Ollama</SelectItem>
              <SelectItem value="custom">Custom</SelectItem>
            </SelectContent>
          </Select>
        </div>

        {/* API Key */}
        <div className="space-y-xs">
          <label className="text-body font-medium text-foreground">
            {t('settings.providers.apiKey')}
          </label>
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
              className="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
            >
              {showApiKey ? <EyeOff className="w-4 h-4" /> : <Eye className="w-4 h-4" />}
            </button>
          </div>
        </div>

        {/* Base URL */}
        {showBaseUrl && (
          <div className="space-y-xs">
            <label className="text-body font-medium text-foreground">
              {t('settings.providers.baseUrl')}
            </label>
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

        {/* Model */}
        <div className="space-y-xs">
          <label className="text-body font-medium text-foreground">
            {t('settings.providers.model')}
          </label>
          <Input
            value={form.model || ''}
            onChange={(e) => setForm({ ...form, model: e.target.value })}
            placeholder={getModelPlaceholder(form.type)}
          />
        </div>

        {/* Enabled toggle (only for existing providers) */}
        {!isNew && (
          <div className="flex items-center justify-between py-sm">
            <span className="text-body text-foreground">
              {t('settings.providers.enabled', 'Enabled')}
            </span>
            <Switch
              checked={form.enabled}
              onCheckedChange={(checked) => setForm({ ...form, enabled: checked })}
            />
          </div>
        )}
      </div>

      {/* Test Connection */}
      {onTestConnection && (
        <div className="space-y-sm">
          <Button
            variant="outline"
            onClick={handleTestConnection}
            disabled={testResult === 'testing'}
            className="w-full"
          >
            {testResult === 'testing' ? (
              <>
                <Loader2 className="w-4 h-4 mr-2 animate-spin" />
                {t('settings.providers.testing', 'Testing...')}
              </>
            ) : (
              <>
                <Zap className="w-4 h-4 mr-2" />
                {t('settings.providers.testConnection', 'Test Connection')}
              </>
            )}
          </Button>
          {testResult === 'success' && (
            <InfoBox variant="success">
              <CheckCircle className="w-4 h-4 mr-1.5 inline" />
              {testMessage}
            </InfoBox>
          )}
          {testResult === 'error' && (
            <InfoBox variant="error">
              <XCircle className="w-4 h-4 mr-1.5 inline" />
              {testMessage}
            </InfoBox>
          )}
        </div>
      )}

      {/* Actions */}
      <div className="flex items-center justify-between pt-md border-t border-border">
        {!isNew ? (
          <Button
            variant="ghost"
            onClick={onDelete}
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="w-4 h-4 mr-2" />
            {t('common.delete')}
          </Button>
        ) : (
          <div />
        )}
        <Button onClick={handleSave} disabled={!form.name.trim()}>
          {isNew ? t('common.add') : t('common.save')}
        </Button>
      </div>
    </div>
  );
}

function getModelPlaceholder(type: string): string {
  switch (type) {
    case 'openai':
      return 'gpt-4o';
    case 'anthropic':
      return 'claude-3-5-sonnet';
    case 'gemini':
      return 'gemini-2.0-flash';
    case 'ollama':
      return 'llama3.2';
    default:
      return 'model-name';
  }
}

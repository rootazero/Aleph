import { useState, useEffect, useCallback } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { useGatewayStore, gateway } from '@/stores/gatewayStore';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { SettingsCard } from '@/components/ui/settings-card';
import { SettingsSection } from '@/components/ui/settings-section';
import { InfoBox } from '@/components/ui/info-box';
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
import {
  Plus,
  Plug,
  Trash2,
  RefreshCw,
  Download,
  GitBranch,
  FileArchive,
  Folder,
  Info,
  Loader2,
  AlertCircle,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';
import type { GWPluginInfo } from '@/lib/gateway';

const sourceIcons = {
  git: GitBranch,
  zip: FileArchive,
  local: Folder,
};

const sourceLabels = {
  git: 'Git Repository',
  zip: 'ZIP Archive',
  local: 'Local Folder',
};

function PluginCard({
  plugin,
  onToggle,
  onDelete,
  isToggling,
}: {
  plugin: GWPluginInfo;
  onToggle: () => void;
  onDelete: () => void;
  isToggling?: boolean;
}) {
  // Determine source from plugin info (Gateway doesn't provide source, default to git)
  const source = 'git' as keyof typeof sourceIcons;
  const Icon = sourceIcons[source];

  return (
    <div
      className={cn(
        'p-4 rounded-card border transition-colors',
        plugin.enabled ? 'border-border bg-card' : 'border-border/50 bg-muted/30'
      )}
    >
      <div className="flex items-start justify-between">
        <div className="flex items-start gap-3">
          <div className="w-10 h-10 rounded-medium bg-primary/10 flex items-center justify-center flex-shrink-0">
            <Plug className="h-5 w-5 text-primary" />
          </div>
          <div>
            <div className="flex items-center gap-2">
              <span className="text-body font-medium text-foreground">
                {plugin.name}
              </span>
              <span className="text-caption text-muted-foreground">
                v{plugin.version}
              </span>
            </div>
            <p className="text-caption text-muted-foreground mt-1 line-clamp-2">
              {plugin.description || 'No description'}
            </p>
            <div className="flex items-center gap-1 mt-2 text-caption text-muted-foreground">
              <Icon className="h-3 w-3" />
              <span>{sourceLabels[source]}</span>
            </div>
          </div>
        </div>

        <div className="flex items-center gap-2 flex-shrink-0 ml-4">
          <Button
            variant="ghost"
            size="icon"
            onClick={onDelete}
            title="Remove"
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
          {isToggling ? (
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          ) : (
            <Switch checked={plugin.enabled} onCheckedChange={onToggle} />
          )}
        </div>
      </div>
    </div>
  );
}

function InstallPluginDialog({
  open,
  onOpenChange,
  onInstall,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onInstall: (url: string) => Promise<void>;
}) {
  const [source, setSource] = useState<'git' | 'zip' | 'local'>('git');
  const [url, setUrl] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleInstall = async () => {
    if (!url.trim()) return;

    setIsLoading(true);
    setError(null);

    try {
      await onInstall(url);
      setUrl('');
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Installation failed');
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Install Plugin</DialogTitle>
          <DialogDescription>
            Install a plugin from Git repository, ZIP file, or local folder
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <label className="text-body font-medium">Source</label>
            <Select value={source} onValueChange={(v) => setSource(v as typeof source)}>
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="git">
                  <div className="flex items-center gap-2">
                    <GitBranch className="h-4 w-4" />
                    Git Repository
                  </div>
                </SelectItem>
                <SelectItem value="zip">
                  <div className="flex items-center gap-2">
                    <FileArchive className="h-4 w-4" />
                    ZIP Archive
                  </div>
                </SelectItem>
                <SelectItem value="local">
                  <div className="flex items-center gap-2">
                    <Folder className="h-4 w-4" />
                    Local Folder
                  </div>
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className="space-y-2">
            <label className="text-body font-medium">
              {source === 'git'
                ? 'Repository URL'
                : source === 'zip'
                  ? 'ZIP URL or Path'
                  : 'Folder Path'}
            </label>
            <Input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder={
                source === 'git'
                  ? 'https://github.com/user/plugin.git'
                  : source === 'zip'
                    ? 'https://example.com/plugin.zip'
                    : '/path/to/plugin'
              }
            />
          </div>

          {error && (
            <div className="flex items-center gap-2 text-destructive text-sm">
              <AlertCircle className="h-4 w-4" />
              {error}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={handleInstall} disabled={!url.trim() || isLoading}>
            {isLoading ? (
              <>
                <RefreshCw className="h-4 w-4 mr-2 animate-spin" />
                Installing...
              </>
            ) : (
              <>
                <Download className="h-4 w-4 mr-2" />
                Install
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function PluginsSettings() {
  const { t } = useTranslation();
  const localPlugins = useSettingsStore((s) => s.plugins);
  const updatePlugins = useSettingsStore((s) => s.updatePlugins);
  const isConnected = useGatewayStore((s) => s.isConnected);

  // Gateway-loaded plugins
  const [plugins, setPlugins] = useState<GWPluginInfo[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [togglingId, setTogglingId] = useState<string | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);

  // Load plugins from Gateway
  const loadPlugins = useCallback(async () => {
    if (!isConnected()) {
      // Fallback to local settings
      setPlugins(localPlugins.plugins.map(p => ({
        id: p.id,
        name: p.name,
        version: p.version,
        enabled: p.enabled,
        description: p.description,
      })));
      setIsLoading(false);
      return;
    }

    setIsLoading(true);
    setError(null);

    try {
      const result = await gateway.pluginsList();
      setPlugins(result);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load plugins');
      // Fallback to local
      setPlugins(localPlugins.plugins.map(p => ({
        id: p.id,
        name: p.name,
        version: p.version,
        enabled: p.enabled,
        description: p.description,
      })));
    } finally {
      setIsLoading(false);
    }
  }, [isConnected, localPlugins.plugins]);

  useEffect(() => {
    loadPlugins();
  }, [loadPlugins]);

  const handleToggle = async (plugin: GWPluginInfo) => {
    setTogglingId(plugin.id);

    try {
      if (isConnected()) {
        if (plugin.enabled) {
          await gateway.pluginsDisable(plugin.id);
        } else {
          await gateway.pluginsEnable(plugin.id);
        }
        // Reload to get updated state
        await loadPlugins();
      } else {
        // Fallback to local
        updatePlugins({
          plugins: localPlugins.plugins.map((p) =>
            p.id === plugin.id ? { ...p, enabled: !p.enabled } : p
          ),
        });
        setPlugins(prev => prev.map(p =>
          p.id === plugin.id ? { ...p, enabled: !p.enabled } : p
        ));
      }
    } catch (e) {
      console.error('Failed to toggle plugin:', e);
    } finally {
      setTogglingId(null);
    }
  };

  const handleDelete = async (plugin: GWPluginInfo) => {
    try {
      if (isConnected()) {
        await gateway.pluginsUninstall(plugin.id);
        await loadPlugins();
      } else {
        updatePlugins({
          plugins: localPlugins.plugins.filter((p) => p.id !== plugin.id),
        });
        setPlugins(prev => prev.filter(p => p.id !== plugin.id));
      }
    } catch (e) {
      console.error('Failed to delete plugin:', e);
    }
  };

  const handleInstall = async (url: string) => {
    if (isConnected()) {
      await gateway.pluginsInstall(url);
      await loadPlugins();
    } else {
      // Fallback to local simulation
      const plugin = {
        id: crypto.randomUUID(),
        name: url.split('/').pop()?.replace('.git', '').replace('.zip', '') || 'Plugin',
        version: '1.0.0',
        description: 'Newly installed plugin',
        source: 'git' as const,
        source_url: url,
        enabled: true,
      };
      updatePlugins({
        plugins: [...localPlugins.plugins, plugin],
      });
      setPlugins(prev => [...prev, {
        id: plugin.id,
        name: plugin.name,
        version: plugin.version,
        enabled: plugin.enabled,
        description: plugin.description,
      }]);
    }
  };

  return (
    <div className="space-y-lg max-w-3xl">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-title mb-1">{t('settings.plugins.title', 'Plugins')}</h1>
          <p className="text-caption text-muted-foreground">
            {t('settings.plugins.description', 'Extend Aleph with third-party plugins')}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="icon" onClick={loadPlugins} disabled={isLoading}>
            <RefreshCw className={cn("h-4 w-4", isLoading && "animate-spin")} />
          </Button>
          <Button onClick={() => setDialogOpen(true)}>
            <Plus className="h-4 w-4 mr-2" />
            {t('settings.plugins.installPlugin', 'Install Plugin')}
          </Button>
        </div>
      </div>

      {/* Error Message */}
      {error && (
        <InfoBox variant="error">
          <div className="flex items-center gap-2">
            <AlertCircle className="h-4 w-4" />
            <span>{error}</span>
          </div>
        </InfoBox>
      )}

      {/* Global Settings */}
      <SettingsSection header={t('settings.plugins.settingsSection', 'Settings')}>
        <SettingsCard
          title={t('settings.plugins.autoUpdate', 'Auto Update')}
          description={t('settings.plugins.autoUpdateDescription', 'Automatically update plugins when new versions are available')}
          icon={RefreshCw}
        >
          <Switch
            checked={localPlugins.auto_update}
            onCheckedChange={(checked) => updatePlugins({ auto_update: checked })}
          />
        </SettingsCard>
      </SettingsSection>

      {/* Installed Plugins */}
      <SettingsSection header={t('settings.plugins.installedSection', 'Installed Plugins ({{count}})', { count: plugins.length })}>
        {isLoading ? (
          <div className="flex items-center justify-center py-12">
            <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
          </div>
        ) : plugins.length === 0 ? (
          <div className="text-center py-12 text-muted-foreground border border-dashed border-border rounded-card">
            <Plug className="h-12 w-12 mx-auto mb-4 opacity-50" />
            <p>{t('settings.plugins.noPlugins', 'No plugins installed')}</p>
            <p className="text-caption mt-1">
              {t('settings.plugins.noPluginsHint', "Install plugins to extend Aleph's functionality")}
            </p>
          </div>
        ) : (
          <div className="space-y-sm">
            {plugins.map((plugin) => (
              <PluginCard
                key={plugin.id}
                plugin={plugin}
                onToggle={() => handleToggle(plugin)}
                onDelete={() => handleDelete(plugin)}
                isToggling={togglingId === plugin.id}
              />
            ))}
          </div>
        )}
      </SettingsSection>

      {/* Info */}
      <InfoBox variant="info">
        <div className="flex items-start gap-sm">
          <Info className="h-4 w-4 mt-0.5 flex-shrink-0" />
          <span>
            {t('settings.plugins.hint', 'Plugins can add new tools, integrations, and capabilities to Aleph. Install plugins from Git repositories, ZIP archives, or local folders.')}
          </span>
        </div>
      </InfoBox>

      <InstallPluginDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        onInstall={handleInstall}
      />
    </div>
  );
}

import { useState } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
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
  Settings,
  RefreshCw,
  Download,
  GitBranch,
  FileArchive,
  Folder,
  Info,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';
import type { Plugin } from '@/lib/commands';

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
  onConfigure,
  onDelete,
}: {
  plugin: Plugin;
  onToggle: () => void;
  onConfigure: () => void;
  onDelete: () => void;
}) {
  const Icon = sourceIcons[plugin.source];

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
              {plugin.description}
            </p>
            <div className="flex items-center gap-1 mt-2 text-caption text-muted-foreground">
              <Icon className="h-3 w-3" />
              <span>{sourceLabels[plugin.source]}</span>
              {plugin.source_url && (
                <span className="truncate max-w-48">· {plugin.source_url}</span>
              )}
            </div>
          </div>
        </div>

        <div className="flex items-center gap-2 flex-shrink-0 ml-4">
          <Button variant="ghost" size="icon" onClick={onConfigure} title="Configure">
            <Settings className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={onDelete}
            title="Remove"
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
          <Switch checked={plugin.enabled} onCheckedChange={onToggle} />
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
  onInstall: (plugin: Plugin) => void;
}) {
  const [source, setSource] = useState<'git' | 'zip' | 'local'>('git');
  const [url, setUrl] = useState('');
  const [isLoading, setIsLoading] = useState(false);

  const handleInstall = async () => {
    if (!url.trim()) return;

    setIsLoading(true);
    // Simulate installation
    await new Promise((resolve) => setTimeout(resolve, 1000));

    const plugin: Plugin = {
      id: crypto.randomUUID(),
      name: url.split('/').pop()?.replace('.git', '').replace('.zip', '') || 'Plugin',
      version: '1.0.0',
      description: 'Newly installed plugin',
      source,
      source_url: url,
      enabled: true,
    };

    onInstall(plugin);
    setUrl('');
    setIsLoading(false);
    onOpenChange(false);
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
  const plugins = useSettingsStore((s) => s.plugins);
  const updatePlugins = useSettingsStore((s) => s.updatePlugins);

  const [dialogOpen, setDialogOpen] = useState(false);

  const handleToggle = (id: string) => {
    updatePlugins({
      plugins: plugins.plugins.map((p) =>
        p.id === id ? { ...p, enabled: !p.enabled } : p
      ),
    });
  };

  const handleDelete = (id: string) => {
    updatePlugins({
      plugins: plugins.plugins.filter((p) => p.id !== id),
    });
  };

  const handleInstall = (plugin: Plugin) => {
    updatePlugins({
      plugins: [...plugins.plugins, plugin],
    });
  };

  const handleConfigure = (plugin: Plugin) => {
    // TODO: Open plugin configuration dialog
    console.log('Configure plugin:', plugin);
  };

  return (
    <div className="space-y-lg max-w-3xl">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-title mb-1">{t('settings.plugins.title', 'Plugins')}</h1>
          <p className="text-caption text-muted-foreground">
            {t('settings.plugins.description', 'Extend Aether with third-party plugins')}
          </p>
        </div>
        <Button onClick={() => setDialogOpen(true)}>
          <Plus className="h-4 w-4 mr-2" />
          {t('settings.plugins.installPlugin', 'Install Plugin')}
        </Button>
      </div>

      {/* Global Settings */}
      <SettingsSection header={t('settings.plugins.settingsSection', 'Settings')}>
        <SettingsCard
          title={t('settings.plugins.autoUpdate', 'Auto Update')}
          description={t('settings.plugins.autoUpdateDescription', 'Automatically update plugins when new versions are available')}
          icon={RefreshCw}
        >
          <Switch
            checked={plugins.auto_update}
            onCheckedChange={(checked) => updatePlugins({ auto_update: checked })}
          />
        </SettingsCard>
      </SettingsSection>

      {/* Installed Plugins */}
      <SettingsSection header={t('settings.plugins.installedSection', 'Installed Plugins ({{count}})', { count: plugins.plugins.length })}>
        {plugins.plugins.length === 0 ? (
          <div className="text-center py-12 text-muted-foreground border border-dashed border-border rounded-card">
            <Plug className="h-12 w-12 mx-auto mb-4 opacity-50" />
            <p>{t('settings.plugins.noPlugins', 'No plugins installed')}</p>
            <p className="text-caption mt-1">
              {t('settings.plugins.noPluginsHint', "Install plugins to extend Aether's functionality")}
            </p>
          </div>
        ) : (
          <div className="space-y-sm">
            {plugins.plugins.map((plugin) => (
              <PluginCard
                key={plugin.id}
                plugin={plugin}
                onToggle={() => handleToggle(plugin.id)}
                onConfigure={() => handleConfigure(plugin)}
                onDelete={() => handleDelete(plugin.id)}
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
            {t('settings.plugins.hint', 'Plugins can add new tools, integrations, and capabilities to Aether. Install plugins from Git repositories, ZIP archives, or local folders.')}
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

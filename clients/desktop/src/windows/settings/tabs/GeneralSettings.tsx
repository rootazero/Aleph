import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { SettingsSection } from '@/components/ui/settings-section';
import { Switch } from '@/components/ui/switch';
import { Button } from '@/components/ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import { commands } from '@/lib/commands';
import { changeLanguage, supportedLanguages } from '@/lib/i18n';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Volume2,
  Power,
  Globe,
  RefreshCw,
  FileText,
  Info,
  ExternalLink,
} from 'lucide-react';

export function GeneralSettings() {
  const { t } = useTranslation();
  const general = useSettingsStore((s) => s.general);
  const updateGeneral = useSettingsStore((s) => s.updateGeneral);
  const [version, setVersion] = useState('');
  const [buildNumber, setBuildNumber] = useState('');
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [autostartLoading, setAutostartLoading] = useState(false);
  const [checkingUpdates, setCheckingUpdates] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [logsLoading, setLogsLoading] = useState(false);

  useEffect(() => {
    commands.getAppVersion().then((v) => {
      setVersion(v.version);
      setBuildNumber(v.build || '');
    });

    // Get actual autostart status from system
    commands
      .getAutostartEnabled()
      .then((enabled) => {
        setAutostartEnabled(enabled);
        // Sync with settings if different
        if (enabled !== general.launch_at_login) {
          updateGeneral({ launch_at_login: enabled });
        }
      })
      .catch(console.error);
  }, []);

  const handleAutostartChange = async (checked: boolean) => {
    setAutostartLoading(true);
    try {
      await commands.setAutostartEnabled(checked);
      setAutostartEnabled(checked);
      updateGeneral({ launch_at_login: checked });
    } catch (error) {
      console.error('Failed to set autostart:', error);
      // Revert UI on error
      setAutostartEnabled(!checked);
    } finally {
      setAutostartLoading(false);
    }
  };

  const handleLanguageChange = (value: string) => {
    updateGeneral({ language: value });
    changeLanguage(value);
  };

  const handleCheckUpdates = async () => {
    setCheckingUpdates(true);
    try {
      // Open GitHub releases page in browser
      window.open('https://github.com/rootazero/Aleph/releases', '_blank');
    } finally {
      setCheckingUpdates(false);
    }
  };

  const handleLoadLogs = async () => {
    setLogsLoading(true);
    try {
      const logContent = await commands.getLogs();
      setLogs(logContent.split('\n'));
    } catch (error) {
      console.error('Failed to load logs:', error);
      setLogs(['Failed to load logs']);
    } finally {
      setLogsLoading(false);
    }
  };

  return (
    <div className="space-y-lg max-w-2xl">
      {/* Page Header */}
      <div>
        <h1 className="text-title mb-1">{t('settings.general.title')}</h1>
        <p className="text-caption text-muted-foreground">
          {t('settings.general.description')}
        </p>
      </div>

      {/* Sound Section */}
      <SettingsSection header={t('settings.general.soundSection', 'Sound')}>
        <SettingsCard
          title={t('settings.general.sound')}
          description={t('settings.general.soundDescription')}
          icon={Volume2}
        >
          <Switch
            checked={general.sound_enabled}
            onCheckedChange={(checked) =>
              updateGeneral({ sound_enabled: checked })
            }
          />
        </SettingsCard>
      </SettingsSection>

      {/* Startup Section */}
      <SettingsSection header={t('settings.general.startupSection', 'Startup')}>
        <SettingsCard
          title={t('settings.general.launchAtLogin')}
          description={t('settings.general.launchAtLoginDescription')}
          icon={Power}
        >
          <Switch
            checked={autostartEnabled}
            onCheckedChange={handleAutostartChange}
            disabled={autostartLoading}
          />
        </SettingsCard>
      </SettingsSection>

      {/* Language Section */}
      <SettingsSection header={t('settings.general.languageSection', 'Language')}>
        <SettingsCard
          title={t('settings.general.language')}
          description={t('settings.general.languageDescription')}
          icon={Globe}
        >
          <Select value={general.language} onValueChange={handleLanguageChange}>
            <SelectTrigger className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">
                {t('common.systemDefault')}
              </SelectItem>
              {supportedLanguages.map((lang) => (
                <SelectItem key={lang.code} value={lang.code}>
                  {lang.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </SettingsCard>
      </SettingsSection>

      {/* Updates Section */}
      <SettingsSection header={t('settings.general.updatesSection', 'Updates')}>
        <SettingsCard
          title={t('settings.general.checkUpdates', 'Check for Updates')}
          description={t(
            'settings.general.checkUpdatesDescription',
            'Check if a newer version is available'
          )}
          icon={RefreshCw}
        >
          <Button
            variant="outline"
            size="sm"
            onClick={handleCheckUpdates}
            disabled={checkingUpdates}
          >
            <ExternalLink className="w-4 h-4 mr-1.5" />
            {t('settings.general.checkUpdatesButton', 'Check')}
          </Button>
        </SettingsCard>
      </SettingsSection>

      {/* Logs Section */}
      <SettingsSection header={t('settings.general.logsSection', 'Logs')}>
        <SettingsCard
          title={t('settings.general.viewLogs', 'View Logs')}
          description={t(
            'settings.general.viewLogsDescription',
            'View application logs for debugging'
          )}
          icon={FileText}
        >
          <Dialog>
            <DialogTrigger asChild>
              <Button variant="outline" size="sm" onClick={handleLoadLogs}>
                {t('settings.general.viewLogsButton', 'View')}
              </Button>
            </DialogTrigger>
            <DialogContent className="max-w-3xl max-h-[80vh]">
              <DialogHeader>
                <DialogTitle>
                  {t('settings.general.logsTitle', 'Application Logs')}
                </DialogTitle>
              </DialogHeader>
              <div className="bg-muted rounded-md p-md max-h-[60vh] overflow-auto">
                {logsLoading ? (
                  <p className="text-caption text-muted-foreground">
                    {t('common.loading')}
                  </p>
                ) : (
                  <pre className="text-code text-caption whitespace-pre-wrap font-mono">
                    {logs.join('\n') || 'No logs available'}
                  </pre>
                )}
              </div>
            </DialogContent>
          </Dialog>
        </SettingsCard>
      </SettingsSection>

      {/* About Section */}
      <SettingsSection header={t('settings.general.aboutSection', 'About')}>
        <SettingsCard
          title={t('settings.general.version')}
          description={t('settings.general.versionDescription')}
          icon={Info}
        >
          <span className="text-body text-muted-foreground font-mono">
            {version ? `${version}${buildNumber ? ` (${buildNumber})` : ''}` : t('common.loading')}
          </span>
        </SettingsCard>
      </SettingsSection>
    </div>
  );
}

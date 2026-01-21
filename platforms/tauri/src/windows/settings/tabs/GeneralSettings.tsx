import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { Switch } from '@/components/ui/switch';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { commands } from '@/lib/commands';
import { changeLanguage, supportedLanguages } from '@/lib/i18n';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

export function GeneralSettings() {
  const { t } = useTranslation();
  const general = useSettingsStore((s) => s.general);
  const updateGeneral = useSettingsStore((s) => s.updateGeneral);
  const [version, setVersion] = useState('');
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [autostartLoading, setAutostartLoading] = useState(false);

  useEffect(() => {
    commands.getAppVersion().then((v) => setVersion(v.version));

    // Get actual autostart status from system
    commands.getAutostartEnabled().then((enabled) => {
      setAutostartEnabled(enabled);
      // Sync with settings if different
      if (enabled !== general.launch_at_login) {
        updateGeneral({ launch_at_login: enabled });
      }
    }).catch(console.error);
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

  return (
    <div className="space-y-6 max-w-2xl">
      <div>
        <h1 className="text-title mb-1">{t('settings.general.title')}</h1>
        <p className="text-caption text-muted-foreground">
          {t('settings.general.description')}
        </p>
      </div>

      <div className="space-y-4">
        <SettingsCard
          title={t('settings.general.sound')}
          description={t('settings.general.soundDescription')}
        >
          <Switch
            checked={general.sound_enabled}
            onCheckedChange={(checked) => updateGeneral({ sound_enabled: checked })}
          />
        </SettingsCard>

        <SettingsCard
          title={t('settings.general.launchAtLogin')}
          description={t('settings.general.launchAtLoginDescription')}
        >
          <Switch
            checked={autostartEnabled}
            onCheckedChange={handleAutostartChange}
            disabled={autostartLoading}
          />
        </SettingsCard>

        <SettingsCard
          title={t('settings.general.language')}
          description={t('settings.general.languageDescription')}
        >
          <Select
            value={general.language}
            onValueChange={handleLanguageChange}
          >
            <SelectTrigger className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">{t('common.systemDefault')}</SelectItem>
              {supportedLanguages.map((lang) => (
                <SelectItem key={lang.code} value={lang.code}>
                  {lang.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </SettingsCard>

        <SettingsCard
          title={t('settings.general.version')}
          description={t('settings.general.versionDescription')}
        >
          <span className="text-body text-muted-foreground">
            {version || t('common.loading')}
          </span>
        </SettingsCard>
      </div>
    </div>
  );
}

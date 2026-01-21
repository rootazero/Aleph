import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { Switch } from '@/components/ui/switch';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select';
import { commands } from '@/lib/commands';
import { useEffect, useState } from 'react';

export function GeneralSettings() {
  const general = useSettingsStore((s) => s.general);
  const updateGeneral = useSettingsStore((s) => s.updateGeneral);
  const [version, setVersion] = useState('');

  useEffect(() => {
    commands.getAppVersion().then((v) => setVersion(v.version));
  }, []);

  return (
    <div className="space-y-6 max-w-2xl">
      <div>
        <h1 className="text-title mb-1">General</h1>
        <p className="text-caption text-muted-foreground">
          Basic application settings
        </p>
      </div>

      <div className="space-y-4">
        <SettingsCard
          title="Sound Effects"
          description="Play sound effects for actions"
        >
          <Switch
            checked={general.sound_enabled}
            onCheckedChange={(checked) => updateGeneral({ sound_enabled: checked })}
          />
        </SettingsCard>

        <SettingsCard
          title="Launch at Login"
          description="Automatically start Aether when you log in"
        >
          <Switch
            checked={general.launch_at_login}
            onCheckedChange={(checked) => updateGeneral({ launch_at_login: checked })}
          />
        </SettingsCard>

        <SettingsCard
          title="Language"
          description="Select your preferred language"
        >
          <Select
            value={general.language}
            onValueChange={(value) => updateGeneral({ language: value })}
          >
            <SelectTrigger className="w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="system">System Default</SelectItem>
              <SelectItem value="en">English</SelectItem>
              <SelectItem value="zh-CN">简体中文</SelectItem>
            </SelectContent>
          </Select>
        </SettingsCard>

        <SettingsCard
          title="Version"
          description="Current application version"
        >
          <span className="text-body text-muted-foreground">
            {version || 'Loading...'}
          </span>
        </SettingsCard>
      </div>
    </div>
  );
}

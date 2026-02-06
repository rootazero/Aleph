import { useState } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { SettingsSection } from '@/components/ui/settings-section';
import { InfoBox } from '@/components/ui/info-box';
import { Button } from '@/components/ui/button';
import { Keyboard, RotateCcw, Sparkles, Command, Mic, Camera } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';

interface ShortcutRecorderProps {
  value: string;
  onChange: (value: string) => void;
}

function ShortcutRecorder({ value, onChange }: ShortcutRecorderProps) {
  const { t } = useTranslation();
  const [isRecording, setIsRecording] = useState(false);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (!isRecording) return;
    e.preventDefault();

    const parts: string[] = [];
    if (e.ctrlKey) parts.push('Ctrl');
    if (e.altKey) parts.push('Alt');
    if (e.shiftKey) parts.push('Shift');
    if (e.metaKey) parts.push('Meta');

    // Only accept if a modifier is pressed with a key
    const key = e.key;
    if (key !== 'Control' && key !== 'Alt' && key !== 'Shift' && key !== 'Meta') {
      // Normalize key names
      let keyName = key;
      if (key === ' ') keyName = 'Space';
      else if (key.length === 1) keyName = key.toUpperCase();

      parts.push(keyName);

      if (parts.length >= 2) {
        onChange(parts.join('+'));
        setIsRecording(false);
      }
    }
  };

  return (
    <div className="flex items-center gap-sm">
      <button
        type="button"
        onClick={() => setIsRecording(true)}
        onBlur={() => setIsRecording(false)}
        onKeyDown={handleKeyDown}
        className={cn(
          'px-3 py-1.5 rounded-sm border text-body font-mono min-w-[140px] text-center transition-colors',
          isRecording
            ? 'border-primary bg-primary/10 text-primary'
            : 'border-border bg-secondary/50 text-foreground hover:bg-secondary'
        )}
      >
        {isRecording ? t('common.pressKeys', 'Press keys...') : value || '—'}
      </button>
      <Button
        variant="ghost"
        size="icon"
        className="h-8 w-8"
        onClick={() => onChange('')}
        title={t('common.clearShortcut', 'Clear shortcut')}
      >
        <RotateCcw className="h-4 w-4" />
      </Button>
    </div>
  );
}

export function ShortcutsSettings() {
  const { t } = useTranslation();
  const shortcuts = useSettingsStore((s) => s.shortcuts);
  const updateShortcuts = useSettingsStore((s) => s.updateShortcuts);

  return (
    <div className="space-y-lg max-w-2xl">
      {/* Page Header */}
      <div>
        <h1 className="text-title mb-1">{t('settings.shortcuts.title')}</h1>
        <p className="text-caption text-muted-foreground">
          {t('settings.shortcuts.description')}
        </p>
      </div>

      {/* Global Shortcuts */}
      <SettingsSection header={t('settings.shortcuts.globalSection', 'Global Shortcuts')}>
        <SettingsCard
          title={t('settings.shortcuts.showHalo', 'Show Halo')}
          description={t('settings.shortcuts.showHaloDescription', 'Open the Halo command window')}
          icon={Sparkles}
        >
          <ShortcutRecorder
            value={shortcuts.show_halo}
            onChange={(value) => updateShortcuts({ show_halo: value })}
          />
        </SettingsCard>

        <SettingsCard
          title={t('settings.shortcuts.commandCompletion', 'Command Completion')}
          description={t('settings.shortcuts.commandCompletionDescription', 'Trigger AI completion at cursor')}
          icon={Command}
        >
          <ShortcutRecorder
            value={shortcuts.command_completion}
            onChange={(value) => updateShortcuts({ command_completion: value })}
          />
        </SettingsCard>
      </SettingsSection>

      {/* Voice & Media */}
      <SettingsSection header={t('settings.shortcuts.voiceSection', 'Voice & Media')}>
        <SettingsCard
          title={t('settings.shortcuts.toggleListening', 'Toggle Listening')}
          description={t('settings.shortcuts.toggleListeningDescription', 'Start or stop voice input')}
          icon={Mic}
        >
          <ShortcutRecorder
            value={shortcuts.toggle_listening}
            onChange={(value) => updateShortcuts({ toggle_listening: value })}
          />
        </SettingsCard>

        <SettingsCard
          title={t('settings.shortcuts.quickCapture', 'Quick Capture')}
          description={t('settings.shortcuts.quickCaptureDescription', 'Capture screen selection')}
          icon={Camera}
        >
          <ShortcutRecorder
            value={shortcuts.quick_capture}
            onChange={(value) => updateShortcuts({ quick_capture: value })}
          />
        </SettingsCard>
      </SettingsSection>

      {/* Hint */}
      <InfoBox variant="info">
        <div className="flex items-center gap-sm">
          <Keyboard className="h-4 w-4" />
          <span>{t('settings.shortcuts.hint', 'Click on a shortcut field and press your desired key combination')}</span>
        </div>
      </InfoBox>
    </div>
  );
}

import { useState } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { Button } from '@/components/ui/button';
import { Keyboard, RotateCcw } from 'lucide-react';
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
    <div className="flex items-center gap-2">
      <button
        type="button"
        onClick={() => setIsRecording(true)}
        onBlur={() => setIsRecording(false)}
        onKeyDown={handleKeyDown}
        className={cn(
          'px-3 py-1.5 rounded-medium border text-body font-mono min-w-[140px] text-center transition-colors',
          isRecording
            ? 'border-primary bg-primary/10 text-primary'
            : 'border-border bg-secondary/50 text-foreground hover:bg-secondary'
        )}
      >
        {isRecording ? t('common.pressKeys') : value}
      </button>
      <Button
        variant="ghost"
        size="icon"
        className="h-8 w-8"
        onClick={() => onChange('')}
        title={t('common.clearShortcut')}
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

  const shortcutItems = [
    {
      key: 'show_halo' as const,
      titleKey: 'settings.shortcuts.showHalo',
      descriptionKey: 'settings.shortcuts.showHaloDescription',
    },
    {
      key: 'command_completion' as const,
      titleKey: 'settings.shortcuts.commandCompletion',
      descriptionKey: 'settings.shortcuts.commandCompletionDescription',
    },
    {
      key: 'toggle_listening' as const,
      titleKey: 'settings.shortcuts.toggleListening',
      descriptionKey: 'settings.shortcuts.toggleListeningDescription',
    },
    {
      key: 'quick_capture' as const,
      titleKey: 'settings.shortcuts.quickCapture',
      descriptionKey: 'settings.shortcuts.quickCaptureDescription',
    },
  ];

  return (
    <div className="space-y-6 max-w-2xl">
      <div>
        <h1 className="text-title mb-1">{t('settings.shortcuts.title')}</h1>
        <p className="text-caption text-muted-foreground">
          {t('settings.shortcuts.description')}
        </p>
      </div>

      <div className="space-y-4">
        {shortcutItems.map((item) => (
          <SettingsCard
            key={item.key}
            title={t(item.titleKey)}
            description={t(item.descriptionKey)}
          >
            <ShortcutRecorder
              value={shortcuts[item.key]}
              onChange={(value) => updateShortcuts({ [item.key]: value })}
            />
          </SettingsCard>
        ))}
      </div>

      <div className="pt-4 border-t border-border">
        <div className="flex items-center gap-2 text-caption text-muted-foreground">
          <Keyboard className="h-4 w-4" />
          <span>{t('settings.shortcuts.hint')}</span>
        </div>
      </div>
    </div>
  );
}

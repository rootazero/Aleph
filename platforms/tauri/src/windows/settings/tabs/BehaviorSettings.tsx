import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { X } from 'lucide-react';
import { useState } from 'react';

export function BehaviorSettings() {
  const behavior = useSettingsStore((s) => s.behavior);
  const updateBehavior = useSettingsStore((s) => s.updateBehavior);
  const [newKeyword, setNewKeyword] = useState('');

  const addKeyword = () => {
    if (newKeyword.trim() && !behavior.pii_keywords.includes(newKeyword.trim())) {
      updateBehavior({
        pii_keywords: [...behavior.pii_keywords, newKeyword.trim()],
      });
      setNewKeyword('');
    }
  };

  const removeKeyword = (keyword: string) => {
    updateBehavior({
      pii_keywords: behavior.pii_keywords.filter((k) => k !== keyword),
    });
  };

  return (
    <div className="space-y-6 max-w-2xl">
      <div>
        <h1 className="text-title mb-1">Behavior</h1>
        <p className="text-caption text-muted-foreground">
          Control how Aether responds and interacts
        </p>
      </div>

      {/* Output Settings */}
      <section className="space-y-4">
        <h2 className="text-body font-medium text-foreground">Output</h2>

        <SettingsCard
          title="Output Mode"
          description="How AI responses are inserted"
        >
          <Select
            value={behavior.output_mode}
            onValueChange={(value: 'replace' | 'append' | 'clipboard') =>
              updateBehavior({ output_mode: value })
            }
          >
            <SelectTrigger className="w-36">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="replace">Replace</SelectItem>
              <SelectItem value="append">Append</SelectItem>
              <SelectItem value="clipboard">Clipboard</SelectItem>
            </SelectContent>
          </Select>
        </SettingsCard>

        <SettingsCard
          title="Typing Speed"
          description="Speed of typewriter effect (0-100)"
        >
          <div className="flex items-center gap-3 w-48">
            <Slider
              value={[behavior.typing_speed]}
              onValueChange={([value]) => updateBehavior({ typing_speed: value })}
              min={0}
              max={100}
              step={5}
              className="flex-1"
            />
            <span className="text-caption text-muted-foreground w-8 text-right">
              {behavior.typing_speed}
            </span>
          </div>
        </SettingsCard>

        <SettingsCard
          title="Auto Dismiss"
          description="Seconds before success message dismisses"
        >
          <div className="flex items-center gap-3 w-48">
            <Slider
              value={[behavior.auto_dismiss_delay]}
              onValueChange={([value]) =>
                updateBehavior({ auto_dismiss_delay: value })
              }
              min={1}
              max={10}
              step={1}
              className="flex-1"
            />
            <span className="text-caption text-muted-foreground w-8 text-right">
              {behavior.auto_dismiss_delay}s
            </span>
          </div>
        </SettingsCard>
      </section>

      {/* Notifications */}
      <section className="space-y-4">
        <h2 className="text-body font-medium text-foreground">Notifications</h2>

        <SettingsCard
          title="Show Notifications"
          description="Display system notifications for important events"
        >
          <Switch
            checked={behavior.show_notifications}
            onCheckedChange={(checked) =>
              updateBehavior({ show_notifications: checked })
            }
          />
        </SettingsCard>
      </section>

      {/* Privacy */}
      <section className="space-y-4">
        <h2 className="text-body font-medium text-foreground">Privacy</h2>

        <SettingsCard
          title="PII Masking"
          description="Automatically mask sensitive information"
        >
          <Switch
            checked={behavior.pii_masking}
            onCheckedChange={(checked) => updateBehavior({ pii_masking: checked })}
          />
        </SettingsCard>

        {behavior.pii_masking && (
          <div className="p-4 rounded-card bg-card border border-border space-y-3">
            <div className="space-y-1">
              <label className="text-body font-medium text-foreground">
                PII Keywords
              </label>
              <p className="text-caption text-muted-foreground">
                Words to detect and mask in AI responses
              </p>
            </div>

            <div className="flex gap-2">
              <Input
                value={newKeyword}
                onChange={(e) => setNewKeyword(e.target.value)}
                placeholder="Add keyword..."
                className="flex-1"
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    addKeyword();
                  }
                }}
              />
              <button
                onClick={addKeyword}
                className="px-4 py-2 rounded-medium bg-primary text-primary-foreground text-body hover:bg-primary/90 transition-colors"
              >
                Add
              </button>
            </div>

            {behavior.pii_keywords.length > 0 && (
              <div className="flex flex-wrap gap-2">
                {behavior.pii_keywords.map((keyword) => (
                  <Badge
                    key={keyword}
                    variant="secondary"
                    className="flex items-center gap-1 pr-1"
                  >
                    {keyword}
                    <button
                      onClick={() => removeKeyword(keyword)}
                      className="ml-1 hover:bg-secondary rounded-full p-0.5"
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </Badge>
                ))}
              </div>
            )}
          </div>
        )}
      </section>
    </div>
  );
}

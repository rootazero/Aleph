import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import { Input } from '@/components/ui/input';
import { RotateCcw } from 'lucide-react';
import { Button } from '@/components/ui/button';

export function GenerationSettings() {
  const generation = useSettingsStore((s) => s.generation);
  const updateGeneration = useSettingsStore((s) => s.updateGeneration);

  const resetToDefaults = () => {
    updateGeneration({
      temperature: 0.7,
      max_tokens: 4096,
      top_p: 1.0,
      frequency_penalty: 0,
      presence_penalty: 0,
      streaming: true,
    });
  };

  return (
    <div className="space-y-6 max-w-2xl">
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-title mb-1">Generation</h1>
          <p className="text-caption text-muted-foreground">
            Configure AI generation parameters
          </p>
        </div>
        <Button variant="ghost" size="sm" onClick={resetToDefaults}>
          <RotateCcw className="h-4 w-4 mr-2" />
          Reset
        </Button>
      </div>

      <div className="space-y-4">
        <SettingsCard
          title="Streaming"
          description="Stream responses token by token"
        >
          <Switch
            checked={generation.streaming}
            onCheckedChange={(checked) => updateGeneration({ streaming: checked })}
          />
        </SettingsCard>

        <SettingsCard
          title="Temperature"
          description="Controls randomness (0 = deterministic, 2 = very random)"
        >
          <div className="flex items-center gap-3 w-48">
            <Slider
              value={[generation.temperature]}
              onValueChange={([value]) => updateGeneration({ temperature: value })}
              min={0}
              max={2}
              step={0.1}
              className="flex-1"
            />
            <span className="text-caption text-muted-foreground w-10 text-right font-mono">
              {generation.temperature.toFixed(1)}
            </span>
          </div>
        </SettingsCard>

        <SettingsCard
          title="Max Tokens"
          description="Maximum number of tokens to generate"
        >
          <Input
            type="number"
            value={generation.max_tokens}
            onChange={(e) =>
              updateGeneration({ max_tokens: parseInt(e.target.value) || 4096 })
            }
            min={1}
            max={128000}
            className="w-32 text-right font-mono"
          />
        </SettingsCard>

        <SettingsCard
          title="Top P"
          description="Nucleus sampling threshold"
        >
          <div className="flex items-center gap-3 w-48">
            <Slider
              value={[generation.top_p]}
              onValueChange={([value]) => updateGeneration({ top_p: value })}
              min={0}
              max={1}
              step={0.05}
              className="flex-1"
            />
            <span className="text-caption text-muted-foreground w-10 text-right font-mono">
              {generation.top_p.toFixed(2)}
            </span>
          </div>
        </SettingsCard>

        <SettingsCard
          title="Frequency Penalty"
          description="Penalize tokens based on frequency"
        >
          <div className="flex items-center gap-3 w-48">
            <Slider
              value={[generation.frequency_penalty]}
              onValueChange={([value]) =>
                updateGeneration({ frequency_penalty: value })
              }
              min={-2}
              max={2}
              step={0.1}
              className="flex-1"
            />
            <span className="text-caption text-muted-foreground w-10 text-right font-mono">
              {generation.frequency_penalty.toFixed(1)}
            </span>
          </div>
        </SettingsCard>

        <SettingsCard
          title="Presence Penalty"
          description="Penalize tokens based on presence"
        >
          <div className="flex items-center gap-3 w-48">
            <Slider
              value={[generation.presence_penalty]}
              onValueChange={([value]) =>
                updateGeneration({ presence_penalty: value })
              }
              min={-2}
              max={2}
              step={0.1}
              className="flex-1"
            />
            <span className="text-caption text-muted-foreground w-10 text-right font-mono">
              {generation.presence_penalty.toFixed(1)}
            </span>
          </div>
        </SettingsCard>
      </div>
    </div>
  );
}

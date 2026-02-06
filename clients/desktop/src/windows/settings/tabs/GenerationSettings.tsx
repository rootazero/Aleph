import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { SettingsSection } from '@/components/ui/settings-section';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import { Input } from '@/components/ui/input';
import { RotateCcw, Zap, Thermometer, Hash, Target, Repeat, UserPlus } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { useTranslation } from 'react-i18next';

export function GenerationSettings() {
  const { t } = useTranslation();
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
    <div className="space-y-lg max-w-2xl">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-title mb-1">{t('settings.generation.title', 'Generation')}</h1>
          <p className="text-caption text-muted-foreground">
            {t('settings.generation.description', 'Configure AI generation parameters')}
          </p>
        </div>
        <Button variant="outline" size="sm" onClick={resetToDefaults}>
          <RotateCcw className="h-4 w-4 mr-2" />
          {t('common.reset', 'Reset')}
        </Button>
      </div>

      {/* Output */}
      <SettingsSection header={t('settings.generation.outputSection', 'Output')}>
        <SettingsCard
          title={t('settings.generation.streaming', 'Streaming')}
          description={t('settings.generation.streamingDescription', 'Stream responses token by token')}
          icon={Zap}
        >
          <Switch
            checked={generation.streaming}
            onCheckedChange={(checked) => updateGeneration({ streaming: checked })}
          />
        </SettingsCard>

        <SettingsCard
          title={t('settings.generation.maxTokens', 'Max Tokens')}
          description={t('settings.generation.maxTokensDescription', 'Maximum number of tokens to generate')}
          icon={Hash}
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
      </SettingsSection>

      {/* Sampling */}
      <SettingsSection header={t('settings.generation.samplingSection', 'Sampling')}>
        <SettingsCard
          title={t('settings.generation.temperature', 'Temperature')}
          description={t('settings.generation.temperatureDescription', 'Controls randomness (0 = deterministic, 2 = very random)')}
          icon={Thermometer}
        >
          <div className="flex items-center gap-md w-48">
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
          title={t('settings.generation.topP', 'Top P')}
          description={t('settings.generation.topPDescription', 'Nucleus sampling threshold')}
          icon={Target}
        >
          <div className="flex items-center gap-md w-48">
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
      </SettingsSection>

      {/* Penalties */}
      <SettingsSection header={t('settings.generation.penaltiesSection', 'Penalties')}>
        <SettingsCard
          title={t('settings.generation.frequencyPenalty', 'Frequency Penalty')}
          description={t('settings.generation.frequencyPenaltyDescription', 'Penalize tokens based on frequency')}
          icon={Repeat}
        >
          <div className="flex items-center gap-md w-48">
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
          title={t('settings.generation.presencePenalty', 'Presence Penalty')}
          description={t('settings.generation.presencePenaltyDescription', 'Penalize tokens based on presence')}
          icon={UserPlus}
        >
          <div className="flex items-center gap-md w-48">
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
      </SettingsSection>
    </div>
  );
}

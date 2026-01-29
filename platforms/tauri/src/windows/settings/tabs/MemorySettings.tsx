import { useState, useEffect, useCallback } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { useGatewayStore, gateway } from '@/stores/gatewayStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { SettingsSection } from '@/components/ui/settings-section';
import { InfoBox } from '@/components/ui/info-box';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Button } from '@/components/ui/button';
import { Trash2, Brain, Database, Save, History, Gauge, Loader2, HardDrive, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

interface MemoryStats {
  count: number;
  size_bytes: number;
}

export function MemorySettings() {
  const { t } = useTranslation();
  const memory = useSettingsStore((s) => s.memory);
  const updateMemory = useSettingsStore((s) => s.updateMemory);
  const isConnected = useGatewayStore((s) => s.isConnected);

  const [stats, setStats] = useState<MemoryStats | null>(null);
  const [isLoadingStats, setIsLoadingStats] = useState(false);
  const [isClearing, setIsClearing] = useState(false);

  const loadStats = useCallback(async () => {
    if (!isConnected()) {
      setStats(null);
      return;
    }

    setIsLoadingStats(true);
    try {
      const result = await gateway.memoryStats();
      setStats(result);
    } catch (e) {
      console.error('Failed to load memory stats:', e);
      setStats(null);
    } finally {
      setIsLoadingStats(false);
    }
  }, [isConnected]);

  useEffect(() => {
    loadStats();
  }, [loadStats]);

  const handleClearMemory = async () => {
    if (!isConnected()) {
      console.log('Gateway not connected, cannot clear memory');
      return;
    }

    setIsClearing(true);
    try {
      await gateway.memoryClear();
      await loadStats();
    } catch (e) {
      console.error('Failed to clear memory:', e);
    } finally {
      setIsClearing(false);
    }
  };

  const formatBytes = (bytes: number): string => {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
  };

  return (
    <div className="space-y-lg max-w-2xl">
      {/* Page Header */}
      <div>
        <h1 className="text-title mb-1">{t('settings.memory.title', 'Memory')}</h1>
        <p className="text-caption text-muted-foreground">
          {t('settings.memory.description', 'Configure conversation memory and context retention')}
        </p>
      </div>

      {/* Core Settings */}
      <SettingsSection header={t('settings.memory.coreSection', 'Core Settings')}>
        <SettingsCard
          title={t('settings.memory.enabled', 'Enable Memory')}
          description={t('settings.memory.enabledDescription', 'Remember context from previous conversations')}
          icon={Brain}
        >
          <Switch
            checked={memory.enabled}
            onCheckedChange={(checked) => updateMemory({ enabled: checked })}
          />
        </SettingsCard>

        {memory.enabled && (
          <>
            <SettingsCard
              title={t('settings.memory.autoSave', 'Auto Save')}
              description={t('settings.memory.autoSaveDescription', 'Automatically save important context')}
              icon={Save}
            >
              <Switch
                checked={memory.auto_save}
                onCheckedChange={(checked) => updateMemory({ auto_save: checked })}
              />
            </SettingsCard>

            <SettingsCard
              title={t('settings.memory.maxHistory', 'Max History')}
              description={t('settings.memory.maxHistoryDescription', 'Number of conversations to remember')}
              icon={History}
            >
              <div className="flex items-center gap-md w-48">
                <Slider
                  value={[memory.max_history]}
                  onValueChange={([value]) => updateMemory({ max_history: value })}
                  min={10}
                  max={500}
                  step={10}
                  className="flex-1"
                />
                <span className="text-caption text-muted-foreground w-10 text-right font-mono">
                  {memory.max_history}
                </span>
              </div>
            </SettingsCard>
          </>
        )}
      </SettingsSection>

      {/* Embedding Settings */}
      {memory.enabled && (
        <SettingsSection header={t('settings.memory.embeddingsSection', 'Embeddings')}>
          <SettingsCard
            title={t('settings.memory.embeddingModel', 'Embedding Model')}
            description={t('settings.memory.embeddingModelDescription', 'Model used for semantic search')}
            icon={Database}
          >
            <Select
              value={memory.embedding_model}
              onValueChange={(value) => updateMemory({ embedding_model: value })}
            >
              <SelectTrigger className="w-52">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="text-embedding-3-small">
                  text-embedding-3-small
                </SelectItem>
                <SelectItem value="text-embedding-3-large">
                  text-embedding-3-large
                </SelectItem>
                <SelectItem value="text-embedding-ada-002">
                  text-embedding-ada-002
                </SelectItem>
              </SelectContent>
            </Select>
          </SettingsCard>

          <SettingsCard
            title={t('settings.memory.similarityThreshold', 'Similarity Threshold')}
            description={t('settings.memory.similarityThresholdDescription', 'Minimum similarity score for memory retrieval')}
            icon={Gauge}
          >
            <div className="flex items-center gap-md w-48">
              <Slider
                value={[memory.similarity_threshold]}
                onValueChange={([value]) =>
                  updateMemory({ similarity_threshold: value })
                }
                min={0.1}
                max={1}
                step={0.05}
                className="flex-1"
              />
              <span className="text-caption text-muted-foreground w-10 text-right font-mono">
                {memory.similarity_threshold.toFixed(2)}
              </span>
            </div>
          </SettingsCard>
        </SettingsSection>
      )}

      {/* Memory Stats */}
      {isConnected() && (
        <SettingsSection header={t('settings.memory.statsSection', 'Storage')}>
          <div className="p-md rounded-md border border-border bg-muted/30">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <div className="w-10 h-10 rounded-medium bg-primary/10 flex items-center justify-center">
                  <HardDrive className="h-5 w-5 text-primary" />
                </div>
                <div>
                  <p className="text-body font-medium text-foreground">
                    {t('settings.memory.storageUsage', 'Memory Storage')}
                  </p>
                  {isLoadingStats ? (
                    <p className="text-caption text-muted-foreground flex items-center gap-1">
                      <Loader2 className="h-3 w-3 animate-spin" />
                      {t('common.loading', 'Loading...')}
                    </p>
                  ) : stats ? (
                    <p className="text-caption text-muted-foreground">
                      {t('settings.memory.statsInfo', '{{count}} memories · {{size}}', {
                        count: stats.count,
                        size: formatBytes(stats.size_bytes),
                      })}
                    </p>
                  ) : (
                    <p className="text-caption text-muted-foreground">
                      {t('settings.memory.noStats', 'Unable to load stats')}
                    </p>
                  )}
                </div>
              </div>
              <Button
                variant="outline"
                size="icon"
                onClick={loadStats}
                disabled={isLoadingStats}
              >
                <RefreshCw className={`h-4 w-4 ${isLoadingStats ? 'animate-spin' : ''}`} />
              </Button>
            </div>
          </div>
        </SettingsSection>
      )}

      {/* Danger Zone */}
      <SettingsSection header={t('settings.memory.dangerZone', 'Danger Zone')}>
        <div className="p-md rounded-md border border-destructive/50 bg-destructive/5">
          <div className="flex items-center justify-between">
            <div>
              <p className="text-body font-medium text-foreground">
                {t('settings.memory.clearAll', 'Clear All Memory')}
              </p>
              <p className="text-caption text-muted-foreground">
                {t('settings.memory.clearAllDescription', 'Permanently delete all stored memories. This cannot be undone.')}
              </p>
            </div>
            <Button
              variant="destructive"
              size="sm"
              onClick={handleClearMemory}
              disabled={isClearing || !isConnected()}
            >
              {isClearing ? (
                <>
                  <Loader2 className="h-4 w-4 mr-2 animate-spin" />
                  {t('common.clearing', 'Clearing...')}
                </>
              ) : (
                <>
                  <Trash2 className="h-4 w-4 mr-2" />
                  {t('common.clear', 'Clear')}
                </>
              )}
            </Button>
          </div>
        </div>

        <InfoBox variant="warning">
          {t('settings.memory.clearWarning', 'Clearing memory will remove all saved context and conversation history. This action is irreversible.')}
        </InfoBox>
      </SettingsSection>
    </div>
  );
}

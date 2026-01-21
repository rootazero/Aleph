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
import { Button } from '@/components/ui/button';
import { Trash2, Brain, Database } from 'lucide-react';

export function MemorySettings() {
  const memory = useSettingsStore((s) => s.memory);
  const updateMemory = useSettingsStore((s) => s.updateMemory);

  const handleClearMemory = () => {
    // TODO: Implement clear memory
    console.log('Clear memory');
  };

  return (
    <div className="space-y-6 max-w-2xl">
      <div>
        <h1 className="text-title mb-1">Memory</h1>
        <p className="text-caption text-muted-foreground">
          Configure conversation memory and context retention
        </p>
      </div>

      {/* Core Settings */}
      <section className="space-y-4">
        <SettingsCard
          title="Enable Memory"
          description="Remember context from previous conversations"
        >
          <div className="flex items-center gap-2">
            <Brain className="h-4 w-4 text-muted-foreground" />
            <Switch
              checked={memory.enabled}
              onCheckedChange={(checked) => updateMemory({ enabled: checked })}
            />
          </div>
        </SettingsCard>

        {memory.enabled && (
          <>
            <SettingsCard
              title="Auto Save"
              description="Automatically save important context"
            >
              <Switch
                checked={memory.auto_save}
                onCheckedChange={(checked) => updateMemory({ auto_save: checked })}
              />
            </SettingsCard>

            <SettingsCard
              title="Max History"
              description="Number of conversations to remember"
            >
              <div className="flex items-center gap-3 w-48">
                <Slider
                  value={[memory.max_history]}
                  onValueChange={([value]) => updateMemory({ max_history: value })}
                  min={10}
                  max={500}
                  step={10}
                  className="flex-1"
                />
                <span className="text-caption text-muted-foreground w-10 text-right">
                  {memory.max_history}
                </span>
              </div>
            </SettingsCard>
          </>
        )}
      </section>

      {/* Embedding Settings */}
      {memory.enabled && (
        <section className="space-y-4">
          <h2 className="text-body font-medium text-foreground flex items-center gap-2">
            <Database className="h-4 w-4" />
            Embeddings
          </h2>

          <SettingsCard
            title="Embedding Model"
            description="Model used for semantic search"
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
            title="Similarity Threshold"
            description="Minimum similarity score for memory retrieval"
          >
            <div className="flex items-center gap-3 w-48">
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
        </section>
      )}

      {/* Danger Zone */}
      <section className="space-y-4 pt-4 border-t border-border">
        <h2 className="text-body font-medium text-destructive">Danger Zone</h2>

        <div className="p-4 rounded-card border border-destructive/50 bg-destructive/5">
          <div className="flex items-center justify-between">
            <div>
              <p className="text-body font-medium text-foreground">
                Clear All Memory
              </p>
              <p className="text-caption text-muted-foreground">
                Permanently delete all stored memories. This cannot be undone.
              </p>
            </div>
            <Button
              variant="destructive"
              size="sm"
              onClick={handleClearMemory}
            >
              <Trash2 className="h-4 w-4 mr-2" />
              Clear
            </Button>
          </div>
        </div>
      </section>
    </div>
  );
}

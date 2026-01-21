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
import { Globe, Shield } from 'lucide-react';

export function SearchSettings() {
  const search = useSettingsStore((s) => s.search);
  const updateSearch = useSettingsStore((s) => s.updateSearch);

  return (
    <div className="space-y-6 max-w-2xl">
      <div>
        <h1 className="text-title mb-1">Search</h1>
        <p className="text-caption text-muted-foreground">
          Configure web search capabilities
        </p>
      </div>

      <div className="space-y-4">
        <SettingsCard
          title="Web Search"
          description="Enable AI to search the web for information"
        >
          <div className="flex items-center gap-2">
            <Globe className="h-4 w-4 text-muted-foreground" />
            <Switch
              checked={search.web_search_enabled}
              onCheckedChange={(checked) =>
                updateSearch({ web_search_enabled: checked })
              }
            />
          </div>
        </SettingsCard>

        {search.web_search_enabled && (
          <>
            <SettingsCard
              title="Search Engine"
              description="Default search engine for web queries"
            >
              <Select
                value={search.search_engine}
                onValueChange={(value: 'google' | 'bing' | 'duckduckgo') =>
                  updateSearch({ search_engine: value })
                }
              >
                <SelectTrigger className="w-40">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="duckduckgo">DuckDuckGo</SelectItem>
                  <SelectItem value="google">Google</SelectItem>
                  <SelectItem value="bing">Bing</SelectItem>
                </SelectContent>
              </Select>
            </SettingsCard>

            <SettingsCard
              title="Max Results"
              description="Maximum search results to retrieve"
            >
              <div className="flex items-center gap-3 w-48">
                <Slider
                  value={[search.max_results]}
                  onValueChange={([value]) => updateSearch({ max_results: value })}
                  min={1}
                  max={20}
                  step={1}
                  className="flex-1"
                />
                <span className="text-caption text-muted-foreground w-8 text-right">
                  {search.max_results}
                </span>
              </div>
            </SettingsCard>

            <SettingsCard
              title="Safe Search"
              description="Filter explicit content from results"
            >
              <div className="flex items-center gap-2">
                <Shield className="h-4 w-4 text-muted-foreground" />
                <Switch
                  checked={search.safe_search}
                  onCheckedChange={(checked) =>
                    updateSearch({ safe_search: checked })
                  }
                />
              </div>
            </SettingsCard>
          </>
        )}
      </div>
    </div>
  );
}

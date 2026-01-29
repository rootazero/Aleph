import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { SettingsSection } from '@/components/ui/settings-section';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Globe, Shield, Search, Hash } from 'lucide-react';
import { useTranslation } from 'react-i18next';

export function SearchSettings() {
  const { t } = useTranslation();
  const search = useSettingsStore((s) => s.search);
  const updateSearch = useSettingsStore((s) => s.updateSearch);

  return (
    <div className="space-y-lg max-w-2xl">
      {/* Page Header */}
      <div>
        <h1 className="text-title mb-1">{t('settings.search.title', 'Search')}</h1>
        <p className="text-caption text-muted-foreground">
          {t('settings.search.description', 'Configure web search capabilities')}
        </p>
      </div>

      {/* Web Search */}
      <SettingsSection header={t('settings.search.webSearchSection', 'Web Search')}>
        <SettingsCard
          title={t('settings.search.webSearch', 'Web Search')}
          description={t('settings.search.webSearchDescription', 'Enable AI to search the web for information')}
          icon={Globe}
        >
          <Switch
            checked={search.web_search_enabled}
            onCheckedChange={(checked) =>
              updateSearch({ web_search_enabled: checked })
            }
          />
        </SettingsCard>

        {search.web_search_enabled && (
          <>
            <SettingsCard
              title={t('settings.search.searchEngine', 'Search Engine')}
              description={t('settings.search.searchEngineDescription', 'Default search engine for web queries')}
              icon={Search}
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
              title={t('settings.search.maxResults', 'Max Results')}
              description={t('settings.search.maxResultsDescription', 'Maximum search results to retrieve')}
              icon={Hash}
            >
              <div className="flex items-center gap-md w-48">
                <Slider
                  value={[search.max_results]}
                  onValueChange={([value]) => updateSearch({ max_results: value })}
                  min={1}
                  max={20}
                  step={1}
                  className="flex-1"
                />
                <span className="text-caption text-muted-foreground w-8 text-right font-mono">
                  {search.max_results}
                </span>
              </div>
            </SettingsCard>
          </>
        )}
      </SettingsSection>

      {/* Safety */}
      {search.web_search_enabled && (
        <SettingsSection header={t('settings.search.safetySection', 'Safety')}>
          <SettingsCard
            title={t('settings.search.safeSearch', 'Safe Search')}
            description={t('settings.search.safeSearchDescription', 'Filter explicit content from results')}
            icon={Shield}
          >
            <Switch
              checked={search.safe_search}
              onCheckedChange={(checked) =>
                updateSearch({ safe_search: checked })
              }
            />
          </SettingsCard>
        </SettingsSection>
      )}
    </div>
  );
}

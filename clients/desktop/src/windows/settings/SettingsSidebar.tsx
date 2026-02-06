import {
  Settings,
  Keyboard,
  Sliders,
  Bot,
  Plug,
  Wrench,
  Shield,
  Cpu,
  Search,
  Brain,
  Palette,
  Sparkles,
  Image,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';

export type SettingsTab =
  | 'general'
  | 'shortcuts'
  | 'behavior'
  | 'providers'
  | 'generation'
  | 'generationProviders'
  | 'memory'
  | 'search'
  | 'mcp'
  | 'skills'
  | 'plugins'
  | 'agent'
  | 'policies';

interface TabConfig {
  id: SettingsTab;
  labelKey: string;
  icon: React.ComponentType<{ className?: string }>;
}

interface TabGroupConfig {
  labelKey: string;
  tabs: TabConfig[];
}

const tabGroupsConfig: TabGroupConfig[] = [
  {
    labelKey: 'settings.groups.basic',
    tabs: [
      { id: 'general', labelKey: 'settings.general.title', icon: Settings },
      { id: 'shortcuts', labelKey: 'settings.shortcuts.title', icon: Keyboard },
      { id: 'behavior', labelKey: 'settings.behavior.title', icon: Sliders },
    ],
  },
  {
    labelKey: 'settings.groups.ai',
    tabs: [
      { id: 'providers', labelKey: 'settings.providers.title', icon: Cpu },
      { id: 'generationProviders', labelKey: 'settings.generationProviders.title', icon: Image },
      { id: 'generation', labelKey: 'settings.generation.title', icon: Palette },
      { id: 'memory', labelKey: 'settings.memory.title', icon: Brain },
    ],
  },
  {
    labelKey: 'settings.groups.extensions',
    tabs: [
      { id: 'mcp', labelKey: 'settings.mcp.title', icon: Wrench },
      { id: 'plugins', labelKey: 'settings.plugins.title', icon: Plug },
      { id: 'skills', labelKey: 'settings.skills.title', icon: Sparkles },
    ],
  },
  {
    labelKey: 'settings.groups.advanced',
    tabs: [
      { id: 'agent', labelKey: 'settings.agent.title', icon: Bot },
      { id: 'search', labelKey: 'settings.search.title', icon: Search },
      { id: 'policies', labelKey: 'settings.policies.title', icon: Shield },
    ],
  },
];

interface SettingsSidebarProps {
  activeTab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
}

export function SettingsSidebar({ activeTab, onTabChange }: SettingsSidebarProps) {
  const { t } = useTranslation();

  return (
    <nav className="space-y-4">
      {tabGroupsConfig.map((group) => (
        <div key={group.labelKey}>
          <h3 className="px-3 py-1 text-caption font-medium text-muted-foreground">
            {t(group.labelKey)}
          </h3>
          <div className="space-y-0.5">
            {group.tabs.map((tab) => {
              const Icon = tab.icon;
              const isActive = activeTab === tab.id;

              return (
                <button
                  key={tab.id}
                  onClick={() => onTabChange(tab.id)}
                  className={cn(
                    'w-full flex items-center gap-2 px-3 py-2 rounded-medium text-body transition-colors',
                    isActive
                      ? 'bg-accent text-accent-foreground'
                      : 'text-foreground hover:bg-accent/50'
                  )}
                >
                  <Icon className="w-4 h-4" />
                  <span>{t(tab.labelKey)}</span>
                </button>
              );
            })}
          </div>
        </div>
      ))}
    </nav>
  );
}

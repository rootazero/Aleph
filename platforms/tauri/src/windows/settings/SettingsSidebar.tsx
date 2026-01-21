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
} from 'lucide-react';
import { cn } from '@/lib/utils';

export type SettingsTab =
  | 'general'
  | 'shortcuts'
  | 'behavior'
  | 'providers'
  | 'generation'
  | 'memory'
  | 'search'
  | 'mcp'
  | 'skills'
  | 'plugins'
  | 'agent'
  | 'policies';

interface TabGroup {
  label: string;
  tabs: {
    id: SettingsTab;
    label: string;
    icon: React.ComponentType<{ className?: string }>;
  }[];
}

const tabGroups: TabGroup[] = [
  {
    label: 'Basic',
    tabs: [
      { id: 'general', label: 'General', icon: Settings },
      { id: 'shortcuts', label: 'Shortcuts', icon: Keyboard },
      { id: 'behavior', label: 'Behavior', icon: Sliders },
    ],
  },
  {
    label: 'AI',
    tabs: [
      { id: 'providers', label: 'Providers', icon: Cpu },
      { id: 'generation', label: 'Generation', icon: Palette },
      { id: 'memory', label: 'Memory', icon: Brain },
    ],
  },
  {
    label: 'Extensions',
    tabs: [
      { id: 'mcp', label: 'MCP', icon: Wrench },
      { id: 'plugins', label: 'Plugins', icon: Plug },
      { id: 'skills', label: 'Skills', icon: Sparkles },
    ],
  },
  {
    label: 'Advanced',
    tabs: [
      { id: 'agent', label: 'Agent', icon: Bot },
      { id: 'search', label: 'Search', icon: Search },
      { id: 'policies', label: 'Policies', icon: Shield },
    ],
  },
];

interface SettingsSidebarProps {
  activeTab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
}

export function SettingsSidebar({ activeTab, onTabChange }: SettingsSidebarProps) {
  return (
    <nav className="space-y-4">
      {tabGroups.map((group) => (
        <div key={group.label}>
          <h3 className="px-3 py-1 text-caption font-medium text-muted-foreground">
            {group.label}
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
                  <span>{tab.label}</span>
                </button>
              );
            })}
          </div>
        </div>
      ))}
    </nav>
  );
}

import { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useSettingsStore } from '@/stores/settingsStore';
import { useTheme } from '@/hooks/useTheme';
import { SettingsSidebar, type SettingsTab } from './SettingsSidebar';
import { SaveBar } from '@/components/ui/save-bar';

// Import all settings tabs
import { GeneralSettings } from './tabs/GeneralSettings';
import { ShortcutsSettings } from './tabs/ShortcutsSettings';
import { BehaviorSettings } from './tabs/BehaviorSettings';
import { ProvidersSettings } from './tabs/ProvidersSettings';
import { GenerationSettings } from './tabs/GenerationSettings';
import { GenerationProvidersSettings } from './tabs/GenerationProvidersSettings';
import { MemorySettings } from './tabs/MemorySettings';
import { McpSettings } from './tabs/McpSettings';
import { PluginsSettings } from './tabs/PluginsSettings';
import { SkillsSettings } from './tabs/SkillsSettings';
import { AgentSettings } from './tabs/AgentSettings';
import { SearchSettings } from './tabs/SearchSettings';
import { PoliciesSettings } from './tabs/PoliciesSettings';

export function SettingsWindow() {
  const [activeTab, setActiveTab] = useState<SettingsTab>('general');
  const { load, save, discard, isDirty, isLoading } = useSettingsStore();

  // Initialize theme
  useTheme();

  // Load settings on mount
  useEffect(() => {
    load();
  }, [load]);

  const handleSave = async () => {
    try {
      await save();
    } catch (error) {
      console.error('Failed to save settings:', error);
    }
  };

  const renderContent = () => {
    switch (activeTab) {
      case 'general':
        return <GeneralSettings />;
      case 'shortcuts':
        return <ShortcutsSettings />;
      case 'behavior':
        return <BehaviorSettings />;
      case 'providers':
        return <ProvidersSettings />;
      case 'generationProviders':
        return <GenerationProvidersSettings />;
      case 'generation':
        return <GenerationSettings />;
      case 'memory':
        return <MemorySettings />;
      case 'mcp':
        return <McpSettings />;
      case 'plugins':
        return <PluginsSettings />;
      case 'skills':
        return <SkillsSettings />;
      case 'agent':
        return <AgentSettings />;
      case 'search':
        return <SearchSettings />;
      case 'policies':
        return <PoliciesSettings />;
      default:
        return <GeneralSettings />;
    }
  };

  if (isLoading) {
    return (
      <div className="flex h-screen items-center justify-center bg-background">
        <div className="text-muted-foreground">Loading settings...</div>
      </div>
    );
  }

  return (
    <div className="flex h-screen bg-background">
      {/* Sidebar */}
      <aside className="w-52 border-r bg-muted/30 p-2 flex-shrink-0">
        <SettingsSidebar activeTab={activeTab} onTabChange={setActiveTab} />
      </aside>

      {/* Main content */}
      <main className="flex-1 flex flex-col min-w-0">
        <div className="flex-1 overflow-y-auto p-6">
          <AnimatePresence mode="wait">
            <motion.div
              key={activeTab}
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={{ duration: 0.15 }}
            >
              {renderContent()}
            </motion.div>
          </AnimatePresence>
        </div>

        {/* Save bar */}
        <AnimatePresence>
          {isDirty && <SaveBar onSave={handleSave} onDiscard={discard} />}
        </AnimatePresence>
      </main>
    </div>
  );
}

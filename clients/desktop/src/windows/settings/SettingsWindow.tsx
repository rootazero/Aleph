import { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useSettingsStore } from '@/stores/settingsStore';
import { useGatewayStore } from '@/stores/gatewayStore';
import { useTheme } from '@/hooks/useTheme';
import { SettingsSidebar, type SettingsTab } from './SettingsSidebar';
import { SaveBar } from '@/components/ui/save-bar';
import { GatewayStatus } from '@/components/ui/gateway-status';

// Import all settings tabs
import { GeneralSettings } from './tabs/GeneralSettings';
import { ShortcutsSettings } from './tabs/ShortcutsSettings';
import { BehaviorSettings } from './tabs/BehaviorSettings';
import { GenerationSettings } from './tabs/GenerationSettings';
import { AgentSettings } from './tabs/AgentSettings';
import { SearchSettings } from './tabs/SearchSettings';
import { MigratedToDashboard } from './tabs/MigratedToDashboard';

export function SettingsWindow() {
  const [activeTab, setActiveTab] = useState<SettingsTab>('general');
  const { load, save, discard, isDirty, isLoading } = useSettingsStore();
  const { connect: connectGateway, connectionState } = useGatewayStore();

  // Initialize theme
  useTheme();

  // Load settings on mount
  useEffect(() => {
    load();
  }, [load]);

  // Connect to Gateway on mount
  useEffect(() => {
    if (connectionState === 'disconnected') {
      connectGateway().catch((error) => {
        console.warn('[Settings] Gateway connection failed, using Tauri fallback:', error);
      });
    }
  }, [connectGateway, connectionState]);

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
        return <MigratedToDashboard featureName="Providers" dashboardPath="/settings/providers" />;
      case 'generationProviders':
        return <MigratedToDashboard featureName="Generation Providers" dashboardPath="/settings/generation-providers" />;
      case 'generation':
        return <GenerationSettings />;
      case 'memory':
        return <MigratedToDashboard featureName="Memory" dashboardPath="/settings/memory" />;
      case 'mcp':
        return <MigratedToDashboard featureName="MCP Plugins" dashboardPath="/settings/mcp" />;
      case 'plugins':
        return <MigratedToDashboard featureName="Plugins" dashboardPath="/settings/plugins" />;
      case 'skills':
        return <MigratedToDashboard featureName="Skills" dashboardPath="/settings/skills" />;
      case 'agent':
        return <AgentSettings />;
      case 'search':
        return <SearchSettings />;
      case 'policies':
        return <MigratedToDashboard featureName="Policies" dashboardPath="/settings/policies" />;
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
      <aside className="w-52 border-r bg-muted/30 p-2 flex-shrink-0 flex flex-col">
        <SettingsSidebar activeTab={activeTab} onTabChange={setActiveTab} />
        {/* Gateway Status */}
        <div className="mt-auto pt-2 px-2 pb-1 border-t border-border/50">
          <GatewayStatus showLabel autoConnect={false} />
        </div>
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

import { useState, useEffect } from 'react';
import { motion, AnimatePresence } from 'framer-motion';
import { useSettingsStore } from '@/stores/settingsStore';
import { useTheme } from '@/hooks/useTheme';
import { SettingsSidebar, type SettingsTab } from './SettingsSidebar';
import { GeneralSettings } from './tabs/GeneralSettings';
import { SaveBar } from '@/components/ui/save-bar';

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
        return <PlaceholderTab name="Shortcuts" />;
      case 'behavior':
        return <PlaceholderTab name="Behavior" />;
      case 'providers':
        return <PlaceholderTab name="Providers" />;
      case 'mcp':
        return <PlaceholderTab name="MCP Servers" />;
      case 'plugins':
        return <PlaceholderTab name="Plugins" />;
      case 'agent':
        return <PlaceholderTab name="Agent" />;
      default:
        return <PlaceholderTab name={activeTab} />;
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

// Placeholder for tabs not yet implemented
function PlaceholderTab({ name }: { name: string }) {
  return (
    <div className="flex items-center justify-center h-64 text-muted-foreground">
      <p>{name} settings coming soon...</p>
    </div>
  );
}

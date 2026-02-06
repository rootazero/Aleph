import { useState, useEffect, useCallback } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { useGatewayStore, gateway } from '@/stores/gatewayStore';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Badge } from '@/components/ui/badge';
import { SettingsSection } from '@/components/ui/settings-section';
import { InfoBox } from '@/components/ui/info-box';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Plus, Server, Trash2, Play, X, Info, RefreshCw, Loader2, AlertCircle } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';
import type { McpServer } from '@/lib/commands';

// Unified server type for both Gateway and local
interface UnifiedMcpServer {
  name: string;
  command: string;
  args: string[];
  env: Record<string, string>;
  enabled: boolean;
}

// Master-Detail Layout Component
function McpServerList({
  servers,
  selectedName,
  onSelect,
  onToggle,
  isToggling,
}: {
  servers: UnifiedMcpServer[];
  selectedName: string | null;
  onSelect: (name: string) => void;
  onToggle: (name: string) => void;
  isToggling?: string | null;
}) {
  return (
    <div className="space-y-2">
      {servers.map((server) => (
        <button
          key={server.name}
          onClick={() => onSelect(server.name)}
          className={cn(
            'w-full flex items-center justify-between p-3 rounded-card border text-left transition-colors',
            selectedName === server.name
              ? 'border-primary bg-accent'
              : 'border-border bg-card hover:bg-accent/50'
          )}
        >
          <div className="flex items-center gap-3 min-w-0">
            <Server
              className={cn(
                'h-5 w-5 flex-shrink-0',
                server.enabled ? 'text-success' : 'text-muted-foreground'
              )}
            />
            <div className="min-w-0">
              <p className="text-body font-medium text-foreground truncate">
                {server.name}
              </p>
              <p className="text-caption text-muted-foreground truncate">
                {server.command}
              </p>
            </div>
          </div>
          {isToggling === server.name ? (
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          ) : (
            <Switch
              checked={server.enabled}
              onCheckedChange={() => onToggle(server.name)}
              onClick={(e) => e.stopPropagation()}
            />
          )}
        </button>
      ))}
    </div>
  );
}

function McpServerDetail({
  server,
  onUpdate,
  onDelete,
  isDeleting,
}: {
  server: UnifiedMcpServer;
  onUpdate: (server: UnifiedMcpServer) => void;
  onDelete: () => void;
  isDeleting?: boolean;
}) {
  const [newArg, setNewArg] = useState('');
  const [newEnvKey, setNewEnvKey] = useState('');
  const [newEnvValue, setNewEnvValue] = useState('');

  const addArg = () => {
    if (newArg.trim()) {
      onUpdate({ ...server, args: [...server.args, newArg.trim()] });
      setNewArg('');
    }
  };

  const removeArg = (index: number) => {
    onUpdate({ ...server, args: server.args.filter((_, i) => i !== index) });
  };

  const addEnv = () => {
    if (newEnvKey.trim()) {
      onUpdate({
        ...server,
        env: { ...server.env, [newEnvKey.trim()]: newEnvValue },
      });
      setNewEnvKey('');
      setNewEnvValue('');
    }
  };

  const removeEnv = (key: string) => {
    const newEnv = { ...server.env };
    delete newEnv[key];
    onUpdate({ ...server, env: newEnv });
  };

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h3 className="text-body font-semibold text-foreground">{server.name}</h3>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="sm">
            <Play className="h-4 w-4 mr-1" />
            Test
          </Button>
          {isDeleting ? (
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          ) : (
            <Button
              variant="ghost"
              size="sm"
              onClick={onDelete}
              className="text-destructive hover:text-destructive"
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          )}
        </div>
      </div>

      {/* Command */}
      <div className="space-y-2">
        <label className="text-body font-medium">Command</label>
        <Input
          value={server.command}
          onChange={(e) => onUpdate({ ...server, command: e.target.value })}
          placeholder="npx -y @modelcontextprotocol/server-xxx"
          className="font-mono text-sm"
        />
      </div>

      {/* Arguments */}
      <div className="space-y-2">
        <label className="text-body font-medium">Arguments</label>
        <div className="flex gap-2">
          <Input
            value={newArg}
            onChange={(e) => setNewArg(e.target.value)}
            placeholder="Add argument..."
            className="font-mono text-sm"
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                addArg();
              }
            }}
          />
          <Button variant="secondary" size="icon" onClick={addArg}>
            <Plus className="h-4 w-4" />
          </Button>
        </div>
        {server.args.length > 0 && (
          <div className="flex flex-wrap gap-2 mt-2">
            {server.args.map((arg, index) => (
              <Badge
                key={index}
                variant="secondary"
                className="font-mono text-xs flex items-center gap-1"
              >
                {arg}
                <button onClick={() => removeArg(index)} className="ml-1">
                  <X className="h-3 w-3" />
                </button>
              </Badge>
            ))}
          </div>
        )}
      </div>

      {/* Environment Variables */}
      <div className="space-y-2">
        <label className="text-body font-medium">Environment Variables</label>
        <div className="flex gap-2">
          <Input
            value={newEnvKey}
            onChange={(e) => setNewEnvKey(e.target.value)}
            placeholder="KEY"
            className="font-mono text-sm flex-1"
          />
          <Input
            value={newEnvValue}
            onChange={(e) => setNewEnvValue(e.target.value)}
            placeholder="value"
            className="font-mono text-sm flex-1"
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                addEnv();
              }
            }}
          />
          <Button variant="secondary" size="icon" onClick={addEnv}>
            <Plus className="h-4 w-4" />
          </Button>
        </div>
        {Object.keys(server.env).length > 0 && (
          <div className="space-y-1 mt-2">
            {Object.entries(server.env).map(([key, value]) => (
              <div
                key={key}
                className="flex items-center justify-between p-2 rounded-small bg-secondary/50"
              >
                <span className="font-mono text-sm">
                  <span className="text-primary">{key}</span>
                  <span className="text-muted-foreground">=</span>
                  <span className="text-foreground">{value}</span>
                </span>
                <button
                  onClick={() => removeEnv(key)}
                  className="text-muted-foreground hover:text-destructive"
                >
                  <X className="h-4 w-4" />
                </button>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function AddServerDialog({
  open,
  onOpenChange,
  onAdd,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdd: (name: string, command: string) => Promise<void>;
}) {
  const [name, setName] = useState('');
  const [command, setCommand] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleAdd = async () => {
    if (!name.trim() || !command.trim()) return;

    setIsLoading(true);
    setError(null);

    try {
      await onAdd(name.trim(), command.trim());
      setName('');
      setCommand('');
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to add server');
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Add MCP Server</DialogTitle>
          <DialogDescription>
            Configure a new Model Context Protocol server
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <label className="text-body font-medium">Name</label>
            <Input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="My MCP Server"
            />
          </div>

          <div className="space-y-2">
            <label className="text-body font-medium">Command</label>
            <Input
              value={command}
              onChange={(e) => setCommand(e.target.value)}
              placeholder="npx -y @modelcontextprotocol/server-xxx"
              className="font-mono text-sm"
            />
          </div>

          {error && (
            <div className="flex items-center gap-2 text-destructive text-sm">
              <AlertCircle className="h-4 w-4" />
              {error}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={handleAdd} disabled={!name.trim() || !command.trim() || isLoading}>
            {isLoading ? (
              <>
                <RefreshCw className="h-4 w-4 mr-2 animate-spin" />
                Adding...
              </>
            ) : (
              'Add'
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function McpSettings() {
  const { t } = useTranslation();
  const localMcp = useSettingsStore((s) => s.mcp);
  const updateMcp = useSettingsStore((s) => s.updateMcp);
  const isConnected = useGatewayStore((s) => s.isConnected);

  // Gateway-loaded servers
  const [servers, setServers] = useState<UnifiedMcpServer[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [togglingName, setTogglingName] = useState<string | null>(null);
  const [deletingName, setDeletingName] = useState<string | null>(null);
  const [selectedName, setSelectedName] = useState<string | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);

  // Convert local McpServer to unified format
  const toUnifiedServer = (s: McpServer): UnifiedMcpServer => ({
    name: s.name,
    command: s.command,
    args: s.args,
    env: s.env,
    enabled: s.enabled,
  });

  // Load servers from Gateway
  const loadServers = useCallback(async () => {
    if (!isConnected()) {
      // Fallback to local settings
      const localServers = localMcp.servers.map(toUnifiedServer);
      setServers(localServers);
      if (localServers.length > 0 && !selectedName) {
        setSelectedName(localServers[0].name);
      }
      setIsLoading(false);
      return;
    }

    setIsLoading(true);
    setError(null);

    try {
      const result = await gateway.mcpListServers();
      // Gateway returns minimal info (name, enabled, url, transport)
      // For display, we map to unified format
      const unifiedServers: UnifiedMcpServer[] = result.map(s => ({
        name: s.name,
        command: s.url || s.transport || 'Unknown',
        args: [],
        env: {},
        enabled: s.enabled,
      }));
      setServers(unifiedServers);
      if (unifiedServers.length > 0 && !selectedName) {
        setSelectedName(unifiedServers[0].name);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load MCP servers');
      // Fallback to local
      const localServers = localMcp.servers.map(toUnifiedServer);
      setServers(localServers);
    } finally {
      setIsLoading(false);
    }
  }, [isConnected, localMcp.servers, selectedName]);

  useEffect(() => {
    loadServers();
  }, [loadServers]);

  const selectedServer = servers.find((s) => s.name === selectedName);

  const handleToggle = async (name: string) => {
    setTogglingName(name);

    try {
      const server = servers.find(s => s.name === name);
      if (!server) return;

      if (isConnected()) {
        if (server.enabled) {
          await gateway.mcpDisableServer(name);
        } else {
          await gateway.mcpEnableServer(name);
        }
        await loadServers();
      } else {
        // Fallback to local
        updateMcp({
          servers: localMcp.servers.map((s) =>
            s.name === name ? { ...s, enabled: !s.enabled } : s
          ),
        });
        setServers(prev => prev.map(s =>
          s.name === name ? { ...s, enabled: !s.enabled } : s
        ));
      }
    } catch (e) {
      console.error('Failed to toggle MCP server:', e);
    } finally {
      setTogglingName(null);
    }
  };

  const handleUpdate = (server: UnifiedMcpServer) => {
    // For now, updates only work locally since Gateway doesn't have an update method
    // Gateway uses add + remove for updates
    updateMcp({
      servers: localMcp.servers.map((s) => (s.name === server.name ? { ...s, ...server } : s)),
    });
    setServers(prev => prev.map(s => s.name === server.name ? server : s));
  };

  const handleDelete = async (name: string) => {
    setDeletingName(name);

    try {
      if (isConnected()) {
        await gateway.mcpRemoveServer(name);
        await loadServers();
      } else {
        updateMcp({
          servers: localMcp.servers.filter((s) => s.name !== name),
        });
        setServers(prev => prev.filter(s => s.name !== name));
      }
      // Select another server
      const remaining = servers.filter(s => s.name !== name);
      setSelectedName(remaining.length > 0 ? remaining[0].name : null);
    } catch (e) {
      console.error('Failed to delete MCP server:', e);
    } finally {
      setDeletingName(null);
    }
  };

  const handleAdd = async (name: string, command: string) => {
    if (isConnected()) {
      // Gateway uses GWMcpServerConfig with transport
      await gateway.mcpAddServer({
        name,
        enabled: true,
        transport: 'stdio',
        command,
        args: [],
        env: {},
      });
      await loadServers();
      setSelectedName(name);
    } else {
      // Fallback to local simulation
      const server = {
        id: crypto.randomUUID(),
        name,
        command,
        args: [],
        env: {},
        enabled: true,
      };
      updateMcp({
        servers: [...localMcp.servers, server],
      });
      setServers(prev => [...prev, {
        name: server.name,
        command: server.command,
        args: server.args,
        env: server.env,
        enabled: server.enabled,
      }]);
      setSelectedName(name);
    }
  };

  return (
    <div className="space-y-lg max-w-4xl">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-title mb-1">{t('settings.mcp.title', 'MCP Servers')}</h1>
          <p className="text-caption text-muted-foreground">
            {t('settings.mcp.description', 'Manage Model Context Protocol servers')}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="icon" onClick={loadServers} disabled={isLoading}>
            <RefreshCw className={cn("h-4 w-4", isLoading && "animate-spin")} />
          </Button>
          <Button onClick={() => setDialogOpen(true)}>
            <Plus className="h-4 w-4 mr-2" />
            {t('settings.mcp.addServer', 'Add Server')}
          </Button>
        </div>
      </div>

      {/* Error Message */}
      {error && (
        <InfoBox variant="error">
          <div className="flex items-center gap-2">
            <AlertCircle className="h-4 w-4" />
            <span>{error}</span>
          </div>
        </InfoBox>
      )}

      {/* Server Management */}
      <SettingsSection header={t('settings.mcp.serversSection', 'Servers')}>
        {isLoading ? (
          <div className="flex items-center justify-center py-12">
            <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
          </div>
        ) : servers.length === 0 ? (
          <div className="text-center py-12 text-muted-foreground border border-dashed border-border rounded-card">
            <Server className="h-12 w-12 mx-auto mb-4 opacity-50" />
            <p>{t('settings.mcp.noServers', 'No MCP servers configured')}</p>
            <p className="text-caption mt-1">
              {t('settings.mcp.noServersHint', "Add a server to extend Aleph's capabilities")}
            </p>
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-lg">
            {/* Master List */}
            <div className="space-y-md">
              <p className="text-caption text-muted-foreground">
                {t('settings.mcp.serversCount', 'Servers ({{count}})', { count: servers.length })}
              </p>
              <McpServerList
                servers={servers}
                selectedName={selectedName}
                onSelect={setSelectedName}
                onToggle={handleToggle}
                isToggling={togglingName}
              />
            </div>

            {/* Detail Panel */}
            <div className="lg:border-l lg:pl-lg border-border">
              {selectedServer ? (
                <McpServerDetail
                  server={selectedServer}
                  onUpdate={handleUpdate}
                  onDelete={() => handleDelete(selectedServer.name)}
                  isDeleting={deletingName === selectedServer.name}
                />
              ) : (
                <div className="flex items-center justify-center h-64 text-muted-foreground">
                  <p>{t('settings.mcp.selectServer', 'Select a server to configure')}</p>
                </div>
              )}
            </div>
          </div>
        )}
      </SettingsSection>

      {/* Info */}
      <InfoBox variant="info">
        <div className="flex items-start gap-sm">
          <Info className="h-4 w-4 mt-0.5 flex-shrink-0" />
          <span>
            {t('settings.mcp.hint', 'MCP servers extend Aleph with additional tools and capabilities. Visit the Model Context Protocol documentation to learn more.')}
          </span>
        </div>
      </InfoBox>

      <AddServerDialog open={dialogOpen} onOpenChange={setDialogOpen} onAdd={handleAdd} />
    </div>
  );
}

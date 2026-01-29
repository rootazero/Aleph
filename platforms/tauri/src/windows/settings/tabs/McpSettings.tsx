import { useState } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
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
import { Plus, Server, Trash2, Play, X, Info } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';
import type { McpServer } from '@/lib/commands';

// Master-Detail Layout Component
function McpServerList({
  servers,
  selectedId,
  onSelect,
  onToggle,
}: {
  servers: McpServer[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  onToggle: (id: string) => void;
}) {
  return (
    <div className="space-y-2">
      {servers.map((server) => (
        <button
          key={server.id}
          onClick={() => onSelect(server.id)}
          className={cn(
            'w-full flex items-center justify-between p-3 rounded-card border text-left transition-colors',
            selectedId === server.id
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
          <Switch
            checked={server.enabled}
            onCheckedChange={() => onToggle(server.id)}
            onClick={(e) => e.stopPropagation()}
          />
        </button>
      ))}
    </div>
  );
}

function McpServerDetail({
  server,
  onUpdate,
  onDelete,
}: {
  server: McpServer;
  onUpdate: (server: McpServer) => void;
  onDelete: () => void;
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
          <Button
            variant="ghost"
            size="sm"
            onClick={onDelete}
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
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
  onAdd: (server: McpServer) => void;
}) {
  const [name, setName] = useState('');
  const [command, setCommand] = useState('');

  const handleAdd = () => {
    if (name.trim() && command.trim()) {
      onAdd({
        id: crypto.randomUUID(),
        name: name.trim(),
        command: command.trim(),
        args: [],
        env: {},
        enabled: true,
      });
      setName('');
      setCommand('');
      onOpenChange(false);
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
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={handleAdd} disabled={!name.trim() || !command.trim()}>
            Add
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function McpSettings() {
  const { t } = useTranslation();
  const mcp = useSettingsStore((s) => s.mcp);
  const updateMcp = useSettingsStore((s) => s.updateMcp);

  const [selectedId, setSelectedId] = useState<string | null>(
    mcp.servers[0]?.id || null
  );
  const [dialogOpen, setDialogOpen] = useState(false);

  const selectedServer = mcp.servers.find((s) => s.id === selectedId);

  const handleToggle = (id: string) => {
    updateMcp({
      servers: mcp.servers.map((s) =>
        s.id === id ? { ...s, enabled: !s.enabled } : s
      ),
    });
  };

  const handleUpdate = (server: McpServer) => {
    updateMcp({
      servers: mcp.servers.map((s) => (s.id === server.id ? server : s)),
    });
  };

  const handleDelete = (id: string) => {
    updateMcp({
      servers: mcp.servers.filter((s) => s.id !== id),
    });
    setSelectedId(mcp.servers.find((s) => s.id !== id)?.id || null);
  };

  const handleAdd = (server: McpServer) => {
    updateMcp({
      servers: [...mcp.servers, server],
    });
    setSelectedId(server.id);
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
        <Button onClick={() => setDialogOpen(true)}>
          <Plus className="h-4 w-4 mr-2" />
          {t('settings.mcp.addServer', 'Add Server')}
        </Button>
      </div>

      {/* Server Management */}
      <SettingsSection header={t('settings.mcp.serversSection', 'Servers')}>
        {mcp.servers.length === 0 ? (
          <div className="text-center py-12 text-muted-foreground border border-dashed border-border rounded-card">
            <Server className="h-12 w-12 mx-auto mb-4 opacity-50" />
            <p>{t('settings.mcp.noServers', 'No MCP servers configured')}</p>
            <p className="text-caption mt-1">
              {t('settings.mcp.noServersHint', "Add a server to extend Aether's capabilities")}
            </p>
          </div>
        ) : (
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-lg">
            {/* Master List */}
            <div className="space-y-md">
              <p className="text-caption text-muted-foreground">
                {t('settings.mcp.serversCount', 'Servers ({{count}})', { count: mcp.servers.length })}
              </p>
              <McpServerList
                servers={mcp.servers}
                selectedId={selectedId}
                onSelect={setSelectedId}
                onToggle={handleToggle}
              />
            </div>

            {/* Detail Panel */}
            <div className="lg:border-l lg:pl-lg border-border">
              {selectedServer ? (
                <McpServerDetail
                  server={selectedServer}
                  onUpdate={handleUpdate}
                  onDelete={() => handleDelete(selectedServer.id)}
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
            {t('settings.mcp.hint', 'MCP servers extend Aether with additional tools and capabilities. Visit the Model Context Protocol documentation to learn more.')}
          </span>
        </div>
      </InfoBox>

      <AddServerDialog open={dialogOpen} onOpenChange={setDialogOpen} onAdd={handleAdd} />
    </div>
  );
}

import { useState } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  AlertTriangle,
  FolderOpen,
  Terminal,
  Globe,
  ShieldCheck,
  X,
  Plus,
} from 'lucide-react';
import { cn } from '@/lib/utils';

export function AgentSettings() {
  const agent = useSettingsStore((s) => s.agent);
  const updateAgent = useSettingsStore((s) => s.updateAgent);

  const [newPath, setNewPath] = useState('');
  const [newCommand, setNewCommand] = useState('');

  const addPath = () => {
    if (newPath.trim() && !agent.allowed_paths.includes(newPath.trim())) {
      updateAgent({ allowed_paths: [...agent.allowed_paths, newPath.trim()] });
      setNewPath('');
    }
  };

  const removePath = (path: string) => {
    updateAgent({ allowed_paths: agent.allowed_paths.filter((p) => p !== path) });
  };

  const addBlockedCommand = () => {
    if (newCommand.trim() && !agent.blocked_commands.includes(newCommand.trim())) {
      updateAgent({
        blocked_commands: [...agent.blocked_commands, newCommand.trim()],
      });
      setNewCommand('');
    }
  };

  const removeBlockedCommand = (cmd: string) => {
    updateAgent({
      blocked_commands: agent.blocked_commands.filter((c) => c !== cmd),
    });
  };

  return (
    <div className="space-y-6 max-w-2xl">
      <div>
        <h1 className="text-title mb-1">Agent</h1>
        <p className="text-caption text-muted-foreground">
          Configure AI agent capabilities and safety settings
        </p>
      </div>

      {/* Capabilities */}
      <section className="space-y-4">
        <h2 className="text-body font-medium text-foreground">Capabilities</h2>

        <SettingsCard
          title="File Operations"
          description="Allow the agent to read, write, and modify files"
        >
          <div className="flex items-center gap-2">
            <FolderOpen className="h-4 w-4 text-muted-foreground" />
            <Switch
              checked={agent.file_operations}
              onCheckedChange={(checked) =>
                updateAgent({ file_operations: checked })
              }
            />
          </div>
        </SettingsCard>

        <SettingsCard
          title="Code Execution"
          description="Allow the agent to execute code and scripts"
          className={cn(
            agent.code_execution && 'border-warning/50 bg-warning/5'
          )}
        >
          <div className="flex items-center gap-2">
            <Terminal className="h-4 w-4 text-muted-foreground" />
            <Switch
              checked={agent.code_execution}
              onCheckedChange={(checked) =>
                updateAgent({ code_execution: checked })
              }
            />
          </div>
        </SettingsCard>

        {agent.code_execution && (
          <div className="flex items-start gap-2 p-3 rounded-medium bg-warning/10 text-warning">
            <AlertTriangle className="h-5 w-5 flex-shrink-0 mt-0.5" />
            <p className="text-caption">
              Code execution is enabled. The agent can run commands on your system.
              Make sure to configure sandbox mode and blocked commands.
            </p>
          </div>
        )}

        <SettingsCard
          title="Web Browsing"
          description="Allow the agent to search and browse the web"
        >
          <div className="flex items-center gap-2">
            <Globe className="h-4 w-4 text-muted-foreground" />
            <Switch
              checked={agent.web_browsing}
              onCheckedChange={(checked) => updateAgent({ web_browsing: checked })}
            />
          </div>
        </SettingsCard>
      </section>

      {/* Limits */}
      <section className="space-y-4">
        <h2 className="text-body font-medium text-foreground">Limits</h2>

        <SettingsCard
          title="Max Iterations"
          description="Maximum steps the agent can take per task"
        >
          <div className="flex items-center gap-3 w-48">
            <Slider
              value={[agent.max_iterations]}
              onValueChange={([value]) => updateAgent({ max_iterations: value })}
              min={1}
              max={50}
              step={1}
              className="flex-1"
            />
            <span className="text-caption text-muted-foreground w-8 text-right">
              {agent.max_iterations}
            </span>
          </div>
        </SettingsCard>

        <SettingsCard
          title="Require Confirmation"
          description="Ask for confirmation before executing actions"
        >
          <Switch
            checked={agent.require_confirmation}
            onCheckedChange={(checked) =>
              updateAgent({ require_confirmation: checked })
            }
          />
        </SettingsCard>
      </section>

      {/* Safety */}
      <section className="space-y-4">
        <h2 className="text-body font-medium text-foreground flex items-center gap-2">
          <ShieldCheck className="h-4 w-4" />
          Safety
        </h2>

        <SettingsCard
          title="Sandbox Mode"
          description="Run commands in an isolated environment"
        >
          <Switch
            checked={agent.sandbox_mode}
            onCheckedChange={(checked) => updateAgent({ sandbox_mode: checked })}
          />
        </SettingsCard>

        {/* Allowed Paths */}
        <div className="p-4 rounded-card bg-card border border-border space-y-3">
          <div className="space-y-1">
            <label className="text-body font-medium text-foreground">
              Allowed Paths
            </label>
            <p className="text-caption text-muted-foreground">
              Directories the agent can access for file operations
            </p>
          </div>

          <div className="flex gap-2">
            <Input
              value={newPath}
              onChange={(e) => setNewPath(e.target.value)}
              placeholder="/Users/username/projects"
              className="flex-1 font-mono text-sm"
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  addPath();
                }
              }}
            />
            <Button variant="secondary" size="icon" onClick={addPath}>
              <Plus className="h-4 w-4" />
            </Button>
          </div>

          {agent.allowed_paths.length > 0 && (
            <div className="space-y-1">
              {agent.allowed_paths.map((path) => (
                <div
                  key={path}
                  className="flex items-center justify-between p-2 rounded-small bg-secondary/50"
                >
                  <span className="font-mono text-sm text-foreground truncate">
                    {path}
                  </span>
                  <button
                    onClick={() => removePath(path)}
                    className="text-muted-foreground hover:text-destructive ml-2"
                  >
                    <X className="h-4 w-4" />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Blocked Commands */}
        <div className="p-4 rounded-card bg-card border border-border space-y-3">
          <div className="space-y-1">
            <label className="text-body font-medium text-foreground">
              Blocked Commands
            </label>
            <p className="text-caption text-muted-foreground">
              Commands that are never allowed to execute
            </p>
          </div>

          <div className="flex gap-2">
            <Input
              value={newCommand}
              onChange={(e) => setNewCommand(e.target.value)}
              placeholder="rm -rf"
              className="flex-1 font-mono text-sm"
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  addBlockedCommand();
                }
              }}
            />
            <Button variant="secondary" size="icon" onClick={addBlockedCommand}>
              <Plus className="h-4 w-4" />
            </Button>
          </div>

          {agent.blocked_commands.length > 0 && (
            <div className="flex flex-wrap gap-2">
              {agent.blocked_commands.map((cmd) => (
                <Badge
                  key={cmd}
                  variant="destructive"
                  className="font-mono text-xs flex items-center gap-1"
                >
                  {cmd}
                  <button onClick={() => removeBlockedCommand(cmd)} className="ml-1">
                    <X className="h-3 w-3" />
                  </button>
                </Badge>
              ))}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}

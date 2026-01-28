import { useState } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { SettingsSection } from '@/components/ui/settings-section';
import { InfoBox } from '@/components/ui/info-box';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import { Input } from '@/components/ui/input';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  FolderOpen,
  Terminal,
  Globe,
  ShieldCheck,
  X,
  Plus,
  FolderPlus,
  Clock,
  Network,
  FileWarning,
  Trash2,
  Pencil,
  Info,
  Repeat,
  Shield,
  Play,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';

// Path list editor component with file browser support
interface PathListEditorProps {
  paths: string[];
  onAdd: (path: string) => void;
  onRemove: (path: string) => void;
  placeholder: string;
  browseTitle?: string;
}

function PathListEditor({
  paths,
  onAdd,
  onRemove,
  placeholder,
  browseTitle,
}: PathListEditorProps) {
  const [newPath, setNewPath] = useState('');

  const handleAdd = () => {
    if (newPath.trim() && !paths.includes(newPath.trim())) {
      onAdd(newPath.trim());
      setNewPath('');
    }
  };

  const handleBrowse = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: browseTitle || 'Select Directory',
      });
      if (selected && typeof selected === 'string' && !paths.includes(selected)) {
        onAdd(selected);
      }
    } catch (err) {
      console.error('Failed to open directory picker:', err);
    }
  };

  return (
    <div className="space-y-2">
      <div className="flex gap-2">
        <Input
          value={newPath}
          onChange={(e) => setNewPath(e.target.value)}
          placeholder={placeholder}
          className="flex-1 font-mono text-sm"
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              handleAdd();
            }
          }}
        />
        <Button variant="outline" size="icon" onClick={handleBrowse} title="Browse">
          <FolderPlus className="h-4 w-4" />
        </Button>
        <Button variant="secondary" size="icon" onClick={handleAdd} disabled={!newPath.trim()}>
          <Plus className="h-4 w-4" />
        </Button>
      </div>

      {paths.length > 0 && (
        <div className="space-y-1">
          {paths.map((path) => (
            <div
              key={path}
              className="flex items-center justify-between p-2 rounded-small bg-secondary/50"
            >
              <span className="font-mono text-sm text-foreground truncate flex-1">
                {path}
              </span>
              <button
                onClick={() => onRemove(path)}
                className="text-muted-foreground hover:text-destructive ml-2 flex-shrink-0"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// Format file size for display
function formatFileSize(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) {
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(0)} GB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(0)} MB`;
}

export function AgentSettings() {
  const { t } = useTranslation();
  const agent = useSettingsStore((s) => s.agent);
  const updateAgent = useSettingsStore((s) => s.updateAgent);
  const updateFileOps = useSettingsStore((s) => s.updateFileOps);
  const updateCodeExec = useSettingsStore((s) => s.updateCodeExec);

  const [newCommand, setNewCommand] = useState('');

  const addBlockedCommand = () => {
    if (newCommand.trim() && !agent.code_exec.blocked_commands.includes(newCommand.trim())) {
      updateCodeExec({
        blocked_commands: [...agent.code_exec.blocked_commands, newCommand.trim()],
      });
      setNewCommand('');
    }
  };

  const removeBlockedCommand = (cmd: string) => {
    updateCodeExec({
      blocked_commands: agent.code_exec.blocked_commands.filter((c) => c !== cmd),
    });
  };

  // File size options in bytes
  const fileSizeOptions = [
    { value: 10 * 1024 * 1024, label: '10 MB' },
    { value: 50 * 1024 * 1024, label: '50 MB' },
    { value: 100 * 1024 * 1024, label: '100 MB' },
    { value: 500 * 1024 * 1024, label: '500 MB' },
    { value: 1024 * 1024 * 1024, label: '1 GB' },
  ];

  return (
    <div className="space-y-lg max-w-2xl">
      {/* Page Header */}
      <div>
        <h1 className="text-title mb-1">{t('settings.agent.title')}</h1>
        <p className="text-caption text-muted-foreground">
          {t('settings.agent.description')}
        </p>
      </div>

      {/* File Operations Section */}
      <SettingsSection header={t('settings.agent.fileOps.title', 'File Operations')}>
        <SettingsCard
          title={t('settings.agent.fileOps.enabled', 'Enable File Operations')}
          description={t('settings.agent.fileOps.enabledDescription', 'Allow the agent to read, write, and modify files')}
          icon={FolderOpen}
        >
          <Switch
            checked={agent.file_ops.enabled}
            onCheckedChange={(checked) => updateFileOps({ enabled: checked })}
          />
        </SettingsCard>

        {agent.file_ops.enabled && (
          <>
            {/* Allowed Paths */}
            <div className="p-4 rounded-card bg-card border border-border space-y-3">
              <div className="space-y-1">
                <label className="text-body font-medium text-foreground flex items-center gap-2">
                  <ShieldCheck className="h-4 w-4 text-green-500" />
                  {t('settings.agent.fileOps.allowedPaths', 'Allowed Paths')}
                </label>
                <p className="text-caption text-muted-foreground">
                  {t('settings.agent.fileOps.allowedPathsDescription', 'Directories the agent can access for file operations')}
                </p>
              </div>

              <PathListEditor
                paths={agent.file_ops.allowed_paths}
                onAdd={(path) =>
                  updateFileOps({ allowed_paths: [...agent.file_ops.allowed_paths, path] })
                }
                onRemove={(path) =>
                  updateFileOps({
                    allowed_paths: agent.file_ops.allowed_paths.filter((p) => p !== path),
                  })
                }
                placeholder={t('settings.agent.fileOps.addAllowedPath', '/path/to/allowed/directory')}
                browseTitle={t('settings.agent.fileOps.selectAllowedPath', 'Select Allowed Directory')}
              />
            </div>

            {/* Denied Paths */}
            <div className="p-4 rounded-card bg-card border border-border space-y-3">
              <div className="space-y-1">
                <label className="text-body font-medium text-foreground flex items-center gap-2">
                  <FileWarning className="h-4 w-4 text-red-500" />
                  {t('settings.agent.fileOps.deniedPaths', 'Denied Paths')}
                </label>
                <p className="text-caption text-muted-foreground">
                  {t('settings.agent.fileOps.deniedPathsDescription', 'Directories the agent is never allowed to access')}
                </p>
              </div>

              <PathListEditor
                paths={agent.file_ops.denied_paths}
                onAdd={(path) =>
                  updateFileOps({ denied_paths: [...agent.file_ops.denied_paths, path] })
                }
                onRemove={(path) =>
                  updateFileOps({
                    denied_paths: agent.file_ops.denied_paths.filter((p) => p !== path),
                  })
                }
                placeholder={t('settings.agent.fileOps.addDeniedPath', '/path/to/denied/directory')}
                browseTitle={t('settings.agent.fileOps.selectDeniedPath', 'Select Denied Directory')}
              />

              <div className="flex items-start gap-2 text-muted-foreground">
                <Info className="h-4 w-4 flex-shrink-0 mt-0.5" />
                <p className="text-caption">
                  {t('settings.agent.fileOps.deniedPathsNote', 'System directories like /etc, /System, and sensitive paths are always denied by default.')}
                </p>
              </div>
            </div>

            {/* Max File Size */}
            <SettingsCard
              title={t('settings.agent.fileOps.maxFileSize', 'Maximum File Size')}
              description={t('settings.agent.fileOps.maxFileSizeDescription', 'Maximum file size the agent can read or write')}
            >
              <Select
                value={String(agent.file_ops.max_file_size)}
                onValueChange={(value) => updateFileOps({ max_file_size: Number(value) })}
              >
                <SelectTrigger className="w-32">
                  <SelectValue>{formatFileSize(agent.file_ops.max_file_size)}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {fileSizeOptions.map((option) => (
                    <SelectItem key={option.value} value={String(option.value)}>
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </SettingsCard>

            {/* Confirmations */}
            <div className="p-4 rounded-card bg-card border border-border space-y-3">
              <div className="space-y-1">
                <label className="text-body font-medium text-foreground">
                  {t('settings.agent.fileOps.confirmations', 'Require Confirmation')}
                </label>
                <p className="text-caption text-muted-foreground">
                  {t('settings.agent.fileOps.confirmationsDescription', 'Ask for confirmation before certain file operations')}
                </p>
              </div>

              <div className="space-y-3">
                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <Pencil className="h-4 w-4 text-muted-foreground" />
                    <span className="text-body">
                      {t('settings.agent.fileOps.confirmWrite', 'Before writing files')}
                    </span>
                  </div>
                  <Switch
                    checked={agent.file_ops.require_confirmation_for_write}
                    onCheckedChange={(checked) =>
                      updateFileOps({ require_confirmation_for_write: checked })
                    }
                  />
                </div>

                <div className="flex items-center justify-between">
                  <div className="flex items-center gap-2">
                    <Trash2 className="h-4 w-4 text-muted-foreground" />
                    <span className="text-body">
                      {t('settings.agent.fileOps.confirmDelete', 'Before deleting files')}
                    </span>
                  </div>
                  <Switch
                    checked={agent.file_ops.require_confirmation_for_delete}
                    onCheckedChange={(checked) =>
                      updateFileOps({ require_confirmation_for_delete: checked })
                    }
                  />
                </div>
              </div>

              {!agent.file_ops.require_confirmation_for_write &&
                !agent.file_ops.require_confirmation_for_delete && (
                  <InfoBox variant="warning">
                    {t('settings.agent.fileOps.noConfirmationWarning', 'Disabling confirmations allows the agent to modify files without asking. Use with caution.')}
                  </InfoBox>
                )}
            </div>
          </>
        )}
      </SettingsSection>

      {/* Code Execution Section */}
      <SettingsSection header={t('settings.agent.codeExec.title', 'Code Execution')}>
        <SettingsCard
          title={t('settings.agent.codeExec.enabled', 'Enable Code Execution')}
          description={t('settings.agent.codeExec.enabledDescription', 'Allow the agent to execute code and scripts')}
          icon={Terminal}
          className={cn(agent.code_exec.enabled && 'border-warning/50 bg-warning/5')}
        >
          <Switch
            checked={agent.code_exec.enabled}
            onCheckedChange={(checked) => updateCodeExec({ enabled: checked })}
          />
        </SettingsCard>

        {agent.code_exec.enabled && (
          <>
            <InfoBox variant="warning">
              {t('settings.agent.codeExec.warning', 'Code execution is enabled. The agent can run commands on your system. Make sure to configure sandbox mode and blocked commands.')}
            </InfoBox>

            {/* Sandbox Mode */}
            <SettingsCard
              title={t('settings.agent.codeExec.sandbox', 'Sandbox Mode')}
              description={t('settings.agent.codeExec.sandboxDescription', 'Run commands in an isolated environment with restricted permissions')}
              icon={Shield}
            >
              <Switch
                checked={agent.code_exec.sandbox_enabled}
                onCheckedChange={(checked) => updateCodeExec({ sandbox_enabled: checked })}
              />
            </SettingsCard>

            {/* Network Access (only when sandbox enabled) */}
            {agent.code_exec.sandbox_enabled && (
              <SettingsCard
                title={t('settings.agent.codeExec.network', 'Allow Network Access')}
                description={t('settings.agent.codeExec.networkDescription', 'Allow sandboxed code to make network requests')}
                icon={Network}
              >
                <Switch
                  checked={agent.code_exec.allow_network}
                  onCheckedChange={(checked) => updateCodeExec({ allow_network: checked })}
                />
              </SettingsCard>
            )}

            {/* Timeout */}
            <SettingsCard
              title={t('settings.agent.codeExec.timeout', 'Execution Timeout')}
              description={t('settings.agent.codeExec.timeoutDescription', 'Maximum time a command can run before being terminated')}
              icon={Clock}
            >
              <div className="flex items-center gap-md w-48">
                <Slider
                  value={[agent.code_exec.timeout_seconds]}
                  onValueChange={([value]) => updateCodeExec({ timeout_seconds: value })}
                  min={10}
                  max={300}
                  step={10}
                  className="flex-1"
                />
                <span className="text-caption text-muted-foreground w-12 text-right font-mono">
                  {agent.code_exec.timeout_seconds}s
                </span>
              </div>
            </SettingsCard>

            {/* Default Runtime */}
            <SettingsCard
              title={t('settings.agent.codeExec.runtime', 'Default Runtime')}
              description={t('settings.agent.codeExec.runtimeDescription', 'Default environment for executing code')}
              icon={Play}
            >
              <Select
                value={agent.code_exec.default_runtime}
                onValueChange={(value) =>
                  updateCodeExec({ default_runtime: value as 'shell' | 'python' | 'node' })
                }
              >
                <SelectTrigger className="w-40">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="shell">Shell (bash/zsh)</SelectItem>
                  <SelectItem value="python">Python</SelectItem>
                  <SelectItem value="node">Node.js</SelectItem>
                </SelectContent>
              </Select>
            </SettingsCard>

            {/* Blocked Commands */}
            <div className="p-4 rounded-card bg-card border border-border space-y-3">
              <div className="space-y-1">
                <label className="text-body font-medium text-foreground">
                  {t('settings.agent.codeExec.blockedCommands', 'Blocked Commands')}
                </label>
                <p className="text-caption text-muted-foreground">
                  {t('settings.agent.codeExec.blockedCommandsDescription', 'Commands that are never allowed to execute')}
                </p>
              </div>

              <div className="flex gap-2">
                <Input
                  value={newCommand}
                  onChange={(e) => setNewCommand(e.target.value)}
                  placeholder={t('settings.agent.codeExec.addBlockedCommand', 'rm -rf')}
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

              {agent.code_exec.blocked_commands.length > 0 && (
                <div className="flex flex-wrap gap-2">
                  {agent.code_exec.blocked_commands.map((cmd) => (
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
          </>
        )}
      </SettingsSection>

      {/* Other Settings */}
      <SettingsSection header={t('settings.agent.other', 'Other Settings')}>
        <SettingsCard
          title={t('settings.agent.webBrowsing', 'Web Browsing')}
          description={t('settings.agent.webBrowsingDescription', 'Allow the agent to search and browse the web')}
          icon={Globe}
        >
          <Switch
            checked={agent.web_browsing}
            onCheckedChange={(checked) => updateAgent({ web_browsing: checked })}
          />
        </SettingsCard>

        <SettingsCard
          title={t('settings.agent.maxIterations', 'Max Iterations')}
          description={t('settings.agent.maxIterationsDescription', 'Maximum steps the agent can take per task')}
          icon={Repeat}
        >
          <div className="flex items-center gap-md w-48">
            <Slider
              value={[agent.max_iterations]}
              onValueChange={([value]) => updateAgent({ max_iterations: value })}
              min={1}
              max={50}
              step={1}
              className="flex-1"
            />
            <span className="text-caption text-muted-foreground w-8 text-right font-mono">
              {agent.max_iterations}
            </span>
          </div>
        </SettingsCard>
      </SettingsSection>
    </div>
  );
}

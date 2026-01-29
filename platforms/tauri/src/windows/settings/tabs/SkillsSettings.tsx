import { useState, useEffect, useCallback } from 'react';
import { useSettingsStore } from '@/stores/settingsStore';
import { useGatewayStore, gateway } from '@/stores/gatewayStore';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
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
import {
  Sparkles,
  Plus,
  Trash2,
  Zap,
  Tag,
  Info,
  RefreshCw,
  Loader2,
  AlertCircle,
  Download,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';
import type { GWSkillInfo } from '@/lib/gateway';

interface SkillCardProps {
  skill: GWSkillInfo;
  onDelete: () => void;
  isDeleting?: boolean;
}

function SkillCard({ skill, onDelete, isDeleting }: SkillCardProps) {
  return (
    <div
      className={cn(
        'p-4 rounded-card border transition-colors',
        'border-border bg-card'
      )}
    >
      <div className="flex items-start justify-between">
        <div className="flex items-start gap-3">
          <div className="w-10 h-10 rounded-medium bg-gradient-to-br from-primary/20 to-primary/5 flex items-center justify-center flex-shrink-0">
            <Zap className="h-5 w-5 text-primary" />
          </div>
          <div>
            <p className="text-body font-medium text-foreground">{skill.name}</p>
            <p className="text-caption text-muted-foreground mt-1 line-clamp-2">
              {skill.description || 'No description'}
            </p>
            {skill.source && (
              <div className="flex items-center gap-1 mt-2">
                <Tag className="h-3 w-3 text-muted-foreground" />
                <Badge variant="outline" className="text-xs">
                  {skill.source}
                </Badge>
              </div>
            )}
          </div>
        </div>

        <div className="flex items-center gap-2 flex-shrink-0 ml-4">
          {isDeleting ? (
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          ) : (
            <Button
              variant="ghost"
              size="icon"
              onClick={onDelete}
              title="Delete"
              className="text-destructive hover:text-destructive"
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}

function InstallSkillDialog({
  open,
  onOpenChange,
  onInstall,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onInstall: (url: string) => Promise<void>;
}) {
  const [url, setUrl] = useState('');
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleInstall = async () => {
    if (!url.trim()) return;

    setIsLoading(true);
    setError(null);

    try {
      await onInstall(url);
      setUrl('');
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Installation failed');
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Install Skill</DialogTitle>
          <DialogDescription>
            Install a skill from a URL
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <label className="text-body font-medium">Skill URL</label>
            <Input
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="https://github.com/user/skill.git"
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
          <Button onClick={handleInstall} disabled={!url.trim() || isLoading}>
            {isLoading ? (
              <>
                <RefreshCw className="h-4 w-4 mr-2 animate-spin" />
                Installing...
              </>
            ) : (
              <>
                <Download className="h-4 w-4 mr-2" />
                Install
              </>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function SkillsSettings() {
  const { t } = useTranslation();
  const localSkills = useSettingsStore((s) => s.skills);
  const updateSkills = useSettingsStore((s) => s.updateSkills);
  const isConnected = useGatewayStore((s) => s.isConnected);

  // Gateway-loaded skills
  const [skills, setSkills] = useState<GWSkillInfo[]>([]);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);

  // Load skills from Gateway
  const loadSkills = useCallback(async () => {
    if (!isConnected()) {
      // Fallback to local settings
      setSkills(localSkills.skills.map(s => ({
        id: s.id,
        name: s.name,
        description: s.description,
        source: null,
      })));
      setIsLoading(false);
      return;
    }

    setIsLoading(true);
    setError(null);

    try {
      const result = await gateway.skillsList();
      setSkills(result);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load skills');
      // Fallback to local
      setSkills(localSkills.skills.map(s => ({
        id: s.id,
        name: s.name,
        description: s.description,
        source: null,
      })));
    } finally {
      setIsLoading(false);
    }
  }, [isConnected, localSkills.skills]);

  useEffect(() => {
    loadSkills();
  }, [loadSkills]);

  const handleDelete = async (skill: GWSkillInfo) => {
    setDeletingId(skill.id);

    try {
      if (isConnected()) {
        await gateway.skillsDelete(skill.id);
        await loadSkills();
      } else {
        updateSkills({
          skills: localSkills.skills.filter((s) => s.id !== skill.id),
        });
        setSkills(prev => prev.filter(s => s.id !== skill.id));
      }
    } catch (e) {
      console.error('Failed to delete skill:', e);
    } finally {
      setDeletingId(null);
    }
  };

  const handleInstall = async (url: string) => {
    if (isConnected()) {
      await gateway.skillsInstall(url);
      await loadSkills();
    } else {
      // Fallback to local simulation
      const skill = {
        id: crypto.randomUUID(),
        name: url.split('/').pop()?.replace('.git', '') || 'Skill',
        description: 'Newly installed skill',
        enabled: true,
        trigger_keywords: [],
      };
      updateSkills({
        skills: [...localSkills.skills, skill],
      });
      setSkills(prev => [...prev, {
        id: skill.id,
        name: skill.name,
        description: skill.description,
        source: 'local',
      }]);
    }
  };

  return (
    <div className="space-y-lg max-w-3xl">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-title mb-1">{t('settings.skills.title', 'Skills')}</h1>
          <p className="text-caption text-muted-foreground">
            {t('settings.skills.description', 'Install and manage AI skills')}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button variant="outline" size="icon" onClick={loadSkills} disabled={isLoading}>
            <RefreshCw className={cn("h-4 w-4", isLoading && "animate-spin")} />
          </Button>
          <Button onClick={() => setDialogOpen(true)}>
            <Plus className="h-4 w-4 mr-2" />
            {t('settings.skills.installSkill', 'Install Skill')}
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

      {/* Skills List */}
      <SettingsSection header={t('settings.skills.installedSection', 'Installed Skills ({{count}})', { count: skills.length })}>
        {isLoading ? (
          <div className="flex items-center justify-center py-12">
            <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" />
          </div>
        ) : skills.length === 0 ? (
          <div className="text-center py-12 text-muted-foreground border border-dashed border-border rounded-card">
            <Sparkles className="h-12 w-12 mx-auto mb-4 opacity-50" />
            <p>{t('settings.skills.noSkills', 'No skills installed')}</p>
            <p className="text-caption mt-1">
              {t('settings.skills.noSkillsHint', 'Install skills to extend AI capabilities')}
            </p>
          </div>
        ) : (
          <div className="space-y-sm">
            {skills.map((skill) => (
              <SkillCard
                key={skill.id}
                skill={skill}
                onDelete={() => handleDelete(skill)}
                isDeleting={deletingId === skill.id}
              />
            ))}
          </div>
        )}
      </SettingsSection>

      {/* Info */}
      <InfoBox variant="info">
        <div className="flex items-start gap-sm">
          <Info className="h-4 w-4 mt-0.5 flex-shrink-0" />
          <span>
            {t('settings.skills.hint', 'Skills extend the AI with specialized capabilities. Install skills from Git repositories or local folders.')}
          </span>
        </div>
      </InfoBox>

      <InstallSkillDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        onInstall={handleInstall}
      />
    </div>
  );
}

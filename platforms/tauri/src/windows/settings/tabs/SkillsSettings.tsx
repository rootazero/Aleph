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
import { Sparkles, Plus, Trash2, X, Zap, Tag, Info } from 'lucide-react';
import { cn } from '@/lib/utils';
import { useTranslation } from 'react-i18next';
import type { Skill } from '@/lib/commands';

function SkillCard({
  skill,
  onToggle,
  onDelete,
  onEdit,
}: {
  skill: Skill;
  onToggle: () => void;
  onDelete: () => void;
  onEdit: () => void;
}) {
  return (
    <div
      className={cn(
        'p-4 rounded-card border transition-colors cursor-pointer hover:border-primary/50',
        skill.enabled ? 'border-border bg-card' : 'border-border/50 bg-muted/30'
      )}
      onClick={onEdit}
    >
      <div className="flex items-start justify-between">
        <div className="flex items-start gap-3">
          <div className="w-10 h-10 rounded-medium bg-gradient-to-br from-primary/20 to-primary/5 flex items-center justify-center flex-shrink-0">
            <Zap className="h-5 w-5 text-primary" />
          </div>
          <div>
            <p className="text-body font-medium text-foreground">{skill.name}</p>
            <p className="text-caption text-muted-foreground mt-1 line-clamp-2">
              {skill.description}
            </p>
            {skill.trigger_keywords.length > 0 && (
              <div className="flex items-center gap-1 mt-2">
                <Tag className="h-3 w-3 text-muted-foreground" />
                <div className="flex flex-wrap gap-1">
                  {skill.trigger_keywords.slice(0, 3).map((keyword) => (
                    <Badge key={keyword} variant="outline" className="text-xs">
                      {keyword}
                    </Badge>
                  ))}
                  {skill.trigger_keywords.length > 3 && (
                    <Badge variant="outline" className="text-xs">
                      +{skill.trigger_keywords.length - 3}
                    </Badge>
                  )}
                </div>
              </div>
            )}
          </div>
        </div>

        <div className="flex items-center gap-2 flex-shrink-0 ml-4">
          <Button
            variant="ghost"
            size="icon"
            onClick={(e) => {
              e.stopPropagation();
              onDelete();
            }}
            title="Delete"
            className="text-destructive hover:text-destructive"
          >
            <Trash2 className="h-4 w-4" />
          </Button>
          <Switch
            checked={skill.enabled}
            onCheckedChange={onToggle}
            onClick={(e) => e.stopPropagation()}
          />
        </div>
      </div>
    </div>
  );
}

function SkillDialog({
  open,
  onOpenChange,
  skill,
  onSave,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  skill: Skill | null;
  onSave: (skill: Skill) => void;
}) {
  const [form, setForm] = useState<Skill>(
    skill || {
      id: crypto.randomUUID(),
      name: '',
      description: '',
      enabled: true,
      trigger_keywords: [],
    }
  );
  const [newKeyword, setNewKeyword] = useState('');

  const addKeyword = () => {
    if (newKeyword.trim() && !form.trigger_keywords.includes(newKeyword.trim())) {
      setForm({
        ...form,
        trigger_keywords: [...form.trigger_keywords, newKeyword.trim()],
      });
      setNewKeyword('');
    }
  };

  const removeKeyword = (keyword: string) => {
    setForm({
      ...form,
      trigger_keywords: form.trigger_keywords.filter((k) => k !== keyword),
    });
  };

  const handleSave = () => {
    if (form.name.trim()) {
      onSave(form);
      onOpenChange(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{skill ? 'Edit Skill' : 'Create Skill'}</DialogTitle>
          <DialogDescription>
            Define a custom skill with trigger keywords
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <label className="text-body font-medium">Name</label>
            <Input
              value={form.name}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="My Custom Skill"
            />
          </div>

          <div className="space-y-2">
            <label className="text-body font-medium">Description</label>
            <Input
              value={form.description}
              onChange={(e) => setForm({ ...form, description: e.target.value })}
              placeholder="What this skill does..."
            />
          </div>

          <div className="space-y-2">
            <label className="text-body font-medium">Trigger Keywords</label>
            <div className="flex gap-2">
              <Input
                value={newKeyword}
                onChange={(e) => setNewKeyword(e.target.value)}
                placeholder="Add keyword..."
                onKeyDown={(e) => {
                  if (e.key === 'Enter') {
                    e.preventDefault();
                    addKeyword();
                  }
                }}
              />
              <Button variant="secondary" size="icon" onClick={addKeyword}>
                <Plus className="h-4 w-4" />
              </Button>
            </div>
            {form.trigger_keywords.length > 0 && (
              <div className="flex flex-wrap gap-2 mt-2">
                {form.trigger_keywords.map((keyword) => (
                  <Badge
                    key={keyword}
                    variant="secondary"
                    className="flex items-center gap-1"
                  >
                    {keyword}
                    <button onClick={() => removeKeyword(keyword)} className="ml-1">
                      <X className="h-3 w-3" />
                    </button>
                  </Badge>
                ))}
              </div>
            )}
          </div>
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={handleSave} disabled={!form.name.trim()}>
            {skill ? 'Save' : 'Create'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function SkillsSettings() {
  const { t } = useTranslation();
  const skills = useSettingsStore((s) => s.skills);
  const updateSkills = useSettingsStore((s) => s.updateSkills);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingSkill, setEditingSkill] = useState<Skill | null>(null);

  const handleToggle = (id: string) => {
    updateSkills({
      skills: skills.skills.map((s) =>
        s.id === id ? { ...s, enabled: !s.enabled } : s
      ),
    });
  };

  const handleDelete = (id: string) => {
    updateSkills({
      skills: skills.skills.filter((s) => s.id !== id),
    });
  };

  const handleSave = (skill: Skill) => {
    if (editingSkill) {
      updateSkills({
        skills: skills.skills.map((s) => (s.id === skill.id ? skill : s)),
      });
    } else {
      updateSkills({
        skills: [...skills.skills, skill],
      });
    }
    setEditingSkill(null);
  };

  const handleEdit = (skill: Skill) => {
    setEditingSkill(skill);
    setDialogOpen(true);
  };

  const handleAddNew = () => {
    setEditingSkill(null);
    setDialogOpen(true);
  };

  return (
    <div className="space-y-lg max-w-3xl">
      {/* Page Header */}
      <div className="flex items-center justify-between">
        <div>
          <h1 className="text-title mb-1">{t('settings.skills.title', 'Skills')}</h1>
          <p className="text-caption text-muted-foreground">
            {t('settings.skills.description', 'Create custom skills triggered by keywords')}
          </p>
        </div>
        <Button onClick={handleAddNew}>
          <Plus className="h-4 w-4 mr-2" />
          {t('settings.skills.createSkill', 'Create Skill')}
        </Button>
      </div>

      {/* Skills List */}
      <SettingsSection header={t('settings.skills.definedSection', 'Defined Skills ({{count}})', { count: skills.skills.length })}>
        {skills.skills.length === 0 ? (
          <div className="text-center py-12 text-muted-foreground border border-dashed border-border rounded-card">
            <Sparkles className="h-12 w-12 mx-auto mb-4 opacity-50" />
            <p>{t('settings.skills.noSkills', 'No skills defined')}</p>
            <p className="text-caption mt-1">
              {t('settings.skills.noSkillsHint', 'Create skills to trigger specific AI behaviors')}
            </p>
          </div>
        ) : (
          <div className="space-y-sm">
            {skills.skills.map((skill) => (
              <SkillCard
                key={skill.id}
                skill={skill}
                onToggle={() => handleToggle(skill.id)}
                onDelete={() => handleDelete(skill.id)}
                onEdit={() => handleEdit(skill)}
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
            {t('settings.skills.hint', 'Skills allow you to define custom behaviors that activate when specific keywords are detected in your conversations.')}
          </span>
        </div>
      </InfoBox>

      <SkillDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        skill={editingSkill}
        onSave={handleSave}
      />
    </div>
  );
}

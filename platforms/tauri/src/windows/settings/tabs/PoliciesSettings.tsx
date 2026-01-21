import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Shield, FileText, BarChart3, Clock } from 'lucide-react';

export function PoliciesSettings() {
  const policies = useSettingsStore((s) => s.policies);
  const updatePolicies = useSettingsStore((s) => s.updatePolicies);

  return (
    <div className="space-y-6 max-w-2xl">
      <div>
        <h1 className="text-title mb-1">Policies</h1>
        <p className="text-caption text-muted-foreground">
          Configure content moderation and data policies
        </p>
      </div>

      {/* Content Safety */}
      <section className="space-y-4">
        <h2 className="text-body font-medium text-foreground flex items-center gap-2">
          <Shield className="h-4 w-4" />
          Content Safety
        </h2>

        <SettingsCard
          title="Content Filter"
          description="Filter potentially harmful content"
        >
          <Switch
            checked={policies.content_filter}
            onCheckedChange={(checked) =>
              updatePolicies({ content_filter: checked })
            }
          />
        </SettingsCard>

        {policies.content_filter && (
          <SettingsCard
            title="Filter Level"
            description="Strictness of content filtering"
          >
            <Select
              value={policies.filter_level}
              onValueChange={(value: 'strict' | 'moderate' | 'off') =>
                updatePolicies({ filter_level: value })
              }
            >
              <SelectTrigger className="w-32">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="strict">Strict</SelectItem>
                <SelectItem value="moderate">Moderate</SelectItem>
                <SelectItem value="off">Off</SelectItem>
              </SelectContent>
            </Select>
          </SettingsCard>
        )}
      </section>

      {/* Data & Privacy */}
      <section className="space-y-4">
        <h2 className="text-body font-medium text-foreground flex items-center gap-2">
          <FileText className="h-4 w-4" />
          Data & Privacy
        </h2>

        <SettingsCard
          title="Log Conversations"
          description="Save conversation history locally"
        >
          <Switch
            checked={policies.log_conversations}
            onCheckedChange={(checked) =>
              updatePolicies({ log_conversations: checked })
            }
          />
        </SettingsCard>

        {policies.log_conversations && (
          <SettingsCard
            title="Data Retention"
            description="Days to keep conversation logs"
          >
            <div className="flex items-center gap-3 w-48">
              <Clock className="h-4 w-4 text-muted-foreground" />
              <Slider
                value={[policies.data_retention_days]}
                onValueChange={([value]) =>
                  updatePolicies({ data_retention_days: value })
                }
                min={7}
                max={365}
                step={7}
                className="flex-1"
              />
              <span className="text-caption text-muted-foreground w-12 text-right">
                {policies.data_retention_days}d
              </span>
            </div>
          </SettingsCard>
        )}
      </section>

      {/* Analytics */}
      <section className="space-y-4">
        <h2 className="text-body font-medium text-foreground flex items-center gap-2">
          <BarChart3 className="h-4 w-4" />
          Analytics
        </h2>

        <SettingsCard
          title="Allow Analytics"
          description="Send anonymous usage data to improve Aether"
        >
          <Switch
            checked={policies.allow_analytics}
            onCheckedChange={(checked) =>
              updatePolicies({ allow_analytics: checked })
            }
          />
        </SettingsCard>

        {policies.allow_analytics && (
          <div className="p-3 rounded-medium bg-muted/50 text-caption text-muted-foreground">
            Analytics include: feature usage, performance metrics, and crash reports.
            No personal data, conversation content, or API keys are collected.
          </div>
        )}
      </section>
    </div>
  );
}

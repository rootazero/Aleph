import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { SettingsSection } from '@/components/ui/settings-section';
import { InfoBox } from '@/components/ui/info-box';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Shield, FileText, BarChart3, Clock, Filter } from 'lucide-react';
import { useTranslation } from 'react-i18next';

export function PoliciesSettings() {
  const { t } = useTranslation();
  const policies = useSettingsStore((s) => s.policies);
  const updatePolicies = useSettingsStore((s) => s.updatePolicies);

  return (
    <div className="space-y-lg max-w-2xl">
      {/* Page Header */}
      <div>
        <h1 className="text-title mb-1">{t('settings.policies.title', 'Policies')}</h1>
        <p className="text-caption text-muted-foreground">
          {t('settings.policies.description', 'Configure content moderation and data policies')}
        </p>
      </div>

      {/* Content Safety */}
      <SettingsSection header={t('settings.policies.contentSafetySection', 'Content Safety')}>
        <SettingsCard
          title={t('settings.policies.contentFilter', 'Content Filter')}
          description={t('settings.policies.contentFilterDescription', 'Filter potentially harmful content')}
          icon={Shield}
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
            title={t('settings.policies.filterLevel', 'Filter Level')}
            description={t('settings.policies.filterLevelDescription', 'Strictness of content filtering')}
            icon={Filter}
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
                <SelectItem value="strict">{t('settings.policies.strict', 'Strict')}</SelectItem>
                <SelectItem value="moderate">{t('settings.policies.moderate', 'Moderate')}</SelectItem>
                <SelectItem value="off">{t('settings.policies.off', 'Off')}</SelectItem>
              </SelectContent>
            </Select>
          </SettingsCard>
        )}
      </SettingsSection>

      {/* Data & Privacy */}
      <SettingsSection header={t('settings.policies.dataPrivacySection', 'Data & Privacy')}>
        <SettingsCard
          title={t('settings.policies.logConversations', 'Log Conversations')}
          description={t('settings.policies.logConversationsDescription', 'Save conversation history locally')}
          icon={FileText}
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
            title={t('settings.policies.dataRetention', 'Data Retention')}
            description={t('settings.policies.dataRetentionDescription', 'Days to keep conversation logs')}
            icon={Clock}
          >
            <div className="flex items-center gap-md w-48">
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
              <span className="text-caption text-muted-foreground w-12 text-right font-mono">
                {policies.data_retention_days}d
              </span>
            </div>
          </SettingsCard>
        )}
      </SettingsSection>

      {/* Analytics */}
      <SettingsSection header={t('settings.policies.analyticsSection', 'Analytics')}>
        <SettingsCard
          title={t('settings.policies.allowAnalytics', 'Allow Analytics')}
          description={t('settings.policies.allowAnalyticsDescription', 'Send anonymous usage data to improve Aleph')}
          icon={BarChart3}
        >
          <Switch
            checked={policies.allow_analytics}
            onCheckedChange={(checked) =>
              updatePolicies({ allow_analytics: checked })
            }
          />
        </SettingsCard>

        {policies.allow_analytics && (
          <InfoBox variant="info">
            {t('settings.policies.analyticsInfo', 'Analytics include: feature usage, performance metrics, and crash reports. No personal data, conversation content, or API keys are collected.')}
          </InfoBox>
        )}
      </SettingsSection>
    </div>
  );
}

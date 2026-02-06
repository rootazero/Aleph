import { useSettingsStore } from '@/stores/settingsStore';
import { SettingsCard } from '@/components/ui/settings-card';
import { SettingsSection } from '@/components/ui/settings-section';
import { SegmentedControl } from '@/components/ui/segmented-control';
import { InfoBox } from '@/components/ui/info-box';
import { Switch } from '@/components/ui/switch';
import { Slider } from '@/components/ui/slider';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import { useState, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import {
  Keyboard,
  Zap,
  Gauge,
  Bell,
  Shield,
  Mail,
  Phone,
  CreditCard,
  Hash,
  Play,
} from 'lucide-react';

type OutputMode = 'typewriter' | 'instant';

const outputModeOptions: Array<{
  value: OutputMode;
  label: string;
  icon: typeof Keyboard;
  description: string;
}> = [
  {
    value: 'typewriter',
    label: 'Typewriter',
    icon: Keyboard,
    description:
      'Characters appear one by one with a natural typing animation, creating a more human-like experience.',
  },
  {
    value: 'instant',
    label: 'Instant',
    icon: Zap,
    description:
      'The entire response appears immediately without animation, ideal for quick operations.',
  },
];

export function BehaviorSettings() {
  const { t } = useTranslation();
  const behavior = useSettingsStore((s) => s.behavior);
  const updateBehavior = useSettingsStore((s) => s.updateBehavior);

  // Map store output_mode to local OutputMode type
  const outputMode: OutputMode =
    behavior.output_mode === 'instant' ? 'instant' : 'typewriter';

  const handleOutputModeChange = (mode: OutputMode) => {
    updateBehavior({ output_mode: mode as 'replace' | 'append' | 'clipboard' });
  };

  const selectedModeDescription =
    outputModeOptions.find((o) => o.value === outputMode)?.description || '';

  return (
    <div className="space-y-lg max-w-2xl">
      {/* Page Header */}
      <div>
        <h1 className="text-title mb-1">
          {t('settings.behavior.title', 'Behavior')}
        </h1>
        <p className="text-caption text-muted-foreground">
          {t(
            'settings.behavior.description',
            'Control how Aleph responds and interacts'
          )}
        </p>
      </div>

      {/* Output Mode Section */}
      <SettingsSection
        header={t('settings.behavior.outputModeSection', 'Output Mode')}
      >
        <SettingsCard
          title={t('settings.behavior.outputMode', 'Output Mode')}
          description={t(
            'settings.behavior.outputModeDescription',
            'Choose how AI responses are displayed'
          )}
          icon={Keyboard}
          variant="stacked"
        >
          <SegmentedControl
            options={outputModeOptions.map((o) => ({
              value: o.value,
              label: o.label,
              icon: o.icon,
            }))}
            value={outputMode}
            onChange={handleOutputModeChange}
          />
          <InfoBox variant="info" className="mt-sm">
            {selectedModeDescription}
          </InfoBox>
        </SettingsCard>
      </SettingsSection>

      {/* Typing Speed Section (only visible in typewriter mode) */}
      {outputMode === 'typewriter' && (
        <SettingsSection
          header={t('settings.behavior.typingSpeedSection', 'Typing Speed')}
        >
          <SettingsCard
            title={t('settings.behavior.typingSpeed', 'Typing Speed')}
            description={t(
              'settings.behavior.typingSpeedDescription',
              'Speed of typewriter effect (50-400 chars/sec)'
            )}
            icon={Gauge}
            variant="stacked"
          >
            <div className="space-y-sm">
              {/* Slider with value display */}
              <div className="flex items-center gap-md">
                <Slider
                  value={[behavior.typing_speed]}
                  onValueChange={([value]) =>
                    updateBehavior({ typing_speed: value })
                  }
                  min={50}
                  max={400}
                  step={10}
                  className="flex-1"
                />
                <span className="text-code text-muted-foreground w-24 text-right">
                  {behavior.typing_speed} chars/sec
                </span>
              </div>

              {/* Speed indicator */}
              <div className="flex items-center justify-between text-caption text-muted-foreground">
                <span>{t('settings.behavior.speedSlow', 'Slow')}</span>
                <span>{t('settings.behavior.speedFast', 'Fast')}</span>
              </div>

              {/* Preview button */}
              <TypingPreviewDialog speed={behavior.typing_speed} />
            </div>
          </SettingsCard>
        </SettingsSection>
      )}

      {/* Auto Dismiss Section */}
      <SettingsSection
        header={t('settings.behavior.autoDismissSection', 'Auto Dismiss')}
      >
        <SettingsCard
          title={t('settings.behavior.autoDismiss', 'Auto Dismiss Delay')}
          description={t(
            'settings.behavior.autoDismissDescription',
            'Seconds before success message dismisses'
          )}
        >
          <div className="flex items-center gap-md w-48">
            <Slider
              value={[behavior.auto_dismiss_delay]}
              onValueChange={([value]) =>
                updateBehavior({ auto_dismiss_delay: value })
              }
              min={1}
              max={10}
              step={1}
              className="flex-1"
            />
            <span className="text-caption text-muted-foreground w-8 text-right">
              {behavior.auto_dismiss_delay}s
            </span>
          </div>
        </SettingsCard>
      </SettingsSection>

      {/* Notifications Section */}
      <SettingsSection
        header={t('settings.behavior.notificationsSection', 'Notifications')}
      >
        <SettingsCard
          title={t('settings.behavior.showNotifications', 'Show Notifications')}
          description={t(
            'settings.behavior.showNotificationsDescription',
            'Display system notifications for important events'
          )}
          icon={Bell}
        >
          <Switch
            checked={behavior.show_notifications}
            onCheckedChange={(checked) =>
              updateBehavior({ show_notifications: checked })
            }
          />
        </SettingsCard>
      </SettingsSection>

      {/* PII Scrubbing Section */}
      <SettingsSection
        header={t('settings.behavior.piiSection', 'Privacy Protection')}
      >
        <SettingsCard
          title={t('settings.behavior.piiScrubbing', 'PII Scrubbing')}
          description={t(
            'settings.behavior.piiScrubbingDescription',
            'Automatically detect and mask sensitive information before sending to AI'
          )}
          icon={Shield}
          variant="section"
        >
          <div className="space-y-md">
            {/* Main toggle */}
            <div className="flex items-center justify-between">
              <span className="text-body">
                {t('settings.behavior.piiEnable', 'Enable PII Scrubbing')}
              </span>
              <Switch
                checked={behavior.pii_masking}
                onCheckedChange={(checked) =>
                  updateBehavior({ pii_masking: checked })
                }
              />
            </div>

            {/* PII type checkboxes (only visible when enabled) */}
            {behavior.pii_masking && (
              <>
                <div className="border-t border-border pt-md">
                  <p className="text-caption text-muted-foreground mb-sm font-medium">
                    {t('settings.behavior.piiTypesLabel', 'Data types to mask:')}
                  </p>
                  <div className="space-y-sm">
                    <PiiTypeCheckbox
                      icon={Mail}
                      label={t('settings.behavior.piiTypeEmail', 'Email addresses')}
                      example="user@example.com → [EMAIL]"
                      checked={behavior.pii_scrub_email ?? true}
                      onChange={(checked) =>
                        updateBehavior({ pii_scrub_email: checked })
                      }
                    />
                    <PiiTypeCheckbox
                      icon={Phone}
                      label={t('settings.behavior.piiTypePhone', 'Phone numbers')}
                      example="+1-555-123-4567 → [PHONE]"
                      checked={behavior.pii_scrub_phone ?? true}
                      onChange={(checked) =>
                        updateBehavior({ pii_scrub_phone: checked })
                      }
                    />
                    <PiiTypeCheckbox
                      icon={Hash}
                      label={t('settings.behavior.piiTypeSSN', 'Social Security Numbers')}
                      example="123-45-6789 → [SSN]"
                      checked={behavior.pii_scrub_ssn ?? true}
                      onChange={(checked) =>
                        updateBehavior({ pii_scrub_ssn: checked })
                      }
                    />
                    <PiiTypeCheckbox
                      icon={CreditCard}
                      label={t('settings.behavior.piiTypeCreditCard', 'Credit card numbers')}
                      example="4111-1111-1111-1111 → [CARD]"
                      checked={behavior.pii_scrub_credit_card ?? true}
                      onChange={(checked) =>
                        updateBehavior({ pii_scrub_credit_card: checked })
                      }
                    />
                  </div>
                </div>
              </>
            )}
          </div>
        </SettingsCard>
      </SettingsSection>
    </div>
  );
}

// PII Type Checkbox Component
interface PiiTypeCheckboxProps {
  icon: typeof Mail;
  label: string;
  example: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
}

function PiiTypeCheckbox({
  icon: Icon,
  label,
  example,
  checked,
  onChange,
}: PiiTypeCheckboxProps) {
  return (
    <label className="flex items-start gap-sm cursor-pointer group">
      <Checkbox
        checked={checked}
        onCheckedChange={onChange}
        className="mt-0.5"
      />
      <div className="flex items-center gap-sm flex-1">
        <Icon className="w-4 h-4 text-warning shrink-0" />
        <div>
          <span className="text-body group-hover:text-foreground transition-colors">
            {label}
          </span>
          <p className="text-caption text-muted-foreground font-mono">
            {example}
          </p>
        </div>
      </div>
    </label>
  );
}

// Typing Preview Dialog Component
interface TypingPreviewDialogProps {
  speed: number;
}

function TypingPreviewDialog({ speed }: TypingPreviewDialogProps) {
  const { t } = useTranslation();
  const [isOpen, setIsOpen] = useState(false);
  const [displayedText, setDisplayedText] = useState('');
  const [isAnimating, setIsAnimating] = useState(false);
  const animationRef = useRef<number | null>(null);

  const sampleText =
    'This is a preview of the typewriter effect at your selected speed. Watch how each character appears one by one, creating a natural typing animation.';

  const startAnimation = () => {
    if (isAnimating) return;

    setIsAnimating(true);
    setDisplayedText('');

    const charactersPerSecond = speed;
    const delayBetweenChars = 1000 / charactersPerSecond;
    let currentIndex = 0;

    const animate = () => {
      if (currentIndex < sampleText.length) {
        setDisplayedText(sampleText.slice(0, currentIndex + 1));
        currentIndex++;
        animationRef.current = window.setTimeout(animate, delayBetweenChars);
      } else {
        setIsAnimating(false);
      }
    };

    animate();
  };

  const resetAnimation = () => {
    if (animationRef.current) {
      clearTimeout(animationRef.current);
    }
    setDisplayedText('');
    setIsAnimating(false);
  };

  useEffect(() => {
    if (isOpen) {
      startAnimation();
    } else {
      resetAnimation();
    }
    return () => {
      if (animationRef.current) {
        clearTimeout(animationRef.current);
      }
    };
  }, [isOpen, speed]);

  return (
    <Dialog open={isOpen} onOpenChange={setIsOpen}>
      <DialogTrigger asChild>
        <Button variant="outline" size="sm">
          <Play className="w-4 h-4 mr-1.5" />
          {t('settings.behavior.previewButton', 'Preview')}
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <DialogTitle>
            {t('settings.behavior.previewTitle', 'Typing Speed Preview')}
          </DialogTitle>
        </DialogHeader>
        <div className="space-y-md">
          <div className="flex items-center gap-sm text-caption text-muted-foreground">
            <span>{t('settings.behavior.speedLabel', 'Speed:')}</span>
            <span className="font-mono">{speed} characters/second</span>
          </div>
          <div className="bg-muted rounded-md p-md min-h-[120px]">
            <p className="text-body">{displayedText}</p>
            {displayedText && !isAnimating && (
              <span className="inline-block w-0.5 h-4 bg-primary animate-pulse ml-0.5" />
            )}
          </div>
          <div className="flex gap-sm">
            <Button
              variant="default"
              size="sm"
              onClick={startAnimation}
              disabled={isAnimating}
            >
              <Play className="w-4 h-4 mr-1.5" />
              {isAnimating
                ? t('settings.behavior.animating', 'Animating...')
                : t('settings.behavior.startPreview', 'Start')}
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={resetAnimation}
              disabled={!displayedText && !isAnimating}
            >
              {t('settings.behavior.reset', 'Reset')}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

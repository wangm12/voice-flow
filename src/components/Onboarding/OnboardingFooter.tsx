import { ArrowLeft, ArrowRight } from "lucide-react";
import { buttonClass, ghostButtonClass, secondaryButtonClass } from "../../lib/theme";
import { iconPropsSm } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";

export function OnboardingFooter({
  step,
  canNext,
  busy,
  busyLabel,
  onBack,
  onSkip,
  onNext,
  onFinish,
}: {
  step: number;
  canNext: boolean;
  busy: boolean;
  busyLabel?: string;
  onBack: () => void;
  onSkip: () => void;
  onNext: () => void;
  onFinish: () => void;
}) {
  const { t } = useI18n();
  return (
    <footer className="shrink-0 border-t border-border">
      <div className={`vf-onboarding-footer-inner flex w-full flex-wrap items-center justify-between gap-3 py-4 ${step === 0 ? "vf-onboarding-footer-inner--welcome" : ""}`}>
        <button type="button" onClick={onBack} disabled={step === 0 || busy} className={`${secondaryButtonClass} min-w-[88px]`}>
          <ArrowLeft {...iconPropsSm} />
          {t("上一步")}
        </button>

        <div className="vf-onboarding-footer-actions ml-auto flex min-w-0 max-w-full flex-wrap items-center justify-end gap-2">
          {step < 5 && (
            <button type="button" onClick={onSkip} disabled={busy} className={ghostButtonClass}>
              {t("稍后设置")}
            </button>
          )}
          {step === 5 ? (
            <button type="button" onClick={onFinish} aria-busy={busy} disabled={busy} className={`${buttonClass} min-w-36`}>
              {busy ? busyLabel ?? t("保存中…") : t("开始使用")}
            </button>
          ) : (
            <button type="button" onClick={onNext} aria-busy={busy} disabled={busy || !canNext} className={`${buttonClass} min-w-36`}>
              {busy ? busyLabel ?? t("保存中…") : t("继续")}
              <ArrowRight {...iconPropsSm} />
            </button>
          )}
        </div>
      </div>
    </footer>
  );
}

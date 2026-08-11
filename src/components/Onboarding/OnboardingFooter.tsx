import { ArrowLeft, ArrowRight } from "lucide-react";
import { buttonClass, ghostButtonClass, secondaryButtonClass } from "../../lib/theme";
import { iconPropsSm } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";

export function OnboardingFooter({
  step,
  canNext,
  busy,
  onBack,
  onSkip,
  onNext,
  onFinish,
}: {
  step: number;
  canNext: boolean;
  busy: boolean;
  onBack: () => void;
  onSkip: () => void;
  onNext: () => void;
  onFinish: () => void;
}) {
  const { t } = useI18n();
  return (
    <footer className="shrink-0 border-t border-border">
      <div className="flex items-center justify-between gap-3 px-8 py-3">
        <button type="button" onClick={onBack} disabled={step === 0 || busy} className={`${secondaryButtonClass} min-w-[88px]`}>
          <ArrowLeft {...iconPropsSm} />
          {t("上一步")}
        </button>

        <div className="flex items-center gap-2">
          {step < 5 && (
            <button type="button" onClick={onSkip} disabled={busy} className={ghostButtonClass}>
              {t("稍后设置")}
            </button>
          )}
          {step === 5 ? (
            <button type="button" onClick={onFinish} disabled={busy} className={`${buttonClass} min-w-[88px]`}>
              {busy ? t("保存中…") : t("开始使用")}
            </button>
          ) : (
            <button type="button" onClick={onNext} disabled={busy || !canNext} className={`${buttonClass} min-w-[88px]`}>
              {busy ? t("保存中…") : t("继续")}
              <ArrowRight {...iconPropsSm} />
            </button>
          )}
        </div>
      </div>
    </footer>
  );
}

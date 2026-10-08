import { CheckCircle2, Keyboard } from "lucide-react";
import { finishHints } from "../../lib/activationCopy";
import { iconPropsSm, iconPropsLg } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";

export function FinishStep({
  hotkeyDisplay,
  activationMode,
}: {
  hotkeyDisplay: string;
  activationMode: string;
}) {
  const { t } = useI18n();
  const hints = finishHints(activationMode, hotkeyDisplay, t);

  return (
    <div className="text-center">
      <div className="mx-auto flex h-11 w-11 items-center justify-center rounded-xl bg-success/10 text-success-ink">
        <CheckCircle2 {...iconPropsLg} className="text-success-ink" />
      </div>
      <h1 className="mt-4 text-[28px] font-semibold leading-tight tracking-tight">{t("准备好了")}</h1>
      <p className="mt-2 text-[13px] leading-6 text-secondary">{t("录音时底部会显示小型状态条，处理时用加载动画提示进度。")}</p>
      <p className="mt-2 text-xs text-tertiary">{t("开启 AI 整理后，VoiceFlow 可根据当前 App 选择表达方式。")}</p>
      <div className="mx-auto mt-6 max-w-sm border-y border-border py-4 text-left text-sm text-secondary">
        {hints.map((hint) => (
          <p key={hint} className="mt-2.5 flex items-center gap-2 first:mt-0">
            <Keyboard {...iconPropsSm} className="shrink-0 text-tertiary" aria-hidden="true" />
            {hint}
          </p>
        ))}
      </div>
    </div>
  );
}

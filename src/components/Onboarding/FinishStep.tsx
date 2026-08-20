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
      <div className="mx-auto flex h-11 w-11 items-center justify-center rounded-xl bg-success/10 text-success">
        <CheckCircle2 {...iconPropsLg} className="text-success" />
      </div>
      <h1 className="mt-4 text-2xl font-semibold tracking-tight">{t("准备好了")}</h1>
      <p className="mt-2 text-sm text-secondary">{t("录音时底部会显示小型状态条，处理时用加载动画提示进度。")}</p>
      <p className="mt-2 text-xs text-tertiary">{t("VoiceFlow 会根据当前 App，选择更适合代码、邮件或聊天的表达方式。")}</p>
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

import { Check } from "lucide-react";
import { iconPropsSm } from "../../lib/icons";
import { useI18n } from "../../lib/i18n";

export function OnboardingSidebar({ step, selectedActionTrial = false }: { step: number; selectedActionTrial?: boolean }) {
  const { t } = useI18n();
  const steps = ["开始", "权限", "连接服务", "语音输入", ...(selectedActionTrial ? ["选中文本"] : []), "完成"];
  const currentStep = step === 5 && !selectedActionTrial ? 4 : step;
  return (
    <aside aria-label={t("新手设置进度")} className="vf-settings-sidebar hidden shrink-0 border-r border-border px-5 py-6 sm:block">
      <div className="flex items-center justify-between gap-3">
        <div className="text-base font-semibold tracking-tight text-primary">VoiceFlow</div>
        <p className="text-xs tabular-nums text-tertiary">{currentStep + 1}/{steps.length}</p>
      </div>

      <ol className="mt-8 space-y-1">
        {steps.map((label, index) => {
          const done = index < currentStep;
          const active = index === currentStep;

          return (
            <li key={label} aria-current={active ? "step" : undefined} className={`flex items-center gap-3 rounded-lg px-3 py-2.5 ${active ? "bg-accent-soft" : ""}`}>
              <span className={`flex h-5 w-5 shrink-0 items-center justify-center ${done ? "text-success-ink" : active ? "text-accent" : "text-tertiary"}`}>
                {done ? <Check {...iconPropsSm} /> : <span className="text-xs tabular-nums">{index + 1}</span>}
              </span>
              <span className={`text-sm ${active ? "font-medium text-accent" : done ? "text-secondary" : "text-tertiary"}`}>{t(label)}</span>
            </li>
          );
        })}
      </ol>
    </aside>
  );
}
